//! Model-agnostic request classifier and model-tier selector.
//!
//! The classifier has two responsibilities:
//! 1. classify the incoming request into one of the public request types;
//! 2. select a model-strength tier for the already-resolved destination provider.
//!
//! The default backend is deterministic regex classification. An optional HTTP/model
//! backend implements the same `RequestClassifier` trait. Any backend failure,
//! timeout, malformed response, or low-confidence response falls back to regex.
//!
//! Provider selection remains outside this module. The classifier only chooses
//! request type and model strength; the registry maps `(provider, tier)` to a
//! concrete model.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rand::Rng;
use rand_distr::{Beta, Distribution};
use serde_json::Value;
use tokio::time::timeout;

use super::patterns::{CATEGORY_PATTERNS, NEGATIVE_SIGNALS, POSITIVE_SIGNALS};

// ============================================================================
// Public request/tier types
// ============================================================================

/// Coarse model strength tier.
///
/// Tier 1 is the strongest model, Tier 3 is the smallest/cheapest model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Complex queries — strongest model.
    Tier1,
    /// Medium-complexity queries.
    Tier2,
    /// Simple queries — smallest/cheapest model.
    Tier3,
}

/// Public request categories used by the classifier evaluation set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestType {
    CodeGeneration,
    CodeUnderstanding,
    TechnicalDesign,
    AnalyticalReasoning,
    Writing,
    FactualLookup,
    General,
}

impl RequestType {
    /// Stable public/wire representation.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestType::CodeGeneration => "code_generation",
            RequestType::CodeUnderstanding => "code_understanding",
            RequestType::TechnicalDesign => "technical_design",
            RequestType::AnalyticalReasoning => "analytical_reasoning",
            RequestType::Writing => "writing",
            RequestType::FactualLookup => "factual_lookup",
            RequestType::General => "general",
        }
    }

    /// Parse the public wire representation.
    pub fn from_wire(s: &str) -> Option<RequestType> {
        Some(match s {
            "code_generation" => RequestType::CodeGeneration,
            "code_understanding" => RequestType::CodeUnderstanding,
            "technical_design" => RequestType::TechnicalDesign,
            "analytical_reasoning" => RequestType::AnalyticalReasoning,
            "writing" => RequestType::Writing,
            "factual_lookup" => RequestType::FactualLookup,
            "general" => RequestType::General,
            _ => return None,
        })
    }
}

// ============================================================================
// Existing learning/scoring structures
// ============================================================================

/// One learned quality estimate for a `(tier, request_type)` pair.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub quality_mean: f64,
    pub samples: i64,
}

/// Learned cells for a single provider.
pub type CellMap = HashMap<(Tier, RequestType), Cell>;

/// Maximum effective sample count.
pub const MAX_SAMPLES: i64 = 200;

/// Thompson quality/cost weights.
pub const DEFAULT_W_QUALITY: f64 = 0.7;
pub const DEFAULT_W_COST: f64 = 0.3;

const PRIOR_PSEUDO_COUNT: f64 = 4.0;

struct TierArm {
    tier: Tier,
    quality_tier: i32,
    cost: f64,
    strengths: &'static [RequestType],
}

const TIER_ARMS: [TierArm; 3] = [
    TierArm {
        tier: Tier::Tier1,
        quality_tier: 3,
        cost: 15.0,
        strengths: &[
            RequestType::CodeGeneration,
            RequestType::AnalyticalReasoning,
            RequestType::TechnicalDesign,
        ],
    },
    TierArm {
        tier: Tier::Tier2,
        quality_tier: 2,
        cost: 3.0,
        strengths: &[
            RequestType::CodeUnderstanding,
            RequestType::Writing,
        ],
    },
    TierArm {
        tier: Tier::Tier3,
        quality_tier: 1,
        cost: 0.8,
        strengths: &[
            RequestType::FactualLookup,
            RequestType::General,
        ],
    },
];

// ============================================================================
// P2 typed classifier interface
// ============================================================================

/// Input passed to every classifier backend.
#[derive(Debug, Clone)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// Typed model-agnostic classifier result.
///
/// `complexity` is on a 1–5 scale.
/// `confidence` is normalized to `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

/// Errors produced by a classifier backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifyError {
    Timeout,
    EndpointMissing,
    InvalidEndpoint,
    Http(String),
    InvalidResponse(String),
    InvalidRequestType(String),
    InvalidComplexity(u8),
    InvalidConfidence,
    LowConfidence(f32),
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassifyError::Timeout => write!(f, "classifier timeout"),
            ClassifyError::EndpointMissing => write!(f, "classifier endpoint is missing"),
            ClassifyError::InvalidEndpoint => write!(f, "classifier endpoint is invalid"),
            ClassifyError::Http(message) => write!(f, "classifier HTTP error: {message}"),
            ClassifyError::InvalidResponse(message) => {
                write!(f, "invalid classifier response: {message}")
            }
            ClassifyError::InvalidRequestType(value) => {
                write!(f, "invalid request_type: {value}")
            }
            ClassifyError::InvalidComplexity(value) => {
                write!(f, "invalid complexity: {value}")
            }
            ClassifyError::InvalidConfidence => {
                write!(f, "invalid classifier confidence")
            }
            ClassifyError::LowConfidence(value) => {
                write!(f, "classifier confidence {value:.3} is below threshold")
            }
        }
    }
}

impl std::error::Error for ClassifyError {}

/// Pluggable request-classification backend.
///
/// The same interface is used by routing and evaluation.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    async fn classify(
        &self,
        input: ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError>;

    /// Human-readable backend name for logs/evaluation.
    fn name(&self) -> &'static str;
}

// ============================================================================
// Classifier configuration
// ============================================================================

/// Configuration supplied by the standalone binary.
///
/// Environment variables are deliberately NOT read here. The binary owns
/// environment/configuration; the library receives an already-created value.
#[derive(Debug, Clone)]
pub struct ClassifierSettings {
    /// `regex` or `http`.
    pub backend: String,

    /// Optional model identifier sent to the HTTP backend.
    pub model: String,

    /// HTTP classifier endpoint.
    pub endpoint: String,

    /// Optional bearer token.
    pub api_key: Option<String>,

    /// Per-request timeout.
    pub timeout_ms: u64,

    /// Minimum confidence accepted from a model backend.
    pub min_confidence: f32,
}

impl Default for ClassifierSettings {
    fn default() -> Self {
        Self {
            backend: "regex".to_string(),
            model: String::new(),
            endpoint: String::new(),
            api_key: None,
            timeout_ms: 5_000,
            min_confidence: 0.5,
        }
    }
}

// ============================================================================
// Classifier statistics
// ============================================================================

/// Thread-safe statistics for classifier calls and fallback behavior.
///
/// These counters are intentionally small and process-local. They are useful
/// for demonstrating fallback safety without coupling the classifier to a
/// particular metrics implementation.
#[derive(Debug, Default)]
pub struct ClassifierStats {
    calls: AtomicU64,
    fallbacks: AtomicU64,
}

impl ClassifierStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    pub fn fallbacks(&self) -> u64 {
        self.fallbacks.load(Ordering::Relaxed)
    }

    fn record_call(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    fn record_fallback(&self) {
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
    }
}

// ============================================================================
// Regex backend
// ============================================================================

/// Deterministic regex classifier.
///
/// This preserves the behavior of the original classifier and therefore acts
/// as the safe baseline/fallback backend.
pub struct RegexRequestClassifier;

impl RegexRequestClassifier {
    fn complexity_for(request_type: RequestType) -> u8 {
        match request_type {
            RequestType::CodeGeneration => 4,
            RequestType::CodeUnderstanding => 3,
            RequestType::TechnicalDesign => 4,
            RequestType::AnalyticalReasoning => 4,
            RequestType::Writing => 3,
            RequestType::FactualLookup => 2,
            RequestType::General => 1,
        }
    }

    fn confidence_for(text: &str, request_type: RequestType) -> f32 {
        if request_type == RequestType::General {
            return 0.50;
        }

        let Some((_, patterns)) = CATEGORY_PATTERNS
            .iter()
            .find(|(candidate, _)| *candidate == request_type)
        else {
            return 0.50;
        };

        let matches = patterns.iter().filter(|pattern| pattern.is_match(text)).count();

        match matches {
            0 => 0.50,
            1 => 0.70,
            2 => 0.82,
            3 => 0.90,
            _ => 0.95,
        }
    }
}

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    async fn classify(
        &self,
        input: ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        let request_type = classify_request_type(input.query);
        let complexity = Self::complexity_for(request_type);
        let confidence = Self::confidence_for(input.query, request_type);

        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }

    fn name(&self) -> &'static str {
        "regex"
    }
}

// ============================================================================
// HTTP/model backend
// ============================================================================

/// HTTP-backed model classifier.
///
/// Expected response:
///
/// ```json
/// {
///   "request_type": "code_generation",
///   "complexity": 4,
///   "confidence": 0.91
/// }
/// ```
///
/// A response may also wrap the classification in:
///
/// ```json
/// {
///   "classification": {
///     "request_type": "code_generation",
///     "complexity": 4,
///     "confidence": 0.91
///   }
/// }
/// ```
pub struct HttpRequestClassifier {
    client: reqwest::Client,
    settings: ClassifierSettings,
}

impl HttpRequestClassifier {
    pub fn new(settings: ClassifierSettings) -> Result<Self, ClassifyError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| ClassifyError::Http(error.to_string()))?;

        Ok(Self { client, settings })
    }

    fn parse_response(value: Value) -> Result<Classification, ClassifyError> {
        let object = value
            .get("classification")
            .unwrap_or(&value);

        let request_type_value = object
            .get("request_type")
            .or_else(|| object.get("label"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ClassifyError::InvalidResponse(
                    "missing request_type".to_string(),
                )
            })?;

        let request_type = RequestType::from_wire(request_type_value)
            .ok_or_else(|| {
                ClassifyError::InvalidRequestType(
                    request_type_value.to_string(),
                )
            })?;

        let complexity = object
            .get("complexity")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ClassifyError::InvalidResponse(
                    "missing complexity".to_string(),
                )
            })? as u8;

        if !(1..=5).contains(&complexity) {
            return Err(ClassifyError::InvalidComplexity(complexity));
        }

        let confidence = object
            .get("confidence")
            .and_then(Value::as_f64)
            .ok_or(ClassifyError::InvalidConfidence)? as f32;

        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(ClassifyError::InvalidConfidence);
        }

        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

#[async_trait]
impl RequestClassifier for HttpRequestClassifier {
    async fn classify(
        &self,
        input: ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        if self.settings.endpoint.trim().is_empty() {
            return Err(ClassifyError::EndpointMissing);
        }

        let mut request_body = serde_json::json!({
            "query": input.query,
        });

        if let Some(context) = input.context {
            request_body["context"] = Value::String(context.to_string());
        }

        if !self.settings.model.is_empty() {
            request_body["model"] =
                Value::String(self.settings.model.clone());
        }

        let mut request = self
            .client
            .post(&self.settings.endpoint)
            .json(&request_body);

        if let Some(api_key) = self.settings.api_key.as_deref() {
            request = request.bearer_auth(api_key);
        }

        let response = timeout(
            Duration::from_millis(self.settings.timeout_ms.max(1)),
            request.send(),
        )
        .await
        .map_err(|_| ClassifyError::Timeout)?
        .map_err(|error| ClassifyError::Http(error.to_string()))?;

        if !response.status().is_success() {
            return Err(ClassifyError::Http(format!(
                "HTTP status {}",
                response.status()
            )));
        }

        let value: Value = response
            .json()
            .await
            .map_err(|error| ClassifyError::InvalidResponse(error.to_string()))?;

        let classification = Self::parse_response(value)?;

        if classification.confidence < self.settings.min_confidence {
            return Err(ClassifyError::LowConfidence(
                classification.confidence,
            ));
        }

        Ok(classification)
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

// ============================================================================
// Backend construction + fallback
// ============================================================================

/// Construct the configured classifier backend.
///
/// Unknown backend names deliberately fall back to regex so a bad optional
/// configuration cannot disable routing.
pub fn build_classifier(
    settings: &ClassifierSettings,
) -> Arc<dyn RequestClassifier> {
    match settings.backend.trim().to_ascii_lowercase().as_str() {
        "http" | "hosted" => match HttpRequestClassifier::new(settings.clone()) {
            Ok(classifier) => Arc::new(classifier),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "classifier: failed to construct HTTP backend; using regex"
                );
                Arc::new(RegexRequestClassifier)
            }
        },

        _ => Arc::new(RegexRequestClassifier),
    }
}

/// Run a classifier and fall back to the deterministic regex classifier on
/// any backend error.
///
/// The fallback itself is intentionally not recursively routed through the
/// configured backend.
pub async fn classify_with_fallback(
    backend: &dyn RequestClassifier,
    input: &ClassifyInput<'_>,
    stats: Option<&ClassifierStats>,
) -> (Classification, bool) {
    if let Some(stats) = stats {
        stats.record_call();
    }

    match backend
        .classify(ClassifyInput {
            query: input.query,
            context: input.context,
        })
        .await
    {
        Ok(classification) => (classification, false),

        Err(error) => {
            if let Some(stats) = stats {
                stats.record_fallback();
            }

            tracing::warn!(
                backend = backend.name(),
                error = %error,
                "classifier backend failed; falling back to regex"
            );

            let regex = RegexRequestClassifier;

            let classification = regex
                .classify(ClassifyInput {
                    query: input.query,
                    context: input.context,
                })
                .await
                .expect("regex classifier must not fail");

            (classification, true)
        }
    }
}

// ============================================================================
// Request-type regex classifier
// ============================================================================

/// Bucket a query into a request type using the existing regex vote-count
/// classifier.
pub fn classify_request_type(text: &str) -> RequestType {
    let mut best = RequestType::General;
    let mut best_score = 0usize;

    for (request_type, patterns) in CATEGORY_PATTERNS.iter() {
        let score = patterns
            .iter()
            .filter(|pattern| pattern.is_match(text))
            .count();

        if score > best_score {
            best_score = score;
            best = *request_type;
        }
    }

    best
}

// ============================================================================
// Complexity helpers
// ============================================================================

/// Estimate a coarse complexity from the request type.
///
/// This is deliberately deterministic for the regex baseline.
pub fn complexity_for_request_type(request_type: RequestType) -> u8 {
    match request_type {
        RequestType::CodeGeneration => 4,
        RequestType::CodeUnderstanding => 3,
        RequestType::TechnicalDesign => 4,
        RequestType::AnalyticalReasoning => 4,
        RequestType::Writing => 3,
        RequestType::FactualLookup => 2,
        RequestType::General => 1,
    }
}

// ============================================================================
// Feedback signal
// ============================================================================

/// Extract a reward from a follow-up message.
///
/// Negative signals are checked first so a mixed message such as
/// "thanks but that's wrong" is treated as negative.
pub fn signal(text: &str) -> Option<f64> {
    if NEGATIVE_SIGNALS.iter().any(|pattern| pattern.is_match(text)) {
        return Some(0.0);
    }

    if POSITIVE_SIGNALS.iter().any(|pattern| pattern.is_match(text)) {
        return Some(1.0);
    }

    None
}

// ============================================================================
// Learning/scoring
// ============================================================================

fn cold_start_prior(
    quality_tier: i32,
    strengths: &[RequestType],
    request_type: RequestType,
) -> f64 {
    let tier_base =
        0.5 + 0.15 * (quality_tier - 1).max(0) as f64;

    let bonus = if strengths.contains(&request_type) {
        0.15
    } else {
        0.0
    };

    (tier_base + bonus).clamp(0.05, 0.95)
}

/// Fold an observation into a learned cell.
pub fn update_cell(cell: Cell, observation: f64) -> Cell {
    let n_eff = cell.samples.min(MAX_SAMPLES);

    let new_mean = cell.quality_mean
        + (observation - cell.quality_mean)
            / (n_eff as f64 + 1.0);

    Cell {
        quality_mean: new_mean,
        samples: (cell.samples + 1).min(MAX_SAMPLES),
    }
}

/// Return the cold-start quality prior for a tier/request-type pair.
pub fn tier_prior(tier: Tier, request_type: RequestType) -> f64 {
    let arm = TIER_ARMS
        .iter()
        .find(|arm| arm.tier == tier)
        .expect("every Tier has a TierArm");

    cold_start_prior(
        arm.quality_tier,
        arm.strengths,
        request_type,
    )
}

fn beta_sample<R: Rng + ?Sized>(
    alpha: f64,
    beta: f64,
    rng: &mut R,
) -> f64 {
    let a = alpha.max(1e-6);
    let b = beta.max(1e-6);

    match Beta::new(a, b) {
        Ok(dist) => dist.sample(rng),
        Err(_) => a / (a + b),
    }
}

/// Thompson-sample a model-strength tier.
pub fn pick_model_thompson<R: Rng + ?Sized>(
    cells: &CellMap,
    request_type: RequestType,
    w_quality: f64,
    w_cost: f64,
    rng: &mut R,
) -> Tier {
    let lo = TIER_ARMS
        .iter()
        .map(|arm| arm.cost)
        .fold(f64::INFINITY, f64::min);

    let hi = TIER_ARMS
        .iter()
        .map(|arm| arm.cost)
        .fold(f64::NEG_INFINITY, f64::max);

    let span = hi - lo;

    let mut best = TIER_ARMS[0].tier;
    let mut best_score = f64::NEG_INFINITY;

    for arm in TIER_ARMS.iter() {
        let prior = cold_start_prior(
            arm.quality_tier,
            arm.strengths,
            request_type,
        );

        let (successes, failures) =
            match cells.get(&(arm.tier, request_type)) {
                Some(cell) => {
                    let successes =
                        cell.quality_mean * cell.samples as f64;

                    let failures =
                        cell.samples as f64 - successes;

                    (successes, failures)
                }

                None => (0.0, 0.0),
            };

        let alpha =
            prior * PRIOR_PSEUDO_COUNT + successes;

        let beta =
            (1.0 - prior) * PRIOR_PSEUDO_COUNT + failures;

        let sampled_quality =
            beta_sample(alpha, beta, rng);

        let normalized_cost = if span > 0.0 {
            (arm.cost - lo) / span
        } else {
            0.0
        };

        let score =
            w_quality * sampled_quality
                + w_cost * (1.0 - normalized_cost);

        if score > best_score {
            best_score = score;
            best = arm.tier;
        }
    }

    best
}

/// Public helper used by routing to select the tier.
pub fn select_tier<R: Rng + ?Sized>(
    query: &str,
    provider: &str,
    request_type: RequestType,
    cells: &CellMap,
    rng: &mut R,
) -> Tier {
    let tier = pick_model_thompson(
        cells,
        request_type,
        DEFAULT_W_QUALITY,
        DEFAULT_W_COST,
        rng,
    );

    let preview: String =
        query.chars().take(120).collect();

    tracing::info!(
        target: "nasiko::llm_router::classifier",
        provider = %provider,
        query_chars = query.chars().count(),
        query_preview = %preview,
        request_type = %request_type.as_str(),
        learned_cells = cells.len(),
        classified_tier = ?tier,
        "classifier: selected model tier"
    );

    tier
}

/// Backward-compatible typed tier-selection helper.
pub fn pick_tier_for_request<R: Rng + ?Sized>(
    cells: &CellMap,
    request_type: RequestType,
    w_quality: f64,
    w_cost: f64,
    rng: &mut R,
) -> Tier {
    pick_model_thompson(
        cells,
        request_type,
        w_quality,
        w_cost,
        rng,
    )
}

// ============================================================================
// Existing public entry point
// ============================================================================

/// Classify a query and select a model-strength tier.
///
/// This preserves the original public classifier API.
pub fn classify<R: Rng + ?Sized>(
    query: &str,
    provider: &str,
    cells: &CellMap,
    rng: &mut R,
) -> (Tier, RequestType) {
    let request_type =
        classify_request_type(query);

    let tier = pick_model_thompson(
        cells,
        request_type,
        DEFAULT_W_QUALITY,
        DEFAULT_W_COST,
        rng,
    );

    let preview: String =
        query.chars().take(120).collect();

    tracing::info!(
        target: "nasiko::llm_router::classifier",
        provider = %provider,
        query_chars = query.chars().count(),
        query_preview = %preview,
        request_type = %request_type.as_str(),
        learned_cells = cells.len(),
        classified_tier = ?tier,
        "classifier: classified query into request type and Thompson-sampled a model tier"
    );

    (tier, request_type)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    // ------------------------------------------------------------------------
    // Request type
    // ------------------------------------------------------------------------

    #[test]
    fn request_type_matches_reference_examples() {
        use RequestType::*;

        assert_eq!(
            classify_request_type(
                "build me a python script that parses CSV"
            ),
            CodeGeneration
        );

        assert_eq!(
            classify_request_type(
                "write me a Python sort function"
            ),
            CodeGeneration
        );

        assert_eq!(
            classify_request_type(
                "explain what this function does"
            ),
            CodeUnderstanding
        );

        assert_eq!(
            classify_request_type(
                "how should I design this API?"
            ),
            TechnicalDesign
        );

        assert_eq!(
            classify_request_type(
                "calculate the probability that it rains tomorrow"
            ),
            AnalyticalReasoning
        );

        assert_eq!(
            classify_request_type(
                "draft an email to my team about the outage"
            ),
            Writing
        );

        assert_eq!(
            classify_request_type(
                "what is the capital of France?"
            ),
            FactualLookup
        );

        assert_eq!(
            classify_request_type("hello there"),
            General
        );
    }

    #[test]
    fn request_type_round_trips_through_wire() {
        for request_type in [
            RequestType::CodeGeneration,
            RequestType::CodeUnderstanding,
            RequestType::TechnicalDesign,
            RequestType::AnalyticalReasoning,
            RequestType::Writing,
            RequestType::FactualLookup,
            RequestType::General,
        ] {
            assert_eq!(
                RequestType::from_wire(
                    request_type.as_str()
                ),
                Some(request_type)
            );
        }

        assert_eq!(
            RequestType::from_wire("nonsense"),
            None
        );
    }

    // ------------------------------------------------------------------------
    // Regex backend
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn regex_backend_returns_typed_classification() {
        let classifier =
            RegexRequestClassifier;

        let result = classifier
            .classify(ClassifyInput {
                query:
                    "write a python function to sort a list",
                context: None,
            })
            .await
            .unwrap();

        assert_eq!(
            result.request_type,
            RequestType::CodeGeneration
        );

        assert!((1..=5).contains(&result.complexity));
        assert!((0.0..=1.0).contains(&result.confidence));
    }

    #[tokio::test]
    async fn regex_backend_is_deterministic() {
        let classifier =
            RegexRequestClassifier;

        let input = ClassifyInput {
            query: "what is the capital of France?",
            context: None,
        };

        let first =
            classifier.classify(input.clone()).await.unwrap();

        let second =
            classifier.classify(input).await.unwrap();

        assert_eq!(first, second);
    }

    // ------------------------------------------------------------------------
    // HTTP response parsing
    // ------------------------------------------------------------------------

    #[test]
    fn parses_flat_http_response() {
        let value = serde_json::json!({
            "request_type": "code_generation",
            "complexity": 4,
            "confidence": 0.91
        });

        let classification =
            HttpRequestClassifier::parse_response(value)
                .unwrap();

        assert_eq!(
            classification.request_type,
            RequestType::CodeGeneration
        );

        assert_eq!(
            classification.complexity,
            4
        );

        assert!(
            (classification.confidence - 0.91).abs()
                < 0.001
        );
    }

    #[test]
    fn parses_wrapped_http_response() {
        let value = serde_json::json!({
            "classification": {
                "request_type": "factual_lookup",
                "complexity": 2,
                "confidence": 0.88
            }
        });

        let classification =
            HttpRequestClassifier::parse_response(value)
                .unwrap();

        assert_eq!(
            classification.request_type,
            RequestType::FactualLookup
        );

        assert_eq!(
            classification.complexity,
            2
        );
    }

    #[test]
    fn rejects_invalid_complexity() {
        let value = serde_json::json!({
            "request_type": "general",
            "complexity": 8,
            "confidence": 0.9
        });

        let result =
            HttpRequestClassifier::parse_response(value);

        assert_eq!(
            result,
            Err(ClassifyError::InvalidComplexity(8))
        );
    }

    // ------------------------------------------------------------------------
    // Fallback
    // ------------------------------------------------------------------------

    struct FailingClassifier;

    #[async_trait]
    impl RequestClassifier for FailingClassifier {
        async fn classify(
            &self,
            _input: ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Timeout)
        }

        fn name(&self) -> &'static str {
            "failing-test"
        }
    }

    #[tokio::test]
    async fn fallback_uses_regex_after_backend_failure() {
        let backend =
            FailingClassifier;

        let stats =
            ClassifierStats::new();

        let (classification, fell_back) =
            classify_with_fallback(
                &backend,
                &ClassifyInput {
                    query:
                        "write a python function",
                    context: None,
                },
                Some(&stats),
            )
            .await;

        assert!(fell_back);

        assert_eq!(
            classification.request_type,
            RequestType::CodeGeneration
        );

        assert_eq!(
            stats.calls(),
            1
        );

        assert_eq!(
            stats.fallbacks(),
            1
        );
    }

    struct HealthyClassifier;

    #[async_trait]
    impl RequestClassifier for HealthyClassifier {
        async fn classify(
            &self,
            _input: ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            Ok(Classification {
                request_type:
                    RequestType::FactualLookup,
                complexity: 2,
                confidence: 0.9,
            })
        }

        fn name(&self) -> &'static str {
            "healthy-test"
        }
    }

    #[tokio::test]
    async fn healthy_backend_does_not_fallback() {
        let backend =
            HealthyClassifier;

        let stats =
            ClassifierStats::new();

        let (classification, fell_back) =
            classify_with_fallback(
                &backend,
                &ClassifyInput {
                    query:
                        "what is Rust?",
                    context: None,
                },
                Some(&stats),
            )
            .await;

        assert!(!fell_back);

        assert_eq!(
            classification.request_type,
            RequestType::FactualLookup
        );

        assert_eq!(
            stats.calls(),
            1
        );

        assert_eq!(
            stats.fallbacks(),
            0
        );
    }

    // ------------------------------------------------------------------------
    // Scoring
    // ------------------------------------------------------------------------

    #[test]
    fn cold_start_prior_matches_reference() {
        assert_eq!(
            cold_start_prior(
                3,
                &[RequestType::AnalyticalReasoning],
                RequestType::AnalyticalReasoning
            ),
            0.95
        );

        assert_eq!(
            cold_start_prior(
                1,
                &[],
                RequestType::AnalyticalReasoning
            ),
            0.5
        );
    }

    #[test]
    fn update_cell_matches_reference() {
        let cell = update_cell(
            Cell {
                quality_mean: 0.5,
                samples: 0,
            },
            1.0,
        );

        assert_eq!(
            cell.quality_mean,
            1.0
        );

        assert_eq!(
            cell.samples,
            1
        );

        let cell =
            update_cell(cell, 0.0);

        assert!(
            (cell.quality_mean - 0.5).abs()
                < 1e-9
        );

        assert_eq!(
            cell.samples,
            2
        );
    }

    #[test]
    fn beta_sample_stays_in_unit_interval() {
        let mut rng =
            StdRng::seed_from_u64(1);

        for _ in 0..1000 {
            let value =
                beta_sample(
                    2.0,
                    5.0,
                    &mut rng,
                );

            assert!(
                (0.0..=1.0).contains(&value)
            );
        }

        assert!(
            beta_sample(
                0.0,
                0.0,
                &mut rng
            )
            .is_finite()
        );
    }

    #[test]
    fn thompson_converges_to_learned_best_tier() {
        let mut cells =
            CellMap::new();

        cells.insert(
            (
                Tier::Tier1,
                RequestType::CodeGeneration,
            ),
            Cell {
                quality_mean: 0.99,
                samples: MAX_SAMPLES,
            },
        );

        for tier in [
            Tier::Tier2,
            Tier::Tier3,
        ] {
            cells.insert(
                (
                    tier,
                    RequestType::CodeGeneration,
                ),
                Cell {
                    quality_mean: 0.05,
                    samples: MAX_SAMPLES,
                },
            );
        }

        let mut rng =
            StdRng::seed_from_u64(42);

        for _ in 0..200 {
            let tier =
                pick_model_thompson(
                    &cells,
                    RequestType::CodeGeneration,
                    1.0,
                    0.0,
                    &mut rng,
                );

            assert_eq!(
                tier,
                Tier::Tier1
            );
        }
    }

    #[test]
    fn thompson_explores_unlearned_arm() {
        let mut cells =
            CellMap::new();

        cells.insert(
            (
                Tier::Tier1,
                RequestType::CodeGeneration,
            ),
            Cell {
                quality_mean: 0.7,
                samples: 8,
            },
        );

        let mut rng =
            StdRng::seed_from_u64(1);

        let mut distinct =
            std::collections::HashSet::new();

        for _ in 0..200 {
            distinct.insert(
                pick_model_thompson(
                    &cells,
                    RequestType::CodeGeneration,
                    1.0,
                    0.0,
                    &mut rng,
                ),
            );
        }

        assert!(
            distinct.len() > 1,
            "expected exploration, got {distinct:?}"
        );
    }

    #[test]
    fn all_cost_prefers_cheapest_tier() {
        let cells =
            CellMap::new();

        let mut rng =
            StdRng::seed_from_u64(7);

        for _ in 0..200 {
            let tier =
                pick_model_thompson(
                    &cells,
                    RequestType::General,
                    0.0,
                    1.0,
                    &mut rng,
                );

            assert_eq!(
                tier,
                Tier::Tier3
            );
        }
    }

    #[test]
    fn classify_returns_valid_tier_and_request_type() {
        let cells =
            CellMap::new();

        let mut rng =
            StdRng::seed_from_u64(3);

        let (tier, request_type) =
            classify(
                "write a python function that sorts a list",
                "anthropic",
                &cells,
                &mut rng,
            );

        assert_eq!(
            request_type,
            RequestType::CodeGeneration
        );

        assert!(
            matches!(
                tier,
                Tier::Tier1
                    | Tier::Tier2
                    | Tier::Tier3
            )
        );
    }

    // ------------------------------------------------------------------------
    // Feedback
    // ------------------------------------------------------------------------

    #[test]
    fn signal_matches_reference() {
        assert_eq!(
            signal(
                "perfect, that worked. thanks!"
            ),
            Some(1.0)
        );

        assert_eq!(
            signal(
                "that's wrong, try again"
            ),
            Some(0.0)
        );

        assert_eq!(
            signal(
                "now add error handling for missing files"
            ),
            None
        );

        assert_eq!(
            signal(
                "thanks but that's wrong"
            ),
            Some(0.0)
        );
    }

    // ------------------------------------------------------------------------
    // Settings
    // ------------------------------------------------------------------------

    #[test]
    fn default_settings_use_regex() {
        let settings =
            ClassifierSettings::default();

        assert_eq!(
            settings.backend,
            "regex"
        );

        assert_eq!(
            settings.timeout_ms,
            5000
        );

        assert!(
            settings.endpoint.is_empty()
        );

        assert!(
            settings.model.is_empty()
        );

        assert!(
            settings.api_key.is_none()
        );

        assert!(
            (settings.min_confidence - 0.5).abs()
                < f32::EPSILON
        );
    }

    // ------------------------------------------------------------------------
    // Backend factory
    // ------------------------------------------------------------------------

    #[test]
    fn unknown_backend_falls_back_to_regex() {
        let settings =
            ClassifierSettings {
                backend: "unknown".to_string(),
                ..Default::default()
            };

        let classifier =
            build_classifier(&settings);

        assert_eq!(
            classifier.name(),
            "regex"
        );
    }

    #[test]
    fn regex_backend_can_be_constructed() {
        let settings =
            ClassifierSettings::default();

        let classifier =
            build_classifier(&settings);

        assert_eq!(
            classifier.name(),
            "regex"
        );
    }
}
