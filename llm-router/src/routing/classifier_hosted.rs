//! Hosted request-type classifier — asks an OpenAI-compatible `/chat/completions` endpoint to
//! label the request. Opt-in only (`CLASSIFIER_BACKEND=hosted`); the default never touches the
//! network.
//!
//! The model is told to answer with one JSON object. Anything else (HTTP error, bad JSON, an
//! unknown label, out-of-range numbers) is a [`ClassifyError`], which [`GuardedClassifier`]
//! turns into the regex answer. The request carries only the query (and optional context) —
//! the API key is sent to the configured endpoint and nowhere else.
//!
//! [`GuardedClassifier`]: super::classifier::GuardedClassifier

use async_trait::async_trait;
use serde_json::{Value, json};

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

const SYSTEM_PROMPT: &str = "You label user requests for an LLM router. Reply with ONE JSON \
object and nothing else: {\"request_type\": one of [\"code_generation\", \"code_understanding\", \
\"technical_design\", \"analytical_reasoning\", \"writing\", \"factual_lookup\", \"general\"], \
\"complexity\": integer 1-5 (1 trivial, 5 multi-constraint or multi-step engineering/analysis), \
\"confidence\": number 0-1 (your probability that request_type is correct)}. \
code_generation = produce or modify code/scripts/queries/config; code_understanding = explain, \
review or debug existing code/errors; technical_design = architecture, schema, tooling \
trade-offs; analytical_reasoning = maths, estimation, data/decision analysis without code; \
writing = draft/edit/summarize prose; factual_lookup = short factual question; general = \
chit-chat, advice, brainstorming, anything else.";

/// Cap on request text sent to the endpoint (chars) — bounds cost and latency.
const MAX_QUERY_CHARS: usize = 2_000;
const MAX_CONTEXT_CHARS: usize = 2_000;

pub struct HostedClassifier {
    http: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl HostedClassifier {
    /// `endpoint` is the full chat-completions URL. `api_key` may be empty (local gateways).
    pub fn new(http: reqwest::Client, endpoint: String, model: String, api_key: String) -> Self {
        Self {
            http,
            endpoint,
            model,
            api_key,
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Parse the model's JSON answer into a [`Classification`], validating every field.
pub(crate) fn parse_answer(content: &str) -> Result<Classification, ClassifyError> {
    // Tolerate a fenced block or surrounding prose by taking the outermost braces.
    let (start, end) = (content.find('{'), content.rfind('}'));
    let json_str = match (start, end) {
        (Some(s), Some(e)) if e > s => &content[s..=e],
        _ => return Err(ClassifyError::InvalidOutput("no JSON object".into())),
    };
    let v: Value =
        serde_json::from_str(json_str).map_err(|e| ClassifyError::InvalidOutput(e.to_string()))?;
    let rt = v["request_type"]
        .as_str()
        .and_then(RequestType::from_wire)
        .ok_or_else(|| ClassifyError::InvalidOutput("unknown request_type".into()))?;
    let complexity = v["complexity"]
        .as_u64()
        .filter(|c| (1..=5).contains(c))
        .ok_or_else(|| ClassifyError::InvalidOutput("complexity must be 1-5".into()))?
        as u8;
    let confidence = v["confidence"]
        .as_f64()
        .filter(|c| c.is_finite() && (0.0..=1.0).contains(c))
        .ok_or_else(|| ClassifyError::InvalidOutput("confidence must be 0-1".into()))?
        as f32;
    Ok(Classification {
        request_type: rt,
        complexity,
        confidence,
    })
}

#[async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut user = format!("Request:\n{}", truncate(input.query, MAX_QUERY_CHARS));
        if let Some(ctx) = input.context {
            user.push_str(&format!(
                "\n\nContext:\n{}",
                truncate(ctx, MAX_CONTEXT_CHARS)
            ));
        }
        let body = json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 80,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user },
            ],
        });
        let mut req = self.http.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClassifyError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ClassifyError::Backend(format!("HTTP {}", resp.status())));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ClassifyError::Backend(e.to_string()))?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| ClassifyError::InvalidOutput("missing message content".into()))?;
        parse_answer(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(content: &str) -> String {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
            .to_string()
    }

    fn hosted(url: String) -> HostedClassifier {
        HostedClassifier::new(
            reqwest::Client::new(),
            format!("{url}/chat/completions"),
            "m".into(),
            "k".into(),
        )
    }

    #[test]
    fn parses_fenced_and_validates_ranges() {
        let ok = parse_answer(
            "```json\n{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":0.9}\n```",
        )
        .unwrap();
        assert_eq!(ok.request_type, RequestType::Writing);
        assert!(
            parse_answer("{\"request_type\":\"writing\",\"complexity\":9,\"confidence\":0.9}")
                .is_err()
        );
        assert!(
            parse_answer("{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":1.5}")
                .is_err()
        );
        assert!(
            parse_answer("{\"request_type\":\"poetry\",\"complexity\":2,\"confidence\":0.5}")
                .is_err()
        );
        assert!(parse_answer("sorry, I cannot").is_err());
    }

    #[tokio::test]
    async fn happy_path_sends_bearer_and_parses() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer k")
            .with_status(200)
            .with_body(answer(
                "{\"request_type\":\"code_generation\",\"complexity\":3,\"confidence\":0.8}",
            ))
            .create_async()
            .await;
        let c = hosted(server.url())
            .classify(&ClassifyInput {
                query: "write a parser",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(
            c,
            Classification {
                request_type: RequestType::CodeGeneration,
                complexity: 3,
                confidence: 0.8
            }
        );
        m.assert_async().await;
    }

    #[tokio::test]
    async fn http_error_and_garbage_are_errors() {
        let mut server = mockito::Server::new_async().await;
        let _e = server
            .mock("POST", "/chat/completions")
            .with_status(500)
            .create_async()
            .await;
        let err = hosted(server.url())
            .classify(&ClassifyInput {
                query: "x",
                context: None,
            })
            .await;
        assert!(matches!(err, Err(ClassifyError::Backend(_))));
    }
}
