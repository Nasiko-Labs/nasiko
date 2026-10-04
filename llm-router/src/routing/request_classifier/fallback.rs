use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use async_trait::async_trait;

use super::{
    ClassifyError, ClassifyInput, Classification, RequestClassifier,
};

/// Wraps an experimental backend and degrades to the regex classifier when the
/// backend fails or returns invalid data.
pub struct FallbackRequestClassifier {
    inner: Arc<dyn RequestClassifier>,
    regex: Arc<dyn RequestClassifier>,
    fallback_count: AtomicU64,
}

impl FallbackRequestClassifier {
    pub fn new(inner: Arc<dyn RequestClassifier>, regex: Arc<dyn RequestClassifier>) -> Self {
        Self {
            inner,
            regex,
            fallback_count: AtomicU64::new(0),
        }
    }

    pub fn fallback_count(&self) -> u64 {
        self.fallback_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RequestClassifier for FallbackRequestClassifier {
    fn name(&self) -> &str {
        "fallback"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        match self.inner.classify(input).await {
            Ok(out) => {
                if out.validate().is_err() {
                    self.fallback_count.fetch_add(1, Ordering::SeqCst);
                    tracing::warn!(target: "nasiko::llm_router::classifier", "fallback: invalid ML output; using regex");
                    let mut fallback = self.regex.classify(input).await?;
                    fallback.fallback = true;
                    return Ok(fallback);
                }
                Ok(out)
            }
            Err(e) => {
                self.fallback_count.fetch_add(1, Ordering::SeqCst);
                tracing::warn!(target: "nasiko::llm_router::classifier", error = %e, "fallback: ML backend failed; using regex");
                let mut fallback = self.regex.classify(input).await?;
                fallback.fallback = true;
                Ok(fallback)
            }
        }
    }
}
