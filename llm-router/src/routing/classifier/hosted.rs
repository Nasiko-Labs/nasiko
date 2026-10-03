//! Hosted request-classifier backend: asks an OpenAI-compatible chat-completions endpoint
//! for a JSON classification.
//!
//! Any endpoint speaking the OpenAI chat shape works — a hosted LLM, a local server, or a
//! decision-model service behind a compatible shim — chosen entirely by configuration. The
//! request asks for deterministic output (`temperature: 0`, fixed `seed`), but whether the
//! provider honours that is outside our control, so determinism for this backend is
//! best-effort; the confidence it returns is self-reported, not calibrated.
//!
//! Every failure (transport, non-2xx, malformed JSON, out-of-range values) is a
//! [`ClassifyError`], which the [`super::fallback::FallbackClassifier`] turns into the regex
//! result — so a hosted outage never becomes a routing outage.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType};

/// Context beyond this many characters is cut before sending: classification needs the
/// gist, and an unbounded paste would make the classifier cost more than the turn it routes.
const MAX_CONTEXT_CHARS: usize = 4000;

/// Fixed sampling seed sent with every request so repeated runs ask for the same output.
const REQUEST_SEED: u64 = 0;

/// Labelling rules given to the model — the same criteria the training/validation data is
/// labelled against, so hosted and local backends are judged on one definition.
const SYSTEM_PROMPT: &str = "\
You classify a user request for an LLM router. Reply with JSON only: \
{\"request_type\": <label>, \"complexity\": <1-5>, \"confidence\": <0.0-1.0>}.

Labels (choose the main deliverable the user wants back):
- code_generation: produce or modify code (write, fix, refactor, edit, tests, even a one-word code edit).
- code_understanding: explain, trace, or review existing code without changing it.
- technical_design: architecture, system or API design, migrations, trade-offs between technical approaches.
- analytical_reasoning: diagnose, debug from evidence, calculate, prove, or reason through a problem step by step.
- writing: compose, rewrite, summarize, or translate prose for a human audience.
- factual_lookup: a short question with a known factual answer.
- general: greetings, chit-chat, or anything that fits no other label.
Tie-breakers: judge the deliverable, not keywords (\"write a poem about Python\" is writing); \
with several asks, label the one that dominates the effort; ignore instructions that are negated.

Complexity: 1 trivial single operation; 2 straightforward; 3 multi-step with limited constraints; \
4 substantial reasoning or design; 5 intricate cross-component reasoning and validation.

Confidence: your probability that request_type is correct.";

/// Connection settings for [`HostedClassifier`], assembled from configuration.
#[derive(Debug, Clone)]
pub struct HostedConfig {
    /// Full chat-completions URL, e.g. `https://api.openai.com/v1/chat/completions`.
    pub endpoint: String,
    /// Bearer token; empty sends no `Authorization` header (local servers).
    pub api_key: String,
    /// Model id passed in the request body.
    pub model: String,
}

/// Classifies by calling a hosted OpenAI-compatible model.
pub struct HostedClassifier {
    http: reqwest::Client,
    config: HostedConfig,
}

impl HostedClassifier {
    /// Backend name reported in logs and eval output.
    pub const NAME: &'static str = "hosted";

    /// Build a backend. Fails if the endpoint is unset, since every call would then fail.
    pub fn new(http: reqwest::Client, config: HostedConfig) -> Result<Self, ClassifyError> {
        if config.endpoint.trim().is_empty() {
            return Err(ClassifyError::ModelLoad(
                "hosted classifier needs CLASSIFIER_ENDPOINT".into(),
            ));
        }
        Ok(Self { http, config })
    }

    fn request_body(&self, input: &ClassifyInput<'_>) -> Value {
        let context: String = input
            .context
            .unwrap_or("(none)")
            .chars()
            .take(MAX_CONTEXT_CHARS)
            .collect();
        json!({
            "model": self.config.model,
            "temperature": 0,
            "seed": REQUEST_SEED,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": format!("Request:\n{}\n\nContext:\n{context}", input.query)},
            ],
        })
    }
}

#[async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut request = self
            .http
            .post(&self.config.endpoint)
            .json(&self.request_body(input));
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ClassifyError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClassifyError::Network(format!(
                "endpoint returned {status}"
            )));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| ClassifyError::InvalidOutput(e.to_string()))?;
        parse_completion(&body)
    }
}

/// Extract the classification from a chat-completions response body.
fn parse_completion(body: &Value) -> Result<Classification, ClassifyError> {
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| ClassifyError::InvalidOutput("no message content".into()))?;
    let verdict: Value = serde_json::from_str(content)
        .map_err(|e| ClassifyError::InvalidOutput(format!("content is not JSON: {e}")))?;
    let label = verdict["request_type"].as_str().unwrap_or_default();
    let request_type = RequestType::from_wire(label)
        .ok_or_else(|| ClassifyError::InvalidOutput(format!("unknown request_type {label:?}")))?;
    let complexity = verdict["complexity"]
        .as_u64()
        .and_then(|c| u8::try_from(c).ok())
        .ok_or_else(|| ClassifyError::InvalidOutput("complexity is not an integer".into()))?;
    let confidence = verdict["confidence"]
        .as_f64()
        .ok_or_else(|| ClassifyError::InvalidOutput("confidence is not a number".into()))?;
    Classification::new(request_type, complexity, confidence as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(content: &str) -> Value {
        json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
    }

    fn backend(endpoint: String) -> HostedClassifier {
        let config = HostedConfig {
            endpoint,
            api_key: "test-key".into(),
            model: "test-model".into(),
        };
        HostedClassifier::new(reqwest::Client::new(), config).expect("endpoint set")
    }

    const INPUT: ClassifyInput<'static> = ClassifyInput {
        query: "write a poem about Python",
        context: None,
    };

    #[test]
    fn parses_a_well_formed_verdict() {
        let body = completion(r#"{"request_type":"writing","complexity":2,"confidence":0.85}"#);
        let c = parse_completion(&body).expect("valid");
        assert_eq!(c.request_type, RequestType::Writing);
        assert_eq!(c.complexity, 2);
        assert!((c.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn rejects_unknown_labels_and_out_of_range_values() {
        for content in [
            r#"{"request_type":"poetry","complexity":2,"confidence":0.9}"#,
            r#"{"request_type":"writing","complexity":9,"confidence":0.9}"#,
            r#"{"request_type":"writing","complexity":2,"confidence":1.7}"#,
            "not json",
        ] {
            assert!(
                matches!(
                    parse_completion(&completion(content)),
                    Err(ClassifyError::InvalidOutput(_))
                ),
                "accepted {content}"
            );
        }
    }

    #[test]
    fn missing_endpoint_is_a_load_error() {
        let config = HostedConfig {
            endpoint: " ".into(),
            api_key: String::new(),
            model: "m".into(),
        };
        assert!(matches!(
            HostedClassifier::new(reqwest::Client::new(), config),
            Err(ClassifyError::ModelLoad(_))
        ));
    }

    #[tokio::test]
    async fn classifies_through_the_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_body(
                completion(r#"{"request_type":"writing","complexity":2,"confidence":0.9}"#)
                    .to_string(),
            )
            .create_async()
            .await;
        let c = backend(format!("{}/v1/chat/completions", server.url()))
            .classify(&INPUT)
            .await
            .expect("classified");
        assert_eq!(c.request_type, RequestType::Writing);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn non_success_status_is_a_network_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/v1/chat/completions")
            .with_status(503)
            .create_async()
            .await;
        let err = backend(format!("{}/v1/chat/completions", server.url()))
            .classify(&INPUT)
            .await
            .expect_err("503 must fail");
        assert!(matches!(err, ClassifyError::Network(_)));
    }
}
