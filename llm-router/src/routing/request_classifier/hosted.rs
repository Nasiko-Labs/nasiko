use async_trait::async_trait;

use super::{ClassifyError, ClassifyInput, Classification, RequestClassifier};

/// Simple hosted backend wrapper. Without a real external endpoint, it behaves as a
/// local deterministic classifier to keep the project demoable without API keys.
pub struct HostedRequestClassifier {
    endpoint: String,
    timeout_ms: u64,
}

impl HostedRequestClassifier {
    pub fn new(endpoint: String, timeout_ms: u64) -> Self {
        Self {
            endpoint,
            timeout_ms,
        }
    }
}

#[async_trait]
impl RequestClassifier for HostedRequestClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        if self.endpoint.trim().is_empty() {
            return Err(ClassifyError::Load(
                "hosted classifier endpoint not configured".to_string(),
            ));
        }
        let local = super::local::LocalRequestClassifier::embedded()?;
        local.classify(input).await
    }
}
