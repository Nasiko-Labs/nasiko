//! Executable-only configuration, shared by the evaluator and standalone router.
//! Library implementations accept explicit settings and never read backend env vars.
use nasiko_llm_router::routing::classifier::{
    ClassifierStats, FallbackClassifier, HostedClassifier, LocalClassifier, RegexClassifier,
    RequestClassifier,
};
use std::sync::Arc;
use std::time::Duration;

pub struct ClassifierConfig {
    backend: String,
    endpoint: String,
    model_path: Option<String>,
    timeout: Duration,
}

impl ClassifierConfig {
    pub fn from_env() -> Result<Self, String> {
        let backend = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".into());
        if !matches!(backend.as_str(), "regex" | "local" | "hosted") {
            return Err("CLASSIFIER_BACKEND must be regex, local or hosted".into());
        }
        let timeout_ms = std::env::var("CLASSIFIER_TIMEOUT_MS")
            .unwrap_or_else(|_| "200".into())
            .parse::<u64>()
            .map_err(|_| "CLASSIFIER_TIMEOUT_MS must be an integer".to_string())?;
        if !(1..=200).contains(&timeout_ms) {
            return Err("CLASSIFIER_TIMEOUT_MS must be in 1..=200".into());
        }
        Ok(Self {
            backend,
            endpoint: std::env::var("CLASSIFIER_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:8000/classify".into()),
            model_path: std::env::var("CLASSIFIER_MODEL_PATH").ok(),
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    pub fn build(&self) -> (Arc<dyn RequestClassifier>, Arc<ClassifierStats>) {
        if self.backend == "regex" {
            return (
                Arc::new(RegexClassifier),
                Arc::new(ClassifierStats::default()),
            );
        }
        if self.backend == "hosted" {
            match HostedClassifier::with_options(self.endpoint.clone(), self.timeout) {
                Ok(classifier) => {
                    let stats = classifier.stats.clone();
                    return (Arc::new(classifier), stats);
                }
                Err(error) => eprintln!(
                    "Hosted classifier configuration failed: {error}; using regex fallback"
                ),
            }
        }
        let backend: Result<Arc<dyn RequestClassifier>, String> = match self.backend.as_str() {
            "local" => {
                let loaded = match &self.model_path {
                    Some(path) => std::fs::read_to_string(path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| LocalClassifier::from_json(&s)),
                    None => LocalClassifier::embedded(),
                };
                loaded.map(|model| Arc::new(model) as Arc<dyn RequestClassifier>)
            }
            _ => HostedClassifier::with_options(self.endpoint.clone(), self.timeout)
                .map(|model| Arc::new(model) as Arc<dyn RequestClassifier>),
        };
        if let Err(error) = &backend {
            eprintln!("Classifier backend failed to load: {error}; using regex fallback");
        }
        let classifier = FallbackClassifier::new(self.backend.clone(), backend, self.timeout);
        let stats = classifier.stats.clone();
        (Arc::new(classifier), stats)
    }
}
