//! Hosted request-classifier backend — an OpenAI-compatible chat-completions endpoint.
//!
//! Some deployments would rather point the classifier at a hosted model than ship a local
//! one: better accuracy on ambiguous requests, at the cost of a network call and per-decision
//! price. This backend implements the same [`RequestClassifier`] interface, so the router can
//! switch to it with `CLASSIFIER_BACKEND=hosted` and no other change.
//!
//! ## Contract
//!
//! The endpoint is the full chat-completions URL (e.g.
//! `https://host/v1/chat/completions`), the model name is supplied by `CLASSIFIER_MODEL`, and
//! the bearer token by `CLASSIFIER_API_KEY`. The model is asked to return a single JSON object
//! with `request_type`, `complexity` (1–5) and `confidence` (0–1). Anything else — a non-2xx
//! status, unparseable content, an unknown label, an out-of-range number — is
//! [`ClassifyError::InvalidResponse`], never a guessed label. The caller
//! ([`ResilientClassifier`](super::classifier::ResilientClassifier)) then falls back to regex
//! and counts it.
//!
//! Sampling is disabled (`temperature: 0`) so a hosted run is reproducible modulo the
//! provider's own determinism guarantees.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

const SYSTEM_PROMPT: &str = "You classify a user's request into exactly one request type, \
then rate its complexity and your confidence.\n\
Allowed request_type values: code_generation, code_understanding, technical_design, \
analytical_reasoning, writing, factual_lookup, general.\n\
complexity: integer 1 (trivial) to 5 (intricate cross-component reasoning).\n\
confidence: number 0 to 1, your probability that the request_type is correct.\n\
Reply with ONLY a JSON object of the form \
{\"request_type\": \"...\", \"complexity\": 1, \"confidence\": 0.9} and nothing else.";

/// An OpenAI-compatible chat-completions classifier.
pub struct HostedClassifier {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl std::fmt::Debug for HostedClassifier {
    /// Debug view that **redacts the API key** — a derived `Debug` would print it verbatim
    /// into any log line or panic message that formats the struct.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedClassifier")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .finish()
    }
}

impl HostedClassifier {
    /// Build the backend. `endpoint` is the full chat-completions URL; an empty endpoint or
    /// model is [`ClassifyError::ModelUnavailable`] (the config layer leaves the backend
    /// unselected in that case, but this is the last line of defence).
    pub fn new(
        endpoint: &str,
        model: &str,
        api_key: &str,
        timeout: Duration,
    ) -> Result<Self, ClassifyError> {
        if endpoint.trim().is_empty() {
            return Err(ClassifyError::ModelUnavailable(
                "CLASSIFIER_ENDPOINT is empty for the hosted backend".into(),
            ));
        }
        if model.trim().is_empty() {
            return Err(ClassifyError::ModelUnavailable(
                "CLASSIFIER_MODEL is empty for the hosted backend".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| ClassifyError::Inference(format!("could not build HTTP client: {e}")))?;
        Ok(Self {
            client,
            endpoint: endpoint.trim().to_string(),
            model: model.trim().to_string(),
            api_key: api_key.to_string(),
        })
    }

    fn build_body(&self, input: &ClassifyInput<'_>) -> Value {
        let context = input.context.map(str::trim).filter(|c| !c.is_empty());
        let user = match context {
            Some(context) => format!("Query:\n{}\n\nContext:\n{}", input.query, context),
            None => format!("Query:\n{}", input.query),
        };
        json!({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 100,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user },
            ],
        })
    }
}

#[async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .json(&self.build_body(input));
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ClassifyError::Inference(format!("request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ClassifyError::InvalidResponse(format!(
                "endpoint returned HTTP {status}"
            )));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| ClassifyError::InvalidResponse(format!("response was not JSON: {e}")))?;
        let content = body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ClassifyError::InvalidResponse("missing choices[0].message.content".into())
            })?;
        parse_classification(content)
    }
}

/// Parse the model's text into a [`Classification`], tolerating a surrounding markdown code
/// fence. Strict about values: an unknown label or out-of-range number is an error, not a
/// coerced guess.
pub fn parse_classification(content: &str) -> Result<Classification, ClassifyError> {
    let trimmed = content.trim();
    let json_slice = extract_json_object(trimmed).unwrap_or(trimmed);
    let value: Value = serde_json::from_str(json_slice).map_err(|e| {
        ClassifyError::InvalidResponse(format!("content was not a JSON object: {e}"))
    })?;

    let label = value
        .get("request_type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ClassifyError::InvalidResponse("missing string field `request_type`".into())
        })?;
    let request_type = RequestType::from_wire(label)
        .ok_or_else(|| ClassifyError::InvalidResponse(format!("unknown request_type {label:?}")))?;

    let complexity = value
        .get("complexity")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ClassifyError::InvalidResponse("missing integer field `complexity`".into())
        })?;
    if !(1..=5).contains(&complexity) {
        return Err(ClassifyError::InvalidResponse(format!(
            "complexity {complexity} is outside 1..=5"
        )));
    }

    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            ClassifyError::InvalidResponse("missing numeric field `confidence`".into())
        })?;
    if !confidence.is_finite() {
        return Err(ClassifyError::InvalidResponse(
            "confidence is not finite".into(),
        ));
    }
    let confidence = confidence.clamp(0.0, 1.0) as f32;

    Ok(Classification {
        request_type,
        complexity: complexity as u8,
        confidence,
    })
}

/// Extract the outermost `{...}` object from text that may include prose or a code fence.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_json_object() {
        let c =
            parse_classification(r#"{"request_type":"writing","complexity":2,"confidence":0.8}"#)
                .unwrap();
        assert_eq!(c.request_type, RequestType::Writing);
        assert_eq!(c.complexity, 2);
        assert!((c.confidence - 0.8).abs() < 1e-6);
    }

    #[test]
    fn parses_json_wrapped_in_a_code_fence() {
        let c = parse_classification("```json\n{\"request_type\":\"factual_lookup\",\"complexity\":1,\"confidence\":0.9}\n```").unwrap();
        assert_eq!(c.request_type, RequestType::FactualLookup);
    }

    #[test]
    fn rejects_an_unknown_label() {
        let err =
            parse_classification(r#"{"request_type":"banana","complexity":1,"confidence":0.5}"#)
                .unwrap_err();
        assert!(matches!(err, ClassifyError::InvalidResponse(_)));
    }

    #[test]
    fn rejects_out_of_range_complexity() {
        let err =
            parse_classification(r#"{"request_type":"writing","complexity":9,"confidence":0.5}"#)
                .unwrap_err();
        assert!(matches!(err, ClassifyError::InvalidResponse(_)));
    }

    #[test]
    fn clamps_out_of_range_confidence() {
        let c =
            parse_classification(r#"{"request_type":"writing","complexity":2,"confidence":1.7}"#)
                .unwrap();
        assert_eq!(c.confidence, 1.0);
    }

    #[test]
    fn rejects_non_json_content() {
        assert!(parse_classification("I think this is writing.").is_err());
    }

    #[test]
    fn empty_endpoint_is_unavailable_not_a_panic() {
        let err = HostedClassifier::new("", "m", "k", Duration::from_millis(100)).unwrap_err();
        assert!(matches!(err, ClassifyError::ModelUnavailable(_)));
    }

    #[tokio::test]
    async fn classifies_against_a_mock_openai_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "{\"request_type\":\"technical_design\",\"complexity\":4,\"confidence\":0.77}"
                        }
                    }]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let classifier = HostedClassifier::new(
            &format!("{}/v1/chat/completions", server.url()),
            "test-model",
            "test-key",
            Duration::from_secs(5),
        )
        .unwrap();
        let out = classifier
            .classify(&ClassifyInput {
                query: "Design a schema",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(out.request_type, RequestType::TechnicalDesign);
        assert_eq!(out.complexity, 4);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_error_is_an_invalid_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(500)
            .with_body("nope")
            .create_async()
            .await;
        let classifier = HostedClassifier::new(
            &format!("{}/v1/chat/completions", server.url()),
            "test-model",
            "",
            Duration::from_secs(5),
        )
        .unwrap();
        let err = classifier
            .classify(&ClassifyInput {
                query: "hello",
                context: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ClassifyError::InvalidResponse(_)));
        mock.assert_async().await;
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let classifier = HostedClassifier::new(
            "https://example.test/v1/chat/completions",
            "m",
            "super-secret",
            Duration::from_secs(1),
        )
        .unwrap();
        let rendered = format!("{classifier:?}");
        assert!(
            !rendered.contains("super-secret"),
            "api key must not be printed: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
    }
}
