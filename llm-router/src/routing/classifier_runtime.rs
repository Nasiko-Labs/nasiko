//! Shared timeout, validation and fallback policy for routing and evaluation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RegexClassifier, RequestClassifier,
    regex_classification,
};

/// Bounds hosted input size without splitting UTF-8. Regex keeps its original full input.
pub const MAX_QUERY_CHARS: usize = 16_384;
pub const MAX_CONTEXT_CHARS: usize = 8_192;

/// Why the returned classification is or isn't eligible for experimental tier selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierStatus {
    Regex,
    Classified,
    Error,
    Timeout,
    LowConfidence,
}

/// A fallback returns the regex label. Uncertainty keeps config; errors use regex routing.
#[derive(Debug, Clone)]
pub struct ClassifierDecision {
    pub classification: Classification,
    pub status: ClassifierStatus,
    /// Preserve an uncertain backend decision for diagnostics; never use it to pick a tier.
    pub candidate: Option<Classification>,
}

impl ClassifierDecision {
    pub fn is_fallback(&self) -> bool {
        matches!(
            self.status,
            ClassifierStatus::Error | ClassifierStatus::Timeout | ClassifierStatus::LowConfidence
        )
    }
}

/// Shared backend and policy, created once. Counters do not influence decisions.
pub struct ClassifierRuntime {
    backend: Arc<dyn RequestClassifier>,
    timeout: Duration,
    min_confidence: f32,
    min_complexity_confidence: f32,
    experimental: bool,
    pub seed: u64,
    decisions: AtomicU64,
    fallbacks: AtomicU64,
}

impl Default for ClassifierRuntime {
    fn default() -> Self {
        Self::regex()
    }
}

impl ClassifierRuntime {
    pub fn regex() -> Self {
        Self::new(Arc::new(RegexClassifier), Duration::from_secs(2), 0.0, 0)
    }

    /// A non-regex backend opts into deterministic, complexity-aware routing.
    pub fn new(
        backend: Arc<dyn RequestClassifier>,
        timeout: Duration,
        min_confidence: f32,
        seed: u64,
    ) -> Self {
        let experimental = backend.name() != "regex";
        Self {
            backend,
            timeout,
            min_confidence: if min_confidence.is_finite() {
                min_confidence.clamp(0.0, 1.0)
            } else {
                0.6
            },
            experimental,
            seed,
            min_complexity_confidence: 0.4,
            decisions: AtomicU64::new(0),
            fallbacks: AtomicU64::new(0),
        }
    }

    pub fn name(&self) -> &str {
        self.backend.name()
    }

    pub fn with_complexity_threshold(mut self, threshold: f32) -> Self {
        self.min_complexity_confidence = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            0.4
        };
        self
    }

    pub fn is_experimental(&self) -> bool {
        self.experimental
    }

    /// `(decisions, fallbacks)` since this runtime was constructed.
    pub fn counts(&self) -> (u64, u64) {
        (
            self.decisions.load(Ordering::Relaxed),
            self.fallbacks.load(Ordering::Relaxed),
        )
    }

    /// Routing and the eval both call this method, including all failure policies.
    pub async fn classify(&self, input: &ClassifyInput<'_>) -> ClassifierDecision {
        self.decisions.fetch_add(1, Ordering::Relaxed);
        let query = self.experimental.then(|| {
            input
                .query
                .chars()
                .take(MAX_QUERY_CHARS)
                .collect::<String>()
        });
        let context = self
            .experimental
            .then(|| {
                input
                    .context
                    .map(|text| text.chars().take(MAX_CONTEXT_CHARS).collect::<String>())
            })
            .flatten();
        let bounded = ClassifyInput {
            query: query.as_deref().unwrap_or(input.query),
            context: context.as_deref(),
        };
        let actual_input = if self.experimental { &bounded } else { input };
        let result = tokio::time::timeout(self.timeout, self.backend.classify(actual_input)).await;
        let (status, candidate) = match result {
            Ok(Ok(classification)) if valid(&classification) => {
                if !self.experimental
                    || (classification.confidence >= self.min_confidence
                        && classification
                            .complexity_confidence
                            .is_none_or(|confidence| confidence >= self.min_complexity_confidence))
                {
                    if self.experimental {
                        tracing::info!(
                            target: "nasiko::llm_router::request_classifier",
                            backend = self.name(),
                            request_type = classification.request_type.as_str(),
                            complexity = classification.complexity,
                            confidence = classification.confidence,
                            complexity_confidence = ?classification.complexity_confidence,
                            "request classification"
                        );
                    }
                    return ClassifierDecision {
                        classification,
                        candidate: None,
                        status: if self.experimental {
                            ClassifierStatus::Classified
                        } else {
                            ClassifierStatus::Regex
                        },
                    };
                }
                (ClassifierStatus::LowConfidence, Some(classification))
            }
            Ok(Err(ClassifyError::Timeout)) | Err(_) => (ClassifierStatus::Timeout, None),
            _ => (ClassifierStatus::Error, None),
        };
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "nasiko::llm_router::request_classifier",
            backend = self.name(),
            ?status,
            "request classifier fallback"
        );
        let mut classification = regex_classification(input.query);
        classification.usage = candidate.as_ref().and_then(|value| value.usage.clone());
        ClassifierDecision {
            classification,
            status,
            candidate,
        }
    }
}

fn valid(classification: &Classification) -> bool {
    (1..=5).contains(&classification.complexity)
        && probability(classification.confidence)
        && classification.complexity_confidence.is_none_or(probability)
}

fn probability(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::super::classifier::RequestType;
    use super::*;

    struct FakeBackend {
        result: Result<Classification, ClassifyError>,
        delay: Duration,
    }

    #[async_trait::async_trait]
    impl RequestClassifier for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(self.delay).await;
            self.result.clone()
        }
    }

    fn result(confidence: f32) -> Classification {
        Classification {
            request_type: RequestType::Writing,
            complexity: 2,
            confidence,
            complexity_confidence: Some(0.9),
            usage: None,
        }
    }

    #[tokio::test]
    async fn failures_uncertainty_and_timeout_return_regex_and_count_fallback() {
        let input = ClassifyInput {
            query: "write a Python function",
            context: None,
        };
        for (response, delay, status) in [
            (
                Err(ClassifyError::Transport),
                Duration::ZERO,
                ClassifierStatus::Error,
            ),
            (
                Ok(result(0.4)),
                Duration::ZERO,
                ClassifierStatus::LowConfidence,
            ),
            (
                Ok(result(f32::NAN)),
                Duration::ZERO,
                ClassifierStatus::Error,
            ),
            (
                Ok(result(0.9)),
                Duration::from_secs(1),
                ClassifierStatus::Timeout,
            ),
        ] {
            let runtime = ClassifierRuntime::new(
                Arc::new(FakeBackend {
                    result: response,
                    delay,
                }),
                Duration::from_millis(20),
                0.6,
                42,
            );
            let decision = runtime.classify(&input).await;
            assert_eq!(decision.status, status);
            assert_eq!(decision.classification, regex_classification(input.query));
            assert_eq!(runtime.counts(), (1, 1));
        }
    }

    #[tokio::test]
    async fn regex_ignores_context_and_is_not_a_low_confidence_fallback() {
        let runtime = ClassifierRuntime::regex();
        let decision = runtime
            .classify(&ClassifyInput {
                query: "hello",
                context: Some("implement a Rust function"),
            })
            .await;
        assert_eq!(decision.classification.request_type, RequestType::General);
        assert_eq!(decision.status, ClassifierStatus::Regex);
        assert_eq!(runtime.counts(), (1, 0));
    }
}
