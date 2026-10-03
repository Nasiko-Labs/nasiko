//! The one inference path every caller shares — router, eval example and UI preview.
//!
//! [`ClassifierService`] wraps the configured primary backend (a
//! [`RequestClassifier`]) with the policy the P2 brief requires and that no backend should
//! have to re-implement:
//!
//! - **Validation.** A primary answer outside the contract (complexity ∉ 1..=5,
//!   non-finite or out-of-range confidence) is an invalid output, not a verdict.
//! - **Timing.** One monotonic clock around preparation, the hosted call, validation and
//!   any fallback — the figure the eval writes as `latency_us`. One-time initialization is
//!   measured separately ([`ClassifierService::init_time`]).
//! - **Deadline.** The configured overall timeout bounds the whole attempt, including the
//!   backend's own retries.
//! - **Fallback.** Any backend failure — never initialized, timeout, network, upstream
//!   status, invalid body — is answered by the regex backend and counted under one
//!   [`FallbackReason`]. A routed request never fails because the classifier did.
//! - **Abstention.** A valid hosted answer whose request-type probability is below the
//!   configured floor is an *abstention*: the service still reports the hosted label and
//!   its (low) confidence so the caller can see what the backend thought, but flags it so
//!   the router routes to the safe default and credits nothing. Abstention is distinct from
//!   fallback in every diagnostic. The floor never applies to the regex backend, so default
//!   routing is unchanged by its existence.
//!
//! The regex backend is always present as the fallback; when it is also the configured
//! backend there is no primary and every call is a plain regex answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::classifier::{
    BackendDiagnostics, Classification, Classified, ClassifyError, ClassifyInput, RegexClassifier,
    RequestClassifier, warm_regex_tables,
};
use super::jev::JevClassifier;
use super::laya::LayaClassifier;
use crate::config::{ClassifierBackend, ClassifierConfig};

/// Longest query the hosted path sends, in chars. Jev's state budget is 32k tokens; the
/// cap keeps one request well inside it and bounds what a pathological prompt costs.
pub const MAX_QUERY_CHARS: usize = 6000;
/// Longest context the hosted path sends, in chars. Context is a distractor past a point
/// (see the Jev jaggedness notes), so it is bounded tighter than the query.
pub const MAX_CONTEXT_CHARS: usize = 3000;

/// Why the regex backend answered instead of the configured primary. One is counted per
/// classification that fell back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// The primary was never usable (missing key, bad endpoint/config).
    Init,
    /// The primary answered with a failure status or refused the request.
    Inference,
    /// The primary answered 2xx but the body violated the contract.
    InvalidOutput,
    /// The primary could not be reached.
    Network,
    /// The overall deadline expired.
    Timeout,
}

impl FallbackReason {
    pub const ALL: [FallbackReason; 5] = [
        FallbackReason::Init,
        FallbackReason::Inference,
        FallbackReason::InvalidOutput,
        FallbackReason::Network,
        FallbackReason::Timeout,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FallbackReason::Init => "init",
            FallbackReason::Inference => "inference",
            FallbackReason::InvalidOutput => "invalid_output",
            FallbackReason::Network => "network",
            FallbackReason::Timeout => "timeout",
        }
    }

    fn of(err: &ClassifyError) -> Self {
        match err {
            ClassifyError::Init(_) => FallbackReason::Init,
            ClassifyError::Timeout => FallbackReason::Timeout,
            ClassifyError::Network(_) => FallbackReason::Network,
            ClassifyError::Upstream { .. } => FallbackReason::Inference,
            ClassifyError::InvalidResponse(_) => FallbackReason::InvalidOutput,
        }
    }
}

/// How the served classification came about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Disposition {
    /// The regex backend is the configured backend; it answered directly.
    Regex,
    /// The primary (hosted) backend answered and its answer was accepted.
    Primary,
    /// The primary answered validly but below the confidence floor. The served label is
    /// still the primary's; the caller must treat it as "no classification".
    Abstained,
    /// The primary failed; the regex backend answered. The served label and confidence
    /// are the regex backend's — the hosted probability is never attached to them.
    Fallback {
        reason: FallbackReason,
        /// Log-safe detail (status code, validation message). Never request text or keys.
        detail: String,
    },
}

/// One classification as served, with everything a caller might want to show or log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassifyOutcome {
    /// The verdict actually served (regex's on fallback, the primary's otherwise).
    pub classification: Classification,
    /// The backend whose label `classification` carries.
    pub answered_by: String,
    pub disposition: Disposition,
    /// Wall-clock for the whole call: preparation, hosted round trip, validation, fallback.
    pub latency: Duration,
    /// The primary backend's diagnostics when it produced an answer (accepted or
    /// abstained). Absent for regex answers and for failed primary calls.
    pub diagnostics: Option<BackendDiagnostics>,
    /// Whether the hosted input was truncated to the size caps.
    pub input_truncated: bool,
}

impl ClassifyOutcome {
    /// `true` when the served label should drive routing as a real classification —
    /// everything except an abstention.
    pub fn is_actionable(&self) -> bool {
        self.disposition != Disposition::Abstained
    }

    pub fn fallback_reason(&self) -> Option<FallbackReason> {
        match &self.disposition {
            Disposition::Fallback { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

/// Monotonic counters for one service instance; read via [`ClassifierService::stats`].
#[derive(Default)]
struct Counters {
    calls: AtomicU64,
    primary_ok: AtomicU64,
    abstained: AtomicU64,
    fallback: [AtomicU64; 5],
}

/// A point-in-time copy of the counters.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClassifierStats {
    pub calls: u64,
    /// Accepted primary answers.
    pub primary_ok: u64,
    /// Valid primary answers below the confidence floor.
    pub abstained: u64,
    /// Regex answers caused by a primary failure, by reason.
    pub fallbacks: Vec<(FallbackReason, u64)>,
}

impl ClassifierStats {
    pub fn fallback_total(&self) -> u64 {
        self.fallbacks.iter().map(|(_, n)| n).sum()
    }
}

/// Which backend the service was asked for and whether that backend is usable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendStatus {
    /// What configuration asked for.
    pub configured: ClassifierBackend,
    /// Backend label that will answer when nothing fails.
    pub effective: String,
    /// `Some(reason)` when the configured backend could not be initialized; every call is
    /// then a counted `init` fallback to regex.
    pub init_error: Option<String>,
    /// Hosted model id requested (not the version that answered — that is per call).
    pub model: Option<String>,
    /// Hosted endpoint in use (no credentials).
    pub endpoint: Option<String>,
    /// Local model directory (Laya), whether or not it loaded.
    pub model_path: Option<String>,
    pub timeout_ms: u64,
    pub min_confidence: f32,
    pub routing_seed: Option<u64>,
}

/// See the module docs.
pub struct ClassifierService {
    configured: ClassifierBackend,
    primary: Option<Arc<dyn RequestClassifier>>,
    init_error: Option<String>,
    regex: RegexClassifier,
    timeout: Duration,
    min_confidence: f32,
    routing_seed: Option<u64>,
    model: Option<String>,
    endpoint: Option<String>,
    /// Laya: the bundle directory (configured, whether or not it loaded).
    model_path: Option<String>,
    init_time: Duration,
    counters: Counters,
}

impl std::fmt::Debug for ClassifierService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassifierService")
            .field("configured", &self.configured)
            .field(
                "primary",
                &self.primary.as_ref().map(|p| p.name().to_string()),
            )
            .field("init_error", &self.init_error)
            .field("timeout", &self.timeout)
            .field("min_confidence", &self.min_confidence)
            .field("routing_seed", &self.routing_seed)
            .finish()
    }
}

impl ClassifierService {
    /// Build the service from typed config. Never fails: a backend that cannot be
    /// initialized is recorded as `init_error` and every call falls back to regex (counted),
    /// so a misconfigured experiment degrades to today's behaviour instead of an outage.
    /// Does no network I/O: with the default config nothing is contacted or downloaded.
    pub fn from_config(cfg: &ClassifierConfig) -> Self {
        let started = Instant::now();
        warm_regex_tables();
        type Built = (
            Option<Arc<dyn RequestClassifier>>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let (primary, init_error, model_override, model_path): Built = match cfg.backend {
            ClassifierBackend::Regex => (None, None, None, None),
            ClassifierBackend::Jev => match JevClassifier::new(cfg) {
                Ok(jev) => (Some(Arc::new(jev)), None, None, None),
                Err(e) => (None, Some(e.to_string()), None, None),
            },
            // The local model is loaded here and only here: with any other backend the
            // bundle is never opened and the runtime is never dynamically loaded.
            ClassifierBackend::Laya => match LayaClassifier::new(cfg) {
                Ok(laya) => {
                    let version = laya.info().model_version.clone();
                    let model_dir = laya.info().model_dir.display().to_string();
                    let c: Arc<dyn RequestClassifier> = Arc::new(laya);
                    (Some(c), None, Some(version), Some(model_dir))
                }
                Err(e) => (
                    None,
                    Some(e.to_string()),
                    None,
                    Some(cfg.model_path.clone()),
                ),
            },
        };
        let hosted = cfg.backend == ClassifierBackend::Jev;
        let service = Self {
            configured: cfg.backend,
            primary,
            init_error,
            regex: RegexClassifier,
            timeout: Duration::from_millis(cfg.timeout_ms.max(1)),
            min_confidence: cfg.min_confidence.clamp(0.0, 1.0),
            routing_seed: cfg.routing_seed,
            model: model_override.or_else(|| hosted.then(|| cfg.model.clone())),
            endpoint: hosted.then(|| cfg.endpoint.clone()),
            model_path,
            init_time: Duration::ZERO,
            counters: Counters::default(),
        };
        let service = Self {
            init_time: started.elapsed(),
            ..service
        };
        match (&service.primary, &service.init_error) {
            (Some(p), _) => tracing::info!(
                target: "nasiko::llm_router::classifier",
                backend = p.name(), model = ?service.model, endpoint = ?service.endpoint,
                timeout_ms = cfg.timeout_ms, min_confidence = service.min_confidence,
                routing_seed_set = service.routing_seed.is_some(),
                "classifier service: hosted backend configured (regex fallback on any failure)"
            ),
            (None, Some(e)) => tracing::warn!(
                target: "nasiko::llm_router::classifier",
                configured = cfg.backend.as_str(), error = %e,
                "classifier service: configured backend failed to initialize; every call will fall back to regex"
            ),
            (None, None) => tracing::info!(
                target: "nasiko::llm_router::classifier",
                "classifier service: backend = regex (default; no network)"
            ),
        }
        service
    }

    /// Build around an explicit primary — the test seam (fake backends) and the way a
    /// future local backend plugs in without touching config parsing.
    pub fn with_primary(
        configured: ClassifierBackend,
        primary: Option<Arc<dyn RequestClassifier>>,
        timeout: Duration,
        min_confidence: f32,
        routing_seed: Option<u64>,
    ) -> Self {
        warm_regex_tables();
        Self {
            configured,
            primary,
            init_error: None,
            regex: RegexClassifier,
            timeout,
            min_confidence: min_confidence.clamp(0.0, 1.0),
            routing_seed,
            model: None,
            endpoint: None,
            model_path: None,
            init_time: Duration::ZERO,
            counters: Counters::default(),
        }
    }

    /// A regex-only service — what the router runs by default and what tests use when the
    /// classifier is not the thing under test.
    pub fn regex_only() -> Self {
        Self::with_primary(
            ClassifierBackend::Regex,
            None,
            Duration::from_secs(1),
            0.0,
            None,
        )
    }

    /// Mark the configured backend as unusable; every call becomes an `init` fallback.
    /// Used by [`from_config`](Self::from_config) and tests.
    pub fn with_init_error(mut self, error: impl Into<String>) -> Self {
        self.primary = None;
        self.init_error = Some(error.into());
        self
    }

    pub fn status(&self) -> BackendStatus {
        BackendStatus {
            configured: self.configured,
            effective: match (&self.primary, &self.init_error) {
                (Some(p), _) => p.name().to_string(),
                _ => "regex".to_string(),
            },
            init_error: self.init_error.clone(),
            model: self.model.clone(),
            endpoint: self.endpoint.clone(),
            model_path: self.model_path.clone(),
            timeout_ms: self.timeout.as_millis() as u64,
            min_confidence: self.min_confidence,
            routing_seed: self.routing_seed,
        }
    }

    /// The seed that makes tier sampling repeatable, if configured.
    pub fn routing_seed(&self) -> Option<u64> {
        self.routing_seed
    }

    /// Whether a hosted backend is configured (usable or not).
    pub fn is_hosted(&self) -> bool {
        self.configured != ClassifierBackend::Regex
    }

    /// Time spent building the service (regex table compile, HTTP client, key checks).
    pub fn init_time(&self) -> Duration {
        self.init_time
    }

    pub fn stats(&self) -> ClassifierStats {
        ClassifierStats {
            calls: self.counters.calls.load(Ordering::Relaxed),
            primary_ok: self.counters.primary_ok.load(Ordering::Relaxed),
            abstained: self.counters.abstained.load(Ordering::Relaxed),
            fallbacks: FallbackReason::ALL
                .iter()
                .map(|r| {
                    (
                        *r,
                        self.counters.fallback[*r as usize].load(Ordering::Relaxed),
                    )
                })
                .collect(),
        }
    }

    /// Classify one request through the shared policy. Never errors.
    pub async fn classify(&self, input: &ClassifyInput<'_>) -> ClassifyOutcome {
        let started = Instant::now();
        self.counters.calls.fetch_add(1, Ordering::Relaxed);

        // Regex is the configured backend: answer directly, no gate, no truncation — the
        // legacy classifier saw the raw query and still does.
        let primary = match (&self.primary, &self.init_error) {
            (Some(p), _) => p,
            (None, Some(err)) => {
                return self.fallback(input, FallbackReason::Init, err.clone(), started, false);
            }
            (None, None) => {
                return ClassifyOutcome {
                    classification: self.regex.classify_sync(input),
                    answered_by: "regex".into(),
                    disposition: Disposition::Regex,
                    latency: started.elapsed(),
                    diagnostics: None,
                    input_truncated: false,
                };
            }
        };

        // Hosted path: bound and normalize what leaves the process.
        let normalized = normalize_input(input);
        let truncated = normalized.truncated;
        if normalized.query.is_empty() {
            // Nothing to classify; the hosted call would be billed for a 422. Regex's
            // default verdict (General) is the honest answer, and it is not a failure.
            return ClassifyOutcome {
                classification: self.regex.classify_sync(input),
                answered_by: "regex".into(),
                disposition: Disposition::Regex,
                latency: started.elapsed(),
                diagnostics: None,
                input_truncated: truncated,
            };
        }
        let hosted_input = ClassifyInput {
            query: &normalized.query,
            context: normalized.context.as_deref(),
        };

        let result = match tokio::time::timeout(
            self.timeout + TIMEOUT_SLACK,
            primary.classify_detailed(&hosted_input),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(ClassifyError::Timeout),
        };

        match result {
            Ok(Classified {
                classification,
                diagnostics,
            }) => {
                if let Err(e) = classification.validate() {
                    return self.fallback(
                        input,
                        FallbackReason::InvalidOutput,
                        e.to_string(),
                        started,
                        truncated,
                    );
                }
                let truncated =
                    truncated || diagnostics.as_ref().is_some_and(|d| d.input_truncated);
                let abstained =
                    self.min_confidence > 0.0 && classification.confidence < self.min_confidence;
                if abstained {
                    self.counters.abstained.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.counters.primary_ok.fetch_add(1, Ordering::Relaxed);
                }
                let latency = started.elapsed();
                tracing::info!(
                    target: "nasiko::llm_router::classifier",
                    backend = primary.name(),
                    request_type = classification.request_type.as_str(),
                    complexity = classification.complexity,
                    confidence = classification.confidence,
                    abstained,
                    latency_us = latency.as_micros() as u64,
                    input_truncated = truncated,
                    "classifier service: hosted backend answered"
                );
                ClassifyOutcome {
                    classification,
                    answered_by: primary.name().to_string(),
                    disposition: if abstained {
                        Disposition::Abstained
                    } else {
                        Disposition::Primary
                    },
                    latency,
                    diagnostics,
                    input_truncated: truncated,
                }
            }
            Err(e) => {
                let reason = FallbackReason::of(&e);
                self.fallback(input, reason, e.to_string(), started, truncated)
            }
        }
    }

    fn fallback(
        &self,
        input: &ClassifyInput<'_>,
        reason: FallbackReason,
        detail: String,
        started: Instant,
        truncated: bool,
    ) -> ClassifyOutcome {
        self.counters.fallback[reason as usize].fetch_add(1, Ordering::Relaxed);
        let classification = self.regex.classify_sync(input);
        let latency = started.elapsed();
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            configured = self.configured.as_str(),
            reason = reason.as_str(),
            detail = %detail,
            request_type = classification.request_type.as_str(),
            latency_us = latency.as_micros() as u64,
            "classifier service: primary backend failed; regex fallback answered"
        );
        ClassifyOutcome {
            classification,
            answered_by: "regex".into(),
            disposition: Disposition::Fallback { reason, detail },
            latency,
            diagnostics: None,
            input_truncated: truncated,
        }
    }
}

/// Grace added to the backend's own deadline so the backend's more specific error (which
/// knows whether it was mid-connect or mid-retry) normally wins the race against the
/// service's blunt wall-clock cutoff.
const TIMEOUT_SLACK: Duration = Duration::from_millis(250);

/// The hosted path's view of the input: trimmed and capped. Truncation cuts at a char
/// boundary after the cap, deterministically, and is reported so a run can tell how often
/// it happened. Never applied to the regex path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedInput {
    pub query: String,
    pub context: Option<String>,
    pub truncated: bool,
}

pub fn normalize_input(input: &ClassifyInput<'_>) -> NormalizedInput {
    let mut truncated = false;
    let query = cap_chars(input.query.trim(), MAX_QUERY_CHARS, &mut truncated);
    let context = input
        .context
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| cap_chars(c, MAX_CONTEXT_CHARS, &mut truncated));
    NormalizedInput {
        query,
        context,
        truncated,
    }
}

/// Take at most `max` chars (not bytes), so a multi-byte sequence is never split.
pub fn cap_chars(s: &str, max: usize, truncated: &mut bool) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => {
            *truncated = true;
            s[..idx].to_string()
        }
        None => s.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Deterministic fake backends for router/service tests.
    use super::*;
    use std::sync::Mutex;

    /// A backend scripted with a fixed answer or error, that counts its calls and records
    /// the exact inputs it was given.
    pub struct FakeClassifier {
        pub name: &'static str,
        pub answer: Result<Classified, ClassifyError>,
        pub calls: AtomicU64,
        pub seen: Mutex<Vec<(String, Option<String>)>>,
        /// Optional delay to exercise the deadline.
        pub delay: Duration,
    }

    impl FakeClassifier {
        pub fn answering(c: Classification) -> Self {
            Self {
                name: "fake",
                answer: Ok(Classified {
                    classification: c,
                    diagnostics: None,
                }),
                calls: AtomicU64::new(0),
                seen: Mutex::new(vec![]),
                delay: Duration::ZERO,
            }
        }
        pub fn failing(e: ClassifyError) -> Self {
            Self {
                name: "fake",
                answer: Err(e),
                calls: AtomicU64::new(0),
                seen: Mutex::new(vec![]),
                delay: Duration::ZERO,
            }
        }
        pub fn calls(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl RequestClassifier for FakeClassifier {
        fn name(&self) -> &str {
            self.name
        }
        async fn classify(
            &self,
            input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            self.classify_detailed(input)
                .await
                .map(|c| c.classification)
        }
        async fn classify_detailed(
            &self,
            input: &ClassifyInput<'_>,
        ) -> Result<Classified, ClassifyError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen
                .lock()
                .unwrap()
                .push((input.query.to_string(), input.context.map(str::to_string)));
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.answer.clone()
        }
    }

    pub fn service_with(
        fake: Arc<FakeClassifier>,
        min_confidence: f32,
        timeout: Duration,
    ) -> ClassifierService {
        ClassifierService::with_primary(
            ClassifierBackend::Jev,
            Some(fake),
            timeout,
            min_confidence,
            None,
        )
    }

    pub fn code_gen(confidence: f32) -> Classification {
        Classification {
            request_type: super::super::classifier::RequestType::CodeGeneration,
            complexity: 4,
            confidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::routing::classifier::{REGEX_COMPLEXITY, REGEX_CONFIDENCE_MATCHED, RequestType};

    const Q: &str = "draft an email to my team about the outage";
    fn input() -> ClassifyInput<'static> {
        ClassifyInput {
            query: Q,
            context: Some("ctx"),
        }
    }

    #[tokio::test]
    async fn regex_only_service_answers_directly_without_gate() {
        // Default config ⇒ regex backend, no primary. The confidence floor is irrelevant.
        let s = ClassifierService::with_primary(
            ClassifierBackend::Regex,
            None,
            Duration::from_secs(1),
            0.99,
            None,
        );
        let out = s.classify(&input()).await;
        assert_eq!(out.disposition, Disposition::Regex);
        assert_eq!(out.answered_by, "regex");
        assert_eq!(out.classification.request_type, RequestType::Writing);
        assert_eq!(out.classification.confidence, REGEX_CONFIDENCE_MATCHED);
        assert!(out.is_actionable());
        let st = s.stats();
        assert_eq!(st.calls, 1);
        assert_eq!(st.fallback_total(), 0);
        assert_eq!(st.abstained, 0);
    }

    #[tokio::test]
    async fn primary_success_is_served_with_its_own_label_and_confidence() {
        let fake = Arc::new(FakeClassifier::answering(code_gen(0.91)));
        let s = service_with(fake.clone(), 0.0, Duration::from_secs(1));
        let out = s.classify(&input()).await;
        assert_eq!(out.disposition, Disposition::Primary);
        assert_eq!(out.answered_by, "fake");
        assert_eq!(out.classification, code_gen(0.91));
        assert_eq!(fake.calls(), 1);
        // The backend saw the normalized query and the context, not the labels.
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0], (Q.to_string(), Some("ctx".to_string())));
        assert_eq!(s.stats().primary_ok, 1);
    }

    #[tokio::test]
    async fn low_confidence_is_an_abstention_not_a_fallback() {
        let fake = Arc::new(FakeClassifier::answering(code_gen(0.2)));
        let s = service_with(fake, 0.4, Duration::from_secs(1));
        let out = s.classify(&input()).await;
        assert_eq!(out.disposition, Disposition::Abstained);
        assert!(!out.is_actionable());
        // The hosted label and its low confidence are reported together — never regex's
        // label with the hosted confidence or vice versa.
        assert_eq!(out.answered_by, "fake");
        assert_eq!(out.classification.request_type, RequestType::CodeGeneration);
        assert_eq!(out.classification.confidence, 0.2);
        let st = s.stats();
        assert_eq!(st.abstained, 1);
        assert_eq!(st.primary_ok, 0);
        assert_eq!(st.fallback_total(), 0);
    }

    #[tokio::test]
    async fn exactly_at_the_floor_is_not_an_abstention() {
        let fake = Arc::new(FakeClassifier::answering(code_gen(0.4)));
        let s = service_with(fake, 0.4, Duration::from_secs(1));
        assert_eq!(s.classify(&input()).await.disposition, Disposition::Primary);
    }

    #[tokio::test]
    async fn every_error_kind_falls_back_to_regex_and_is_counted_once() {
        let cases = [
            (ClassifyError::Init("no key".into()), FallbackReason::Init),
            (ClassifyError::Timeout, FallbackReason::Timeout),
            (
                ClassifyError::Network("dns".into()),
                FallbackReason::Network,
            ),
            (
                ClassifyError::Upstream {
                    status: 429,
                    detail: "rate limited".into(),
                },
                FallbackReason::Inference,
            ),
            (
                ClassifyError::InvalidResponse("bad json".into()),
                FallbackReason::InvalidOutput,
            ),
        ];
        for (err, expected) in cases {
            let fake = Arc::new(FakeClassifier::failing(err.clone()));
            let s = service_with(fake, 0.9, Duration::from_secs(1));
            let out = s.classify(&input()).await;
            assert_eq!(out.fallback_reason(), Some(expected), "{err:?}");
            assert_eq!(out.answered_by, "regex");
            // Regex's label and regex's confidence, and the floor does not apply to it.
            assert_eq!(out.classification.request_type, RequestType::Writing);
            assert_eq!(out.classification.confidence, REGEX_CONFIDENCE_MATCHED);
            assert_eq!(out.classification.complexity, REGEX_COMPLEXITY);
            assert!(out.is_actionable());
            assert!(out.diagnostics.is_none());
            let st = s.stats();
            assert_eq!(st.fallback_total(), 1, "{err:?}");
            assert_eq!(
                st.fallbacks.iter().find(|(r, _)| *r == expected).unwrap().1,
                1
            );
            // The detail is log-safe: it carries the error, not the query.
            if let Disposition::Fallback { detail, .. } = out.disposition {
                assert!(!detail.contains(Q));
            }
        }
    }

    #[tokio::test]
    async fn invalid_primary_output_is_rejected_before_it_can_route() {
        let bad = Classification {
            request_type: RequestType::Writing,
            complexity: 9,
            confidence: 0.99,
        };
        let fake = Arc::new(FakeClassifier::answering(bad));
        let s = service_with(fake, 0.0, Duration::from_secs(1));
        let out = s.classify(&input()).await;
        assert_eq!(out.fallback_reason(), Some(FallbackReason::InvalidOutput));
        assert_eq!(out.classification.complexity, REGEX_COMPLEXITY);
    }

    #[tokio::test]
    async fn slow_primary_hits_the_deadline_and_falls_back() {
        let mut fake = FakeClassifier::answering(code_gen(0.9));
        fake.delay = Duration::from_millis(600);
        let s = service_with(Arc::new(fake), 0.0, Duration::from_millis(50));
        let started = Instant::now();
        let out = s.classify(&input()).await;
        assert_eq!(out.fallback_reason(), Some(FallbackReason::Timeout));
        assert!(
            started.elapsed() < Duration::from_millis(550),
            "deadline not enforced"
        );
        assert!(out.latency >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn init_failure_counts_an_init_fallback_per_call_and_never_calls_primary() {
        let s = ClassifierService::regex_only().with_init_error("TYPESAFE_API_KEY unset");
        let status = s.status();
        assert_eq!(status.init_error.as_deref(), Some("TYPESAFE_API_KEY unset"));
        assert_eq!(status.effective, "regex");
        for _ in 0..3 {
            let out = s.classify(&input()).await;
            assert_eq!(out.fallback_reason(), Some(FallbackReason::Init));
        }
        assert_eq!(s.stats().fallback_total(), 3);
    }

    #[tokio::test]
    async fn empty_query_skips_the_hosted_call() {
        let fake = Arc::new(FakeClassifier::answering(code_gen(0.9)));
        let s = service_with(fake.clone(), 0.0, Duration::from_secs(1));
        let out = s
            .classify(&ClassifyInput {
                query: "   \n\t ",
                context: None,
            })
            .await;
        assert_eq!(out.disposition, Disposition::Regex);
        assert_eq!(out.classification.request_type, RequestType::General);
        assert_eq!(fake.calls(), 0);
    }

    #[tokio::test]
    async fn hosted_input_is_capped_but_regex_sees_the_raw_query() {
        let long_query = "x".repeat(MAX_QUERY_CHARS + 500);
        let long_ctx = "é".repeat(MAX_CONTEXT_CHARS + 10); // multi-byte: cap by chars
        let fake = Arc::new(FakeClassifier::answering(code_gen(0.9)));
        let s = service_with(fake.clone(), 0.0, Duration::from_secs(1));
        let out = s
            .classify(&ClassifyInput {
                query: &long_query,
                context: Some(&long_ctx),
            })
            .await;
        assert!(out.input_truncated);
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].0.chars().count(), MAX_QUERY_CHARS);
        assert_eq!(
            seen[0].1.as_ref().unwrap().chars().count(),
            MAX_CONTEXT_CHARS
        );
    }

    #[test]
    fn normalization_is_deterministic_and_unicode_safe() {
        let a = normalize_input(&ClassifyInput {
            query: "  héllo 🌍  ",
            context: Some("  "),
        });
        let b = normalize_input(&ClassifyInput {
            query: "  héllo 🌍  ",
            context: Some("  "),
        });
        assert_eq!(a, b);
        assert_eq!(a.query, "héllo 🌍");
        assert_eq!(a.context, None);
        assert!(!a.truncated);
        let mut t = false;
        assert_eq!(cap_chars("🌍🌍🌍", 2, &mut t), "🌍🌍");
        assert!(t);
    }

    #[test]
    fn from_config_with_defaults_is_regex_and_does_not_need_a_key() {
        let s = ClassifierService::from_config(&ClassifierConfig::default());
        let st = s.status();
        assert_eq!(st.configured, ClassifierBackend::Regex);
        assert_eq!(st.effective, "regex");
        assert!(st.init_error.is_none());
        assert!(st.model.is_none());
        assert!(!s.is_hosted());
    }

    #[test]
    fn from_config_jev_without_key_is_an_init_error_not_a_panic() {
        let cfg = ClassifierConfig {
            backend: ClassifierBackend::Jev,
            ..Default::default()
        };
        let s = ClassifierService::from_config(&cfg);
        let st = s.status();
        assert_eq!(st.configured, ClassifierBackend::Jev);
        assert_eq!(st.effective, "regex");
        assert!(
            st.init_error
                .as_deref()
                .unwrap()
                .contains("TYPESAFE_API_KEY")
        );
        assert!(s.is_hosted());
        // Debug output of the service never carries a key.
        assert!(!format!("{s:?}").contains("sk-"));
    }
}
