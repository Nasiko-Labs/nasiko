//! Environment and backend construction stay at the router's composition boundary.

use std::sync::Arc;
use std::time::Duration;

use crate::routing::classifier::{Classification, ClassifyError, ClassifyInput, RequestClassifier};
use crate::routing::classifier_runtime::ClassifierRuntime;
use crate::routing::jev::JevClassifier;

#[derive(Clone)]
pub struct ClassifierConfig {
    pub backend: String,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub timeout_ms: u64,
    pub min_confidence: f32,
    pub min_complexity_confidence: f32,
    pub seed: u64,
}

impl std::fmt::Debug for ClassifierConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClassifierConfig")
            .field("backend", &self.backend)
            .field("model", &self.model)
            .field("api_key_set", &!self.api_key.is_empty())
            .field("timeout_ms", &self.timeout_ms)
            .field("min_confidence", &self.min_confidence)
            .field("min_complexity_confidence", &self.min_complexity_confidence)
            .field("seed", &self.seed)
            .finish_non_exhaustive()
    }
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            backend: "regex".into(),
            endpoint: "https://api.typesafe.ai/v1/systemone".into(),
            model: "jev-latest".into(),
            api_key: String::new(),
            timeout_ms: 2_000,
            min_confidence: 0.6,
            min_complexity_confidence: 0.4,
            seed: 42,
        }
    }
}

impl ClassifierConfig {
    /// Called by GatewayConfig at startup and by the cargo eval binary, never per query.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            backend: super::env_or("CLASSIFIER_BACKEND", &defaults.backend),
            endpoint: super::env_or("CLASSIFIER_ENDPOINT", &defaults.endpoint),
            model: super::env_or("CLASSIFIER_MODEL", &defaults.model),
            api_key: super::env_or("CLASSIFIER_API_KEY", &defaults.api_key),
            timeout_ms: super::env_usize("CLASSIFIER_TIMEOUT_MS", defaults.timeout_ms as usize)
                .max(1) as u64,
            min_confidence: std::env::var("CLASSIFIER_MIN_CONFIDENCE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(defaults.min_confidence),
            min_complexity_confidence: std::env::var("CLASSIFIER_MIN_COMPLEXITY_CONFIDENCE")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(defaults.min_complexity_confidence),
            seed: std::env::var("CLASSIFIER_SEED")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(defaults.seed),
        }
    }
}

/// Build once. Unsupported backend or invalid setup is a counted regex fallback on use.
pub fn build_classifier(config: &ClassifierConfig) -> ClassifierRuntime {
    // Materialize the LazyLock regex patterns before decision timing (including fallbacks).
    crate::routing::classifier::classify_request_type("");
    if config.backend == "regex" {
        return ClassifierRuntime::regex();
    }
    let backend: Arc<dyn RequestClassifier> = if config.backend == "jev" {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|_| ClassifyError::Configuration)
            .and_then(|http| {
                JevClassifier::new(
                    http,
                    config.endpoint.clone(),
                    config.model.clone(),
                    config.api_key.clone(),
                )
            })
            .map(|backend| Arc::new(backend) as Arc<dyn RequestClassifier>)
            .unwrap_or_else(|_| Arc::new(UnavailableClassifier))
    } else {
        Arc::new(UnavailableClassifier)
    };
    ClassifierRuntime::new(
        backend,
        Duration::from_millis(config.timeout_ms),
        config.min_confidence,
        config.seed,
    )
    .with_complexity_threshold(config.min_complexity_confidence)
}

struct UnavailableClassifier;

#[async_trait::async_trait]
impl RequestClassifier for UnavailableClassifier {
    fn name(&self) -> &str {
        "unavailable"
    }
    async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Err(ClassifyError::Configuration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_backend_is_observable_fallback_and_default_is_regex() {
        assert!(!build_classifier(&ClassifierConfig::default()).is_experimental());
        let runtime = build_classifier(&ClassifierConfig {
            backend: "typo".into(),
            ..Default::default()
        });
        let result = runtime
            .classify(&ClassifyInput {
                query: "hello",
                context: None,
            })
            .await;
        assert!(result.is_fallback());
        assert_eq!(runtime.counts(), (1, 1));
    }

    #[test]
    fn configuration_debug_redacts_credentials_and_endpoint() {
        let config = ClassifierConfig {
            api_key: "private-secret".into(),
            endpoint: "private-host".into(),
            ..Default::default()
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("private-secret"));
        assert!(!debug.contains("private-host"));
    }
}
