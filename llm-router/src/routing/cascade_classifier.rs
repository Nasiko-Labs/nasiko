//! Cascade request classifier — the local model answers, and only requests it is unsure about
//! are escalated to the hosted classifier.
//!
//! Cost per decision is therefore `escalation rate × hosted cost`, and latency is the local
//! model's microseconds except on escalations. If the hosted call fails or exceeds its share
//! of the budget, the cascade keeps the **local** answer (not the regex), counting an
//! escalation failure — the local answer is still better informed than the regex.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::classifier::{
    Classification, ClassifierStats, ClassifyError, ClassifyInput, RequestClassifier,
};
use super::local_classifier::LocalClassifier;

pub struct CascadeClassifier {
    local: Arc<LocalClassifier>,
    hosted: Arc<dyn RequestClassifier>,
    escalate_below: f32,
    hosted_budget: Duration,
    stats: Arc<ClassifierStats>,
}

impl CascadeClassifier {
    /// `hosted_budget` should leave headroom inside the guard's overall timeout (the factory
    /// uses 80% of it) so a slow escalation still returns the local answer rather than the
    /// regex.
    pub fn new(
        local: Arc<LocalClassifier>,
        hosted: Arc<dyn RequestClassifier>,
        escalate_below: f32,
        hosted_budget: Duration,
        stats: Arc<ClassifierStats>,
    ) -> Self {
        Self {
            local,
            hosted,
            escalate_below,
            hosted_budget,
            stats,
        }
    }
}

#[async_trait::async_trait]
impl RequestClassifier for CascadeClassifier {
    fn name(&self) -> &str {
        "cascade"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let local = self.local.predict(input.query, input.context);
        if local.confidence >= self.escalate_below {
            return Ok(local);
        }
        self.stats.escalations.fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(self.hosted_budget, self.hosted.classify(input)).await {
            Ok(Ok(hosted)) => Ok(hosted),
            Ok(Err(e)) => {
                self.stats
                    .escalation_failures
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    error = %e,
                    "cascade escalation failed; keeping the local answer"
                );
                Ok(local)
            }
            Err(_) => {
                self.stats
                    .escalation_failures
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    budget_ms = self.hosted_budget.as_millis() as u64,
                    "cascade escalation timed out; keeping the local answer"
                );
                Ok(local)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::RequestType;
    use crate::routing::local_classifier::tests::fixture_json;
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        calls: AtomicUsize,
        answer: Option<Classification>,
    }
    #[async_trait::async_trait]
    impl RequestClassifier for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.answer
                .ok_or_else(|| ClassifyError::Unavailable("down".into()))
        }
    }

    fn cascade(
        threshold: f32,
        answer: Option<Classification>,
    ) -> (CascadeClassifier, Arc<Fake>, Arc<ClassifierStats>) {
        let fake = Arc::new(Fake {
            calls: AtomicUsize::new(0),
            answer,
        });
        let stats = Arc::new(ClassifierStats::default());
        let local = Arc::new(LocalClassifier::from_json(&fixture_json()).unwrap());
        (
            CascadeClassifier::new(
                local,
                fake.clone(),
                threshold,
                Duration::from_secs(1),
                stats.clone(),
            ),
            fake,
            stats,
        )
    }

    fn input() -> ClassifyInput<'static> {
        ClassifyInput {
            query: "fix typo in comment",
            context: None,
        }
    }

    #[tokio::test]
    async fn confident_local_never_calls_hosted() {
        let (c, fake, stats) =
            cascade(0.0, Some(Classification::new(RequestType::Writing, 2, 0.9)));
        let got = c.classify(&input()).await.unwrap();
        assert_eq!(got.request_type, RequestType::CodeGeneration);
        assert_eq!(fake.calls.load(Ordering::Relaxed), 0);
        assert_eq!(stats.snapshot().escalations, 0);
    }

    #[tokio::test]
    async fn uncertain_local_escalates() {
        let hosted = Classification::new(RequestType::Writing, 2, 0.9);
        let (c, fake, stats) = cascade(1.01, Some(hosted));
        assert_eq!(c.classify(&input()).await.unwrap(), hosted);
        assert_eq!(fake.calls.load(Ordering::Relaxed), 1);
        assert_eq!(stats.snapshot().escalations, 1);
    }

    #[tokio::test]
    async fn hosted_failure_keeps_local_answer() {
        let (c, _, stats) = cascade(1.01, None);
        let got = c.classify(&input()).await.unwrap();
        assert_eq!(got.request_type, RequestType::CodeGeneration);
        let snap = stats.snapshot();
        assert_eq!((snap.escalations, snap.escalation_failures), (1, 1));
    }
}
