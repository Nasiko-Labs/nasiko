//! Hosted `RequestClassifier` backend — a single direct call to an OpenAI-compatible
//! chat-completion endpoint, asking the model to classify the query into a
//! [`RequestType`], a coarse `complexity`, and its own `confidence`.
//!
//! Deliberately bypasses `crate::providers::fallback::execute_chat` (the client-facing
//! retry/param-fix/cross-provider-fallback machinery): this is one internal call with one
//! documented retry (a `json_schema` → `json_object` response-format downgrade, see
//! [`is_response_format_rejection`]), not a proxied client request. Timeout and
//! error-to-regex fallback are the caller's job — see [`super::FallbackClassifier`].
//!
//! **Determinism.** `temperature: 0` and a `strict` JSON Schema `response_format` are set
//! on every request, which guarantees the *shape* of the response (valid JSON matching the
//! schema) but not that identical input always yields byte-identical output — provider-side
//! execution (routing, batching, floating-point non-associativity) is outside this crate's
//! control. This backend's reproducibility is a measured, reported property, not a
//! guarantee; the always-available [`super::RegexClassifier`] fallback is what stays
//! provably deterministic.

use serde_json::{Value, json};

use super::classifier::{
    ClassifyError, ClassifyInput, Classification, RequestClassifier, RequestType,
};

/// The 7 wire labels `RequestType` accepts, spelled out once for the JSON Schema `enum`
/// and the system prompt — kept next to each other so the two can't silently drift apart.
const REQUEST_TYPE_LABELS: [&str; 7] = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
];

/// A hosted classifier backend: one call to an OpenAI-compatible chat-completion
/// endpoint. Never retries beyond the single documented `json_schema` → `json_object`
/// downgrade; never times out on its own — wrap in
/// [`FallbackClassifier`](super::FallbackClassifier) for that.
pub struct HostedClassifier {
    http: reqwest::Client,
    /// Full chat-completion endpoint URL (e.g. `https://api.openai.com/v1/chat/completions`
    /// or a self-hosted/proxy equivalent) — not just a base, so this backend stays
    /// provider-shape-agnostic rather than assuming OpenAI's own URL layout.
    endpoint: String,
    model: String,
    /// `CLASSIFIER_API_KEY`, sent as `Authorization: Bearer <key>` when set. `None`/empty
    /// ⇒ no auth header at all (an unauthenticated local/dev endpoint). Never logged.
    api_key: Option<String>,
}

impl HostedClassifier {
    pub fn new(http: reqwest::Client, endpoint: String, model: String) -> Self {
        Self {
            http,
            endpoint,
            model,
            api_key: None,
        }
    }

    /// Set the bearer token sent with every request. A `None`/empty key clears it (no auth
    /// header), matching this backend's default of working against an unauthenticated
    /// endpoint until a key is supplied.
    pub fn with_api_key(mut self, api_key: Option<String>) -> Self {
        self.api_key = api_key.filter(|k| !k.is_empty());
        self
    }

    fn system_prompt(&self) -> String {
        format!(
            "You classify a user's request for an LLM router. Respond ONLY with a JSON \
             object with exactly these fields:\n\
             - request_type: one of {labels:?}\n\
             - complexity: an integer from 1 (trivial) to 5 (very complex, multi-step)\n\
             - confidence: your confidence in request_type, a number from 0.0 to 1.0\n\
             No prose, no markdown fences — the JSON object only.",
            labels = REQUEST_TYPE_LABELS
        )
    }

    fn response_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "request_type": { "type": "string", "enum": REQUEST_TYPE_LABELS },
                "complexity": { "type": "integer", "minimum": 1, "maximum": 5 },
                "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
            },
            "required": ["request_type", "complexity", "confidence"],
            "additionalProperties": false
        })
    }

    fn user_content(input: &ClassifyInput<'_>) -> String {
        match input.context {
            Some(context) if !context.is_empty() => {
                format!("Context: {context}\n\nQuery: {}", input.query)
            }
            _ => input.query.to_string(),
        }
    }

    fn request_body(&self, input: &ClassifyInput<'_>, use_json_object_mode: bool) -> Value {
        let response_format = if use_json_object_mode {
            // Downgrade path: schema is spelled out in the system prompt instead, since the
            // provider has already rejected structured `json_schema` mode.
            json!({ "type": "json_object" })
        } else {
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "classification",
                    "strict": true,
                    "schema": Self::response_schema()
                }
            })
        };
        let system = if use_json_object_mode {
            format!(
                "{}\n\nRespond with a single JSON object (no prose, no markdown fences) \
                 that conforms exactly to this JSON Schema:\n{}",
                self.system_prompt(),
                Self::response_schema()
            )
        } else {
            self.system_prompt()
        };
        json!({
            "model": self.model,
            "temperature": 0,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": Self::user_content(input) }
            ],
            "response_format": response_format
        })
    }

    async fn call(
        &self,
        input: &ClassifyInput<'_>,
        use_json_object_mode: bool,
    ) -> Result<reqwest::Response, ClassifyError> {
        let mut req = self
            .http
            .post(&self.endpoint)
            .json(&self.request_body(input, use_json_object_mode));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        req.send()
            .await
            .map_err(|e| ClassifyError::Network(e.to_string()))
    }
}

/// Whether a non-2xx response is the provider rejecting `json_schema`-mode structured
/// output (some OpenAI-compatible providers — e.g. DeepSeek — don't support it). This is a
/// best-effort heuristic (string match on the error body), the same technique already used
/// in this codebase for the same situation (see `server::capabilities::generator`'s
/// `is_response_format_rejection`) — a provider that rejects it with different wording
/// falls through to a generic backend error (and then to the regex fallback), which is
/// still correct, just forgoes the retry.
fn is_response_format_rejection(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::BAD_REQUEST && body.contains("response_format")
}

/// On-wire shape of the classification JSON the model's `message.content` must parse into.
#[derive(serde::Deserialize)]
struct RawClassification {
    request_type: String,
    complexity: i64,
    confidence: f64,
}

/// Minimal slice of an OpenAI-shaped chat-completion response this backend needs.
#[derive(serde::Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(serde::Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(serde::Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
}

#[async_trait::async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut resp = self.call(input, false).await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if is_response_format_rejection(status, &body) {
                tracing::info!(
                    target: "nasiko::llm_router::classifier",
                    "hosted classifier: provider rejected json_schema response_format; retrying once in json_object mode"
                );
                resp = self.call(input, true).await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(ClassifyError::Backend(format!(
                        "hosted classifier returned {status} (after json_object retry): {body}"
                    )));
                }
            } else {
                return Err(ClassifyError::Backend(format!(
                    "hosted classifier returned {status}: {body}"
                )));
            }
        }

        let parsed: ChatCompletionResponse = resp
            .json()
            .await
            .map_err(|e| ClassifyError::MalformedResponse(format!("invalid response JSON: {e}")))?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| {
                ClassifyError::MalformedResponse("no message content in response".into())
            })?;

        let raw: RawClassification = serde_json::from_str(&content).map_err(|e| {
            ClassifyError::MalformedResponse(format!("content is not the expected JSON: {e}"))
        })?;

        let request_type = RequestType::from_wire(&raw.request_type)
            .ok_or_else(|| ClassifyError::InvalidRequestType(raw.request_type.clone()))?;
        if !(1..=5).contains(&raw.complexity) {
            return Err(ClassifyError::InvalidComplexity(
                raw.complexity.clamp(0, u8::MAX as i64) as u8,
            ));
        }
        Classification::new(request_type, raw.complexity as u8, raw.confidence as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier(endpoint: String) -> HostedClassifier {
        HostedClassifier::new(reqwest::Client::new(), endpoint, "test-model".into())
    }

    #[tokio::test]
    async fn parses_valid_json_schema_response() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{
                "message": {
                    "content": json!({
                        "request_type": "code_generation",
                        "complexity": 3,
                        "confidence": 0.87
                    }).to_string()
                }
            }]
        });
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;

        let c = classifier(server.url());
        let input = ClassifyInput {
            query: "write a function",
            context: None,
        };
        let result = c.classify(&input).await.unwrap();
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.complexity, 3);
        assert!((result.confidence - 0.87).abs() < 1e-6);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn malformed_content_is_malformed_response_error() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{ "message": { "content": "not json at all" } }]
        });
        server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let c = classifier(server.url());
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        let err = c.classify(&input).await.unwrap_err();
        assert!(matches!(err, ClassifyError::MalformedResponse(_)));
    }

    #[tokio::test]
    async fn unknown_request_type_is_invalid_request_type_error() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{
                "message": {
                    "content": json!({
                        "request_type": "not_a_real_type",
                        "complexity": 2,
                        "confidence": 0.5
                    }).to_string()
                }
            }]
        });
        server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let c = classifier(server.url());
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        let err = c.classify(&input).await.unwrap_err();
        assert!(matches!(err, ClassifyError::InvalidRequestType(_)));
    }

    #[tokio::test]
    async fn json_schema_rejection_retries_once_in_json_object_mode() {
        let mut server = mockito::Server::new_async().await;
        let rejection = json!({ "error": { "message": "response_format type json_schema unsupported" } });
        // Distinguish the two calls by body: the first request's response_format is
        // json_schema, the retry's is json_object (mockito matches by request matcher, not
        // registration order, so an unambiguous body matcher per mock is required).
        let first = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("json_schema".into()))
            .with_status(400)
            .with_body(rejection.to_string())
            .expect(1)
            .create_async()
            .await;
        let success_body = json!({
            "choices": [{
                "message": {
                    "content": json!({
                        "request_type": "general",
                        "complexity": 1,
                        "confidence": 0.6
                    }).to_string()
                }
            }]
        });
        let second = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("json_object".into()))
            .with_status(200)
            .with_body(success_body.to_string())
            .expect(1)
            .create_async()
            .await;

        let c = classifier(server.url());
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        let result = c.classify(&input).await.unwrap();
        assert_eq!(result.request_type, RequestType::General);
        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn network_failure_is_network_error() {
        // Nothing is listening on this URL - the connection itself fails.
        let c = classifier("http://127.0.0.1:1".to_string());
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        let err = c.classify(&input).await.unwrap_err();
        assert!(matches!(err, ClassifyError::Network(_)));
    }

    #[tokio::test]
    async fn sends_bearer_auth_header_when_api_key_is_set() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{
                "message": {
                    "content": json!({
                        "request_type": "general",
                        "complexity": 1,
                        "confidence": 0.5
                    }).to_string()
                }
            }]
        });
        let mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer sk-test-123")
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let c = classifier(server.url()).with_api_key(Some("sk-test-123".into()));
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        c.classify(&input).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn sends_no_auth_header_when_api_key_is_unset() {
        let mut server = mockito::Server::new_async().await;
        let body = json!({
            "choices": [{
                "message": {
                    "content": json!({
                        "request_type": "general",
                        "complexity": 1,
                        "confidence": 0.5
                    }).to_string()
                }
            }]
        });
        let mock = server
            .mock("POST", "/")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let c = classifier(server.url());
        let input = ClassifyInput {
            query: "hi",
            context: None,
        };
        c.classify(&input).await.unwrap();
        mock.assert_async().await;
    }
}
