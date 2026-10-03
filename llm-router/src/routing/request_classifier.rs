//! Request classification backends used by cost-aware routing.
//!
//! This is deliberately separate from [`super::classifier`], which owns the
//! existing request-type mapping and tier-selection policy. Backends classify the
//! request; they do not select a provider or model tier.

use async_trait::async_trait;
use serde::Deserialize;
use thiserror::Error;
use std::time::Duration;

use super::classifier::{RequestType, classify_request_type};

/// User input available at a safe routing boundary.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// Classification result. Confidence is the backend's calibrated probability for
/// `request_type`, and complexity is an ordinal score from 1 (simple) to 5 (complex).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

impl Classification {
    /// Validate values returned by an external or loaded backend before routing uses them.
    pub fn validate(self) -> Result<Self, ClassifyError> {
        if !(1..=5).contains(&self.complexity) {
            return Err(ClassifyError::InvalidOutput("complexity must be in 1..=5"));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(ClassifyError::InvalidOutput("confidence must be in 0..=1"));
        }
        Ok(self)
    }
}

#[derive(Debug, Error)]
pub enum ClassifyError {
    #[error("classifier backend failed: {0}")]
    Backend(String),
    #[error("classifier returned invalid output: {0}")]
    InvalidOutput(&'static str),
}

/// Classifies a request at a safe routing boundary. Implementations must be deterministic
/// for identical input and model state.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError>;
}

/// Apply the routing fail-safe policy to a configured backend. Returns whether regex was
/// used so callers can record fallback metrics without logging request content.
pub async fn classify_with_fallback(
    backend: &dyn RequestClassifier,
    input: &ClassifyInput<'_>,
    timeout: Duration,
    min_confidence: f32,
) -> (Classification, bool) {
    let result = tokio::time::timeout(timeout, backend.classify(input)).await;
    match result {
        Ok(Ok(classification)) => match classification.validate() {
            Ok(classification) if classification.confidence >= min_confidence => {
                (classification, false)
            }
            _ => (regex_fallback(input).await, true),
        },
        _ => (regex_fallback(input).await, true),
    }
}

async fn regex_fallback(input: &ClassifyInput<'_>) -> Classification {
    RegexRequestClassifier.classify(input).await.unwrap_or(Classification {
        request_type: RequestType::General,
        complexity: 3,
        confidence: 1.0,
    })
}

/// Existing keyword classifier, retained as the default and fail-safe backend.
/// Complexity and confidence are fixed placeholders because the legacy classifier does not
/// estimate either quantity; confidence `1.0` means only that this is the selected baseline,
/// not that its label is calibrated.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexRequestClassifier;

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        let text = match input.context {
            Some(context) if !context.is_empty() => format!("{}\n{}", input.query, context),
            _ => input.query.to_owned(),
        };
        Ok(Classification {
            request_type: classify_request_type(&text),
            complexity: 3,
            confidence: 1.0,
        })
    }
}

/// HTTP adapter for the private LLMRouter KNN inference sidecar.
///
/// The endpoint is configured by the binary, never by library-level environment reads.
#[derive(Clone)]
pub struct HttpRequestClassifier {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpRequestClassifier {
    pub fn new(client: reqwest::Client, endpoint: impl Into<String>) -> Self {
        Self {
            client,
            endpoint: endpoint.into(),
        }
    }
}

#[derive(Deserialize)]
struct SidecarResponse {
    request_type: String,
    complexity: u8,
    confidence: f32,
}

#[async_trait]
impl RequestClassifier for HttpRequestClassifier {
    fn name(&self) -> &str {
        "llmrouter_knn"
    }

    async fn classify(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "query": input.query,
                "context": input.context,
            }))
            .send()
            .await
            .map_err(|e| ClassifyError::Backend(format!("sidecar request failed: {e}")))?
            .error_for_status()
            .map_err(|e| ClassifyError::Backend(format!("sidecar returned an error: {e}")))?
            .json::<SidecarResponse>()
            .await
            .map_err(|e| ClassifyError::Backend(format!("invalid sidecar response: {e}")))?;

        let request_type = RequestType::from_wire(&response.request_type)
            .ok_or(ClassifyError::InvalidOutput("unknown request_type"))?;
        Classification {
            request_type,
            complexity: response.complexity,
            confidence: response.confidence,
        }
        .validate()
    }
}

/// Build the configured backend. Unknown names fail back to the legacy classifier.
pub fn configured_request_classifier(
    config: &crate::config::GatewayConfig,
    client: reqwest::Client,
) -> std::sync::Arc<dyn RequestClassifier> {
    match config.request_classifier_backend.as_str() {
        "regex" => std::sync::Arc::new(RegexRequestClassifier),
        "llmrouter_knn" if !config.request_classifier_endpoint.is_empty() => {
            std::sync::Arc::new(HttpRequestClassifier::new(
                client,
                config.request_classifier_endpoint.clone(),
            ))
        }
        backend => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                backend,
                "unknown or incomplete request classifier configuration; using regex"
            );
            std::sync::Arc::new(RegexRequestClassifier)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBackend {
        confidence: f32,
        delay: Duration,
        fails: bool,
    }

    #[async_trait]
    impl RequestClassifier for TestBackend {
        fn name(&self) -> &str { "test" }
        async fn classify(&self, _input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(self.delay).await;
            if self.fails { return Err(ClassifyError::Backend("test failure".into())); }
            Ok(Classification { request_type: RequestType::Writing, complexity: 4, confidence: self.confidence })
        }
    }

    #[test]
    fn classification_rejects_out_of_range_and_non_finite_values() {
        let valid = Classification {
            request_type: RequestType::Writing,
            complexity: 5,
            confidence: 0.6,
        };
        assert_eq!(valid.validate().unwrap(), valid);
        assert!(Classification { complexity: 0, ..valid }.validate().is_err());
        assert!(Classification { complexity: 6, ..valid }.validate().is_err());
        assert!(Classification { confidence: f32::NAN, ..valid }.validate().is_err());
        assert!(Classification { confidence: 1.01, ..valid }.validate().is_err());
    }

    #[tokio::test]
    async fn regex_backend_retains_existing_request_type_mapping() {
        let backend = RegexRequestClassifier;
        let input = ClassifyInput {
            query: "Implement a function to sort these values",
            context: None,
        };
        let result = backend.classify(&input).await.unwrap();
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.complexity, 3);
        assert_eq!(result.confidence, 1.0);
    }

    #[tokio::test]
    async fn low_confidence_error_and_timeout_all_fall_back_to_regex() {
        let input = ClassifyInput { query: "Write a helper function", context: None };
        for backend in [
            TestBackend { confidence: 0.84, delay: Duration::ZERO, fails: false },
            TestBackend { confidence: 0.9, delay: Duration::ZERO, fails: true },
            TestBackend { confidence: 0.9, delay: Duration::from_millis(20), fails: false },
        ] {
            let (result, fallback) = classify_with_fallback(&backend, &input, Duration::from_millis(2), 0.85).await;
            assert!(fallback);
            assert_eq!(result.request_type, RequestType::CodeGeneration);
        }
    }
}
