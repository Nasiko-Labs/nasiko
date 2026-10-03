//! Local-first cascade: successful decisions use local complexity, and only uncertain
//! type decisions pay for Strands. The 0.70 policy is fixed, not fit on the holdout.
//! Remote failures retain the P2 regex fallback contract.
use super::{Classification, ClassifierInput, RequestClassifier};
use async_trait::async_trait;
use std::sync::Arc;

pub struct HybridClassifier {
    local: Arc<dyn RequestClassifier>,
    remote: Arc<dyn RequestClassifier>,
}
impl HybridClassifier {
    pub fn new(
        local: impl RequestClassifier + 'static,
        remote: impl RequestClassifier + 'static,
    ) -> Self {
        Self {
            local: Arc::new(local),
            remote: Arc::new(remote),
        }
    }
}
#[async_trait]
impl RequestClassifier for HybridClassifier {
    async fn classify(&self, input: &ClassifierInput<'_>) -> Classification {
        let mut local = self.local.classify(input).await;
        if local.confidence >= 0.70 {
            return local;
        }
        let remote = self.remote.classify(input).await;
        if remote.source.fallback_reason().is_some() {
            return remote;
        }
        local.request_type = remote.request_type;
        local.type_probabilities = remote.type_probabilities;
        local.confidence = remote.confidence;
        local.source = super::request_classifier::ClassifierSource::HybridStrands;
        local
    }
}

#[cfg(test)]
mod tests {
    use super::super::request_classifier::ClassifierSource;
    use super::super::{RegexClassifier, RequestType};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Stub(Classification, Arc<AtomicUsize>);
    #[async_trait]
    impl RequestClassifier for Stub {
        async fn classify(&self, _: &ClassifierInput<'_>) -> Classification {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.clone()
        }
    }
    #[tokio::test]
    async fn cascade_skips_confident_types_preserves_complexity_and_falls_back_to_regex() {
        for (confidence, failure) in [(0.9, false), (0.4, false), (0.4, true)] {
            let mut local = RegexClassifier::classify_query("hello");
            local.confidence = confidence;
            local.complexity = 5;
            local.complexity_level = 3.7;
            local.complexity_confidence = 0.81;
            local.source = ClassifierSource::Local;
            let mut remote = RegexClassifier::classify_query("hello");
            remote.request_type = RequestType::Writing;
            remote.source = if failure {
                ClassifierSource::Fallback("timeout")
            } else {
                ClassifierSource::Strands
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let hybrid = HybridClassifier::new(
                Stub(local.clone(), Arc::new(AtomicUsize::new(0))),
                Stub(remote, calls.clone()),
            );
            let result = hybrid
                .classify(&ClassifierInput {
                    query: "hello",
                    state: "hello",
                })
                .await;
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(confidence < 0.70));
            if !failure {
                assert_eq!(
                    (
                        result.complexity,
                        result.complexity_level,
                        result.complexity_confidence
                    ),
                    (5, 3.7, 0.81)
                );
            }
            if confidence >= 0.70 {
                assert_eq!(result, local);
            } else if failure {
                assert_eq!(result.request_type, RequestType::Writing);
                assert_eq!(result.complexity, 3);
                assert_eq!(result.source.fallback_reason(), Some("timeout"));
            } else {
                assert_eq!(result.request_type, RequestType::Writing);
                assert_eq!(result.source, ClassifierSource::HybridStrands);
            }
        }
    }
}
