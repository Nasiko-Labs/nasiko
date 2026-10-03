//! Guarded request classification — the single entry point the router and the eval use.
//!
//! [`FallbackClassifier`] runs the configured [`RequestClassifier`] under a timeout and turns
//! every failure into the regex result, so a broken, slow, or unreachable backend degrades
//! routing to exactly what it was before backends existed — never to an outage. It also
//! applies the low-confidence rule: a backend answer below `min_confidence` is marked
//! [`DecisionSource::LowConfidence`] and the router serves the safe default for it.
//!
//! Every fallback and low-confidence decision is counted ([`ClassifierStats`]) and logged,
//! so the fallback rate is observable rather than silent.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::{Classification, ClassifyError, ClassifyInput, RegexClassifier, RequestClassifier};

/// How a [`Decision`] was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    /// The configured backend answered with enough confidence to route on.
    Backend,
    /// The backend answered, but below the confidence threshold. The classification is the
    /// backend's (kept for logging and eval), but the router must not route on it.
    LowConfidence,
    /// The backend failed or timed out; the classification is the regex result.
    Fallback,
}

/// A classification plus how it was obtained.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    /// The classification the router acts on (the regex result for a fallback).
    pub classification: Classification,
    /// Which path produced it.
    pub source: DecisionSource,
}

/// Point-in-time copy of the counters, for logs, tests, and eval reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// Every call to [`FallbackClassifier::decide`].
    pub decisions: u64,
    /// Backend errors and timeouts answered by regex.
    pub fallbacks: u64,
    /// Backend answers below the confidence threshold.
    pub low_confidence: u64,
}

/// Monotonic decision counters. Relaxed ordering: these are independent tallies read for
/// reporting, never used to synchronise anything.
#[derive(Debug, Default)]
pub struct ClassifierStats {
    decisions: AtomicU64,
    fallbacks: AtomicU64,
    low_confidence: AtomicU64,
}

impl ClassifierStats {
    /// Read all counters.
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            decisions: self.decisions.load(Ordering::Relaxed),
            fallbacks: self.fallbacks.load(Ordering::Relaxed),
            low_confidence: self.low_confidence.load(Ordering::Relaxed),
        }
    }
}

/// A [`RequestClassifier`] backend wrapped with a timeout, regex fallback, and the
/// low-confidence rule. Built once at startup and shared; cheap to call concurrently.
pub struct FallbackClassifier {
    backend: Arc<dyn RequestClassifier>,
    timeout: Duration,
    min_confidence: f32,
    stats: ClassifierStats,
}

impl FallbackClassifier {
    /// Wrap `backend`. Answers slower than `timeout` fall back to regex; answers with
    /// confidence below `min_confidence` are [`DecisionSource::LowConfidence`].
    pub fn new(
        backend: Arc<dyn RequestClassifier>,
        timeout: Duration,
        min_confidence: f32,
    ) -> Self {
        Self {
            backend,
            timeout,
            min_confidence,
            stats: ClassifierStats::default(),
        }
    }

    /// The out-of-the-box classifier: the regex baseline, which can neither fail nor be
    /// low-confidence (its fixed confidence is a placeholder, so the threshold is 0), so
    /// routing is identical to the pre-trait router.
    pub fn regex_only() -> Self {
        Self::new(Arc::new(RegexClassifier), Duration::MAX, 0.0)
    }

    /// Name of the wrapped backend.
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Current counter values.
    pub fn stats(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }

    /// Classify `input`. Always returns a decision: backend errors and timeouts become the
    /// regex result with [`DecisionSource::Fallback`].
    pub async fn decide(&self, input: &ClassifyInput<'_>) -> Decision {
        self.stats.decisions.fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(self.timeout, self.backend.classify(input)).await {
            Ok(Ok(classification)) => self.judge_confidence(classification),
            Ok(Err(err)) => self.fall_back(input, err),
            Err(_elapsed) => self.fall_back(input, ClassifyError::Timeout(self.timeout)),
        }
    }

    /// Apply the threshold. (The regex default is built by [`Self::regex_only`] with a zero
    /// threshold, so `CLASSIFIER_MIN_CONFIDENCE` never changes out-of-the-box routing.)
    fn judge_confidence(&self, classification: Classification) -> Decision {
        if classification.confidence >= self.min_confidence {
            return Decision {
                classification,
                source: DecisionSource::Backend,
            };
        }
        self.stats.low_confidence.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            target: "nasiko::llm_router::classifier",
            backend = %self.backend.name(),
            request_type = %classification.request_type.as_str(),
            confidence = classification.confidence,
            min_confidence = self.min_confidence,
            "classifier: low-confidence decision; router will serve the safe default"
        );
        Decision {
            classification,
            source: DecisionSource::LowConfidence,
        }
    }

    fn fall_back(&self, input: &ClassifyInput<'_>, err: ClassifyError) -> Decision {
        self.stats.fallbacks.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            backend = %self.backend.name(),
            error = %err,
            "classifier: backend failed; using the regex result"
        );
        Decision {
            classification: RegexClassifier::classify_now(input),
            source: DecisionSource::Fallback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::RequestType;
    use async_trait::async_trait;

    /// A backend that returns a fixed result (or error) after an optional delay.
    struct ScriptedBackend {
        result: fn() -> Result<Classification, ClassifyError>,
        delay: Duration,
    }

    #[async_trait]
    impl RequestClassifier for ScriptedBackend {
        fn name(&self) -> &str {
            "scripted"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(self.delay).await;
            (self.result)()
        }
    }

    fn guard(
        result: fn() -> Result<Classification, ClassifyError>,
        delay_ms: u64,
    ) -> FallbackClassifier {
        let backend = ScriptedBackend {
            result,
            delay: Duration::from_millis(delay_ms),
        };
        FallbackClassifier::new(Arc::new(backend), Duration::from_millis(50), 0.6)
    }

    fn confident_writing() -> Result<Classification, ClassifyError> {
        Classification::new(RequestType::Writing, 2, 0.9)
    }

    fn unsure_writing() -> Result<Classification, ClassifyError> {
        Classification::new(RequestType::Writing, 2, 0.3)
    }

    fn model_load_error() -> Result<Classification, ClassifyError> {
        Err(ClassifyError::ModelLoad("weights missing".into()))
    }

    /// "write a python function" is CodeGeneration to the regex but scripted as Writing,
    /// so the label tells us which path produced the decision.
    const QUERY: ClassifyInput<'static> = ClassifyInput {
        query: "write a python function that sorts a list",
        context: None,
    };

    #[tokio::test]
    async fn confident_backend_answer_is_used() {
        let g = guard(confident_writing, 0);
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::Backend);
        assert_eq!(d.classification.request_type, RequestType::Writing);
        assert_eq!(g.stats().fallbacks, 0);
    }

    #[tokio::test]
    async fn backend_error_falls_back_to_regex_and_is_counted() {
        let g = guard(model_load_error, 0);
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::Fallback);
        assert_eq!(d.classification, RegexClassifier::classify_now(&QUERY));
        assert_eq!(g.stats().fallbacks, 1);
    }

    #[tokio::test]
    async fn slow_backend_times_out_to_regex_and_is_counted() {
        let g = guard(confident_writing, 500);
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::Fallback);
        assert_eq!(d.classification.request_type, RequestType::CodeGeneration);
        assert_eq!(g.stats().fallbacks, 1);
    }

    #[tokio::test]
    async fn answer_below_threshold_is_low_confidence_not_fallback() {
        let g = guard(unsure_writing, 0);
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::LowConfidence);
        assert_eq!(d.classification.request_type, RequestType::Writing);
        let stats = g.stats();
        assert_eq!((stats.low_confidence, stats.fallbacks), (1, 0));
    }

    #[tokio::test]
    async fn regex_default_is_never_low_confidence() {
        let g = FallbackClassifier::regex_only();
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::Backend);
        assert_eq!(d.classification, RegexClassifier::classify_now(&QUERY));
    }

    fn invalid_output() -> Result<Classification, ClassifyError> {
        Err(ClassifyError::InvalidOutput("unknown label".into()))
    }

    #[tokio::test]
    async fn invalid_backend_output_falls_back_to_regex_and_is_counted() {
        let g = guard(invalid_output, 0);
        let d = g.decide(&QUERY).await;
        assert_eq!(d.source, DecisionSource::Fallback);
        assert_eq!(d.classification, RegexClassifier::classify_now(&QUERY));
        assert_eq!(g.stats().fallbacks, 1);
    }

    #[tokio::test]
    async fn invalid_backend_output_is_rejected_at_construction() {
        assert!(Classification::new(RequestType::General, 0, 0.5).is_err());
        assert!(Classification::new(RequestType::General, 6, 0.5).is_err());
        assert!(Classification::new(RequestType::General, 3, 1.5).is_err());
        assert!(Classification::new(RequestType::General, 3, f32::NAN).is_err());
    }
}
