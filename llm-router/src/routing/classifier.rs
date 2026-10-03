//! Query classifier — maps an incoming query to a model [`Tier`] for the destination
//! provider.
//!
//! The classifier answers "how much model does this query need?" as a coarse tier; the
//! [tier registry](super::registry) then maps `(provider, tier)` to a concrete model.
//! Provider selection and request translation happen elsewhere (the resolver / inbound
//! spokes) — the classifier only chooses the *strength* of the model, never the provider.
//!
//! ## How the tier is chosen
//!
//! Two steps, both faithful ports of the litellm-rust **Adaptive Router** reference
//! (`classifier/{categories,signals}.rs`, `scoring.rs`) — see `THIRD_PARTY_LICENSES.md`
//! (crate root) for the upstream MIT attribution this requires:
//!
//! 1. **Request type** — a regex vote-count classifier buckets the query into one of a
//!    handful of [`RequestType`]s (code generation, factual lookup, …), defaulting to
//!    `General`.
//! 2. **Tier** — the three tiers are treated as bandit *arms*. [`pick_model_thompson`]
//!    Thompson-samples a quality estimate per tier from a Beta posterior — seeded by a
//!    cold-start prior (stronger/on-strength tiers start higher) and updated by learned
//!    [`Cell`]s — then blends it with a normalized cost term and takes the argmax.
//!
//! The learned [`Cell`]s come from real feedback: the router credits a tier's quality from
//! the user's next-turn reaction ([`signal`]), persisted per provider by the
//! [cell store](super::cells). With no learning yet the priors + cost blend decide; as
//! feedback accumulates the posterior tightens and selection converges. Thompson's
//! stochasticity is the exploration that makes that learning possible, so production feeds
//! it an entropy RNG; tests inject a seeded one.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rand::Rng;
use rand_distr::{Beta, Distribution};

use super::patterns::{CATEGORY_PATTERNS, NEGATIVE_SIGNALS, POSITIVE_SIGNALS};

/// Coarse model strength tier. Tier 1 = most capable (complex queries), Tier 3 = smallest
/// (very simple queries), Tier 2 = in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Complex queries — the strongest model in the provider's registry.
    Tier1,
    /// Mid-complexity queries.
    Tier2,
    /// Very simple queries — the smallest/cheapest model.
    Tier3,
}

/// The coarse kind of work a query represents. Learning is keyed on this, so the router can
/// discover (e.g.) that the cheap tier is good enough for `FactualLookup` but not
/// `CodeGeneration`. Order is irrelevant; `General` is the catch-all default.
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
    /// Stable string form used as the persisted cell key (`router_quality_cells.request_type`).
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

    /// Inverse of [`RequestType::as_str`]; `None` for unknown values (a row written by an
    /// older/newer schema is skipped rather than trusted). Named `from_wire` rather than
    /// `from_str` to avoid shadowing the `std::str::FromStr` trait method.
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

/// One learned quality estimate: a running mean of observed reward for a `(tier,
/// request_type)` under some provider, plus how many observations back it. This is the unit
/// the [cell store](super::cells) persists; it is a direct port of the reference
/// `scoring.rs::Cell`.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub quality_mean: f64,
    pub samples: i64,
}

/// Learned cells for a single provider, keyed by `(tier, request_type)`. The provider is
/// the scope of the whole map, so it is not part of the key.
pub type CellMap = HashMap<(Tier, RequestType), Cell>;

/// Sample cap for the running mean — past this the mean stops chasing new observations, so
/// a cell's estimate is stable once well-sampled. Port of the reference `MAX_SAMPLES`.
pub const MAX_SAMPLES: i64 = 200;

/// Strength of the cold-start prior, in Beta pseudo-observations. Port of the reference
/// `PRIOR_PSEUDO_COUNT`.
const PRIOR_PSEUDO_COUNT: f64 = 4.0;

/// Quality/cost blend weights (`w_quality`, `w_cost`). The reference default: quality leads,
/// cost trims. Tunable — learning corrects any cold-start bias over time.
pub const DEFAULT_W_QUALITY: f64 = 0.7;
pub const DEFAULT_W_COST: f64 = 0.3;

/// A tier as a bandit arm: its nominal quality tier (for the cold-start prior), a relative
/// cost, and the request types it is expected to be good at (a prior bonus). Costs are a
/// generic gradient — only their *relative* ordering matters after normalization, so this is
/// provider-independent for now.
struct TierArm {
    tier: Tier,
    quality_tier: i32,
    cost: f64,
    strengths: &'static [RequestType],
}

/// The three tiers as bandit arms. Tier1 = strongest+priciest, Tier3 = weakest+cheapest.
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
        strengths: &[RequestType::CodeUnderstanding, RequestType::Writing],
    },
    TierArm {
        tier: Tier::Tier3,
        quality_tier: 1,
        cost: 0.8,
        strengths: &[RequestType::FactualLookup, RequestType::General],
    },
];

// --------------------------------------------------------------------------
// 1. Request-type classifier — port of classifier/categories.rs
//    (order matters: on a tie the earlier category wins; patterns in `super::patterns`)
// --------------------------------------------------------------------------

/// Bucket a query into a [`RequestType`] by vote count — the category matching the most
/// patterns wins, ties broken by declaration order, defaulting to `General`. Port of
/// `categories.rs::classify`.
pub fn classify_request_type(text: &str) -> RequestType {
    let mut best = RequestType::General;
    let mut best_score = 0usize;
    for (rt, pats) in CATEGORY_PATTERNS.iter() {
        let score = pats.iter().filter(|p| p.is_match(text)).count();
        if score > best_score {
            best_score = score;
            best = *rt;
        }
    }
    best
}

// --------------------------------------------------------------------------
// 2. Feedback signal — port of classifier/signals.rs (patterns in `super::patterns`)
// --------------------------------------------------------------------------

/// Extract a reward from a follow-up message: `0.0` on a complaint, `1.0` on approval,
/// `None` when the text carries no clear verdict. Negative is checked first so a mixed
/// message ("thanks but that's wrong") counts as negative. The regexes are deliberately
/// conservative, so an ordinary new question yields `None` and earns no false credit. Port
/// of `signals.rs::signal`.
pub fn signal(text: &str) -> Option<f64> {
    if NEGATIVE_SIGNALS.iter().any(|p| p.is_match(text)) {
        return Some(0.0);
    }
    if POSITIVE_SIGNALS.iter().any(|p| p.is_match(text)) {
        return Some(1.0);
    }
    None
}

// --------------------------------------------------------------------------
// 3. Scoring — port of scoring.rs
// --------------------------------------------------------------------------

/// Initial quality estimate for a tier before any feedback: a base that grows with the
/// quality tier plus a bonus when the request type is one of the tier's strengths, clamped
/// away from the extremes. Port of `scoring.rs::cold_start_prior`.
fn cold_start_prior(quality_tier: i32, strengths: &[RequestType], rt: RequestType) -> f64 {
    let tier_base = 0.5 + 0.15 * (quality_tier - 1).max(0) as f64;
    let bonus = if strengths.contains(&rt) { 0.15 } else { 0.0 };
    (tier_base + bonus).clamp(0.05, 0.95)
}

/// Fold one observation into a cell's running mean, capping the effective sample count so a
/// well-sampled estimate stays stable. Port of `scoring.rs::update_cell`.
pub fn update_cell(cell: Cell, observation: f64) -> Cell {
    let n_eff = cell.samples.min(MAX_SAMPLES);
    let new_mean = cell.quality_mean + (observation - cell.quality_mean) / (n_eff as f64 + 1.0);
    Cell {
        quality_mean: new_mean,
        samples: (cell.samples + 1).min(MAX_SAMPLES),
    }
}

/// The cold-start prior for a given tier and request type, used to seed both the Beta
/// posterior in [`pick_model_thompson`] and a fresh cell in the store.
pub fn tier_prior(tier: Tier, rt: RequestType) -> f64 {
    let arm = TIER_ARMS
        .iter()
        .find(|a| a.tier == tier)
        .expect("every Tier has a TierArm");
    cold_start_prior(arm.quality_tier, arm.strengths, rt)
}

/// Sample a `Beta(alpha, beta)` variate, guarding degenerate parameters. Falls back to the
/// distribution mean if the parameters can't form a valid Beta.
fn beta_sample<R: Rng + ?Sized>(alpha: f64, beta: f64, rng: &mut R) -> f64 {
    let a = alpha.max(1e-6);
    let b = beta.max(1e-6);
    match Beta::new(a, b) {
        Ok(dist) => dist.sample(rng),
        Err(_) => a / (a + b),
    }
}

/// Thompson-sample a [`Tier`] for `request_type`: draw a quality per tier from its Beta
/// posterior (cold-start prior as pseudo-observations + learned [`Cell`] as real ones),
/// blend with a normalized cost term, and take the argmax (ties → earlier/stronger tier).
/// Port of the reference `pick_model_thompson`, with the three tiers as the candidate arms.
pub fn pick_model_thompson<R: Rng + ?Sized>(
    cells: &CellMap,
    request_type: RequestType,
    w_quality: f64,
    w_cost: f64,
    rng: &mut R,
) -> Tier {
    let lo = TIER_ARMS
        .iter()
        .map(|a| a.cost)
        .fold(f64::INFINITY, f64::min);
    let hi = TIER_ARMS
        .iter()
        .map(|a| a.cost)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;

    let mut best = TIER_ARMS[0].tier;
    let mut best_score = f64::NEG_INFINITY;
    for arm in TIER_ARMS.iter() {
        let prior = cold_start_prior(arm.quality_tier, arm.strengths, request_type);
        let (successes, failures) = match cells.get(&(arm.tier, request_type)) {
            Some(cell) => {
                let s = cell.quality_mean * cell.samples as f64;
                (s, cell.samples as f64 - s)
            }
            None => (0.0, 0.0),
        };
        let alpha = prior * PRIOR_PSEUDO_COUNT + successes;
        let beta = (1.0 - prior) * PRIOR_PSEUDO_COUNT + failures;
        let q = beta_sample(alpha, beta, rng);
        let norm_cost = if span > 0.0 {
            (arm.cost - lo) / span
        } else {
            0.0
        };
        let score = w_quality * q + w_cost * (1.0 - norm_cost);
        if score > best_score {
            best_score = score;
            best = arm.tier;
        }
    }
    best
}

// --------------------------------------------------------------------------
// 4. Public entry point
// --------------------------------------------------------------------------

/// Classify a `query` into a model [`Tier`] (and the [`RequestType`] it was bucketed as) for
/// the destination `provider`.
///
/// `provider` is the **destination** provider the request will be routed to (already
/// resolved), not the agent's client SDK — the tier is later looked up in *that* provider's
/// registry, and the returned `RequestType` is what feedback is later credited to.
///
/// `cells` are the provider's learned quality estimates (empty ⇒ pure cold-start priors);
/// `rng` drives Thompson exploration (entropy in production, seeded in tests).
pub fn classify<R: Rng + ?Sized>(
    query: &str,
    provider: &str,
    cells: &CellMap,
    rng: &mut R,
) -> (Tier, RequestType) {
    let request_type = classify_request_type(query);
    let tier = pick_model_thompson(cells, request_type, DEFAULT_W_QUALITY, DEFAULT_W_COST, rng);
    let preview: String = query.chars().take(120).collect();
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

// ==========================================================================
// RequestClassifier — the model-agnostic decision interface (P2)
// ==========================================================================
//
// `classify_request_type` above is the historical, keyword-only entry point. The trait
// below is the pluggable seam the router (and `examples/classifier_eval.rs`) call so a
// backend can be swapped without touching the routing flow. The regex implementation is the
// default and reproduces the historical behaviour; a local model and an optional hosted
// backend are alternative implementations, selected by configuration. See
// [`ClassifierRuntime`] for how the router turns their results into a tier while preserving
// the safe-default/fallback contract.

/// The typed input to a [`RequestClassifier`]: the query plus optional surrounding context
/// (pasted code, prior-turn detail). Context is a hint — the regex backend ignores it, the
/// local and hosted backends use it.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

impl<'a> ClassifyInput<'a> {
    /// Input with no context — the router's case (it only has the latest user message).
    pub fn query_only(query: &'a str) -> Self {
        Self {
            query,
            context: None,
        }
    }
}

/// A backend's verdict: the request type, its complexity (1–5), and the backend's
/// `confidence` in `request_type` (0–1). `confidence` is what the router compares against a
/// threshold; below it, the decision is treated as the safe default (see
/// [`ClassifierRuntime::classify_or_fallback`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// 1 = trivial single operation … 5 = intricate cross-component reasoning.
    pub complexity: u8,
    /// Calibrated probability that `request_type` is correct, in `[0, 1]`.
    pub confidence: f32,
}

/// Why a [`RequestClassifier`] could not produce a [`Classification`]. Every variant is a
/// reason the router should fall back to the regex baseline, never a reason to guess.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClassifyError {
    /// The backend is not configured (e.g. `local` with no model, or `hosted` with no
    /// endpoint).
    #[error("classifier backend not configured: {0}")]
    NotConfigured(String),
    /// The backend's model could not be loaded.
    #[error("classifier model load failed: {0}")]
    Load(String),
    /// The backend ran but produced unusable output (never a parsed-but-wrong call).
    #[error("classifier inference failed: {0}")]
    Inference(String),
    /// The backend exceeded its decision-latency budget.
    #[error("classifier timed out after {0:?}")]
    Timeout(Duration),
    /// A network / transport failure reaching a hosted backend.
    #[error("classifier backend error: {0}")]
    Backend(String),
}

/// A pluggable request-type classifier. Implementations must be `Send + Sync` (the router
/// holds an `Arc<dyn RequestClassifier>` across `.await`s) and should be cheap to call
/// repeatedly (the local model is; a hosted backend is bounded by [`TimeoutClassifier`]).
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Short, stable backend identifier for logs and the eval output (`"regex"`, `"local"`,
    /// `"hosted"`).
    fn name(&self) -> &str;

    /// Classify one request. Returning `Err` is a normal outcome — the caller falls back to
    /// the regex baseline and counts it; it is never a routing failure.
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// The regex baseline as a [`RequestClassifier`] — the default backend and the fallback for
/// every other one.
///
/// It wraps [`classify_request_type`] unchanged and reports **fixed** complexity and
/// confidence, because the keyword vote-count has neither signal:
/// - [`REGEX_COMPLEXITY`] — the mid-scale value, for every query;
/// - [`REGEX_CONFIDENCE`] — a deliberately low constant, so the router treats a bare regex
///   verdict as the safe default rather than a confident decision.
pub struct RegexClassifier;

/// The regex baseline's fixed complexity (see [`RegexClassifier`]).
pub const REGEX_COMPLEXITY: u8 = 3;
/// The regex baseline's fixed confidence (see [`RegexClassifier`]).
pub const REGEX_CONFIDENCE: f32 = 0.35;

/// The regex baseline's verdict for a query, with its fixed complexity/confidence.
pub fn regex_classification(query: &str) -> Classification {
    Classification {
        request_type: classify_request_type(query),
        complexity: REGEX_COMPLEXITY,
        confidence: REGEX_CONFIDENCE,
    }
}

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(regex_classification(input.query))
    }
}

/// The in-process model backend (`CLASSIFIER_BACKEND=local`): the hashed n-gram classifier in
/// [`super::request_model`], embedded in the binary. No network, no runtime file dependency.
pub struct LocalClassifier {
    model: super::request_model::RequestModel,
}

impl LocalClassifier {
    /// Wrap an already-loaded model.
    pub fn new(model: super::request_model::RequestModel) -> Self {
        Self { model }
    }

    /// Load the model embedded in this binary.
    pub fn embedded() -> Result<Self, ClassifyError> {
        super::request_model::embedded_model()
            .map(Self::new)
            .map_err(ClassifyError::Load)
    }

    /// Load a model from a weights file (the `CLASSIFIER_MODEL_PATH` override).
    pub fn from_path(path: &std::path::Path) -> Result<Self, ClassifyError> {
        super::request_model::load_from_file(path)
            .map(Self::new)
            .map_err(ClassifyError::Load)
    }
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let (request_type, complexity, confidence) =
            self.model.classify(input.query, input.context);
        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

/// Bounds any backend's decision latency. A hosted call without a deadline could stall a
/// routed request; wrapping the backend here keeps the timeout policy in the routing layer
/// (one place) rather than in each backend.
pub struct TimeoutClassifier {
    inner: Arc<dyn RequestClassifier>,
    timeout: Duration,
}

impl TimeoutClassifier {
    /// Wrap `inner` with a per-decision deadline. A zero timeout is treated as "no
    /// deadline" — the backend is returned unwrapped by [`build_request_classifier`].
    pub fn new(inner: Arc<dyn RequestClassifier>, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

#[async_trait]
impl RequestClassifier for TimeoutClassifier {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        match tokio::time::timeout(self.timeout, self.inner.classify(input)).await {
            Ok(result) => result,
            Err(_) => Err(ClassifyError::Timeout(self.timeout)),
        }
    }
}

/// Which classification backend is active. Parsed from `CLASSIFIER_BACKEND` in
/// [`crate::config::GatewayConfig`]; the default is [`BackendKind::Regex`], so an
/// out-of-the-box deployment makes no network call and needs no model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The keyword baseline (default).
    Regex,
    /// The embedded in-process model.
    Local,
    /// An OpenAI-compatible chat endpoint.
    Hosted,
}

impl BackendKind {
    /// Parse a backend label (case-insensitive). An empty string is [`BackendKind::Regex`];
    /// an unrecognized label is `None` so the caller can warn and fall back to the default.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "regex" => Some(BackendKind::Regex),
            "local" => Some(BackendKind::Local),
            "hosted" => Some(BackendKind::Hosted),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Regex => "regex",
            BackendKind::Local => "local",
            BackendKind::Hosted => "hosted",
        }
    }
}

/// Plain configuration for building a classifier. Deliberately env-free: the caller (the
/// router's `config.rs`, or the eval example) reads the environment and populates this, so
/// the routing library never reads env vars itself.
#[derive(Debug, Clone)]
pub struct ClassifierConfig {
    pub backend: BackendKind,
    /// Optional weights-file override for the local backend (empty ⇒ embedded model).
    pub model_path: String,
    /// Base URL (ending at the API version) for the hosted backend.
    pub endpoint: String,
    pub api_key: String,
    /// Model id for the hosted backend.
    pub model: String,
    /// Per-decision deadline applied to whichever backend is built. Zero ⇒ no timeout.
    pub timeout: Duration,
    /// Below this request-type confidence the router uses the safe default and counts a
    /// fallback. Must be in `[0, 1]`.
    pub low_confidence: f32,
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            backend: BackendKind::Regex,
            model_path: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            timeout: Duration::from_millis(1500),
            low_confidence: 0.55,
        }
    }
}

/// Build the configured backend, degrading to the regex classifier whenever the requested
/// backend cannot be constructed — a missing/corrupt model, a `local` build with no asset, or
/// a `hosted` build with no endpoint. This is the "regex fallback on model-load failure"
/// contract: the router always gets *a* working classifier. Runtime failures and timeouts are
/// handled separately, per decision, by [`ClassifierRuntime::classify_or_fallback`].
pub fn build_request_classifier(
    cfg: &ClassifierConfig,
    http: reqwest::Client,
) -> Arc<dyn RequestClassifier> {
    let inner: Arc<dyn RequestClassifier> = match cfg.backend {
        BackendKind::Regex => Arc::new(RegexClassifier),
        BackendKind::Local => {
            let loaded = if cfg.model_path.is_empty() {
                LocalClassifier::embedded()
            } else {
                LocalClassifier::from_path(std::path::Path::new(&cfg.model_path))
            };
            match loaded {
                Ok(classifier) => Arc::new(classifier),
                Err(e) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::classifier",
                        error = %e,
                        "classifier: local model failed to load; falling back to the regex baseline"
                    );
                    Arc::new(RegexClassifier)
                }
            }
        }
        BackendKind::Hosted => {
            if cfg.endpoint.is_empty() {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    "classifier: CLASSIFIER_BACKEND=hosted but CLASSIFIER_ENDPOINT is empty; falling back to the regex baseline"
                );
                Arc::new(RegexClassifier)
            } else {
                Arc::new(super::hosted_classifier::HostedClassifier::new(
                    http,
                    cfg.endpoint.clone(),
                    cfg.api_key.clone(),
                    cfg.model.clone(),
                ))
            }
        }
    };

    if cfg.timeout.is_zero() {
        inner
    } else {
        Arc::new(TimeoutClassifier::new(inner, cfg.timeout))
    }
}

/// Process-lifetime counters for the router's classifier decisions. Only two numbers matter
/// operationally: how often the classifier ran, and how often the router used the safe
/// default instead (a backend error, a timeout, or a low-confidence verdict). The fallback
/// rate is the `costs we measure` figure from the track brief.
#[derive(Debug, Default)]
pub struct ClassifierStats {
    decisions: AtomicU64,
    fallbacks: AtomicU64,
}

impl ClassifierStats {
    /// Total decisions taken (successful or fallen back).
    pub fn decisions(&self) -> u64 {
        self.decisions.load(Ordering::Relaxed)
    }

    /// Decisions answered by the regex safe default rather than the configured backend.
    pub fn fallbacks(&self) -> u64 {
        self.fallbacks.load(Ordering::Relaxed)
    }

    /// `fallbacks / decisions` in `[0, 1]`; `0.0` before any decision.
    pub fn fallback_rate(&self) -> f64 {
        let decisions = self.decisions();
        if decisions == 0 {
            0.0
        } else {
            self.fallbacks() as f64 / decisions as f64
        }
    }

    fn record(&self, fell_back: bool) {
        self.decisions.fetch_add(1, Ordering::Relaxed);
        if fell_back {
            self.fallbacks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The router's request classifier: a pluggable backend, the low-confidence threshold, and
/// the fallback counters. Held as `Arc<ClassifierRuntime>` by the router so the counters are
/// process-wide.
///
/// [`classify_or_fallback`](ClassifierRuntime::classify_or_fallback) is the single entry
/// point used by both the router and `examples/classifier_eval.rs`, so the eval exercises the
/// exact path production takes — including the fallback.
pub struct ClassifierRuntime {
    classifier: Arc<dyn RequestClassifier>,
    low_confidence: f32,
    stats: ClassifierStats,
}

impl ClassifierRuntime {
    /// Build a runtime over `classifier`, treating any verdict below `low_confidence` as
    /// uncertain. `low_confidence` is clamped to `[0, 1]`.
    pub fn new(classifier: Arc<dyn RequestClassifier>, low_confidence: f32) -> Self {
        Self {
            classifier,
            low_confidence: low_confidence.clamp(0.0, 1.0),
            stats: ClassifierStats::default(),
        }
    }

    /// The regex-baseline runtime — the default, and what tests use to reproduce the
    /// pre-classifier behaviour.
    pub fn regex() -> Self {
        Self::new(Arc::new(RegexClassifier), 1.0)
    }

    /// The active backend's name.
    pub fn name(&self) -> &str {
        self.classifier.name()
    }

    /// The confidence threshold below which a verdict is treated as uncertain.
    pub fn low_confidence(&self) -> f32 {
        self.low_confidence
    }

    /// The fallback counters.
    pub fn stats(&self) -> &ClassifierStats {
        &self.stats
    }

    /// Classify a request, returning the verdict and whether the safe default was used.
    ///
    /// Three cases produce `(regex_result, true)` — a **fallback**, not an error:
    /// - the backend returned `Err` (inference, load-at-call-time, or network failure);
    /// - the backend exceeded its timeout (surfaced as [`ClassifyError::Timeout`]);
    /// - the backend returned `Ok` but with `confidence < low_confidence`.
    ///
    /// The last case is the "safe low-confidence behaviour" the brief requires: an unsure
    /// model does not get to pin a conversation's model; the router falls back to exactly
    /// what it would have done before the classifier existed.
    pub async fn classify_or_fallback(
        &self,
        input: &ClassifyInput<'_>,
    ) -> (Classification, bool) {
        match self.classifier.classify(input).await {
            Ok(classification) if classification.confidence >= self.low_confidence => {
                self.stats.record(false);
                (classification, false)
            }
            Ok(classification) => {
                tracing::info!(
                    target: "nasiko::llm_router::classifier",
                    backend = self.classifier.name(),
                    confidence = classification.confidence,
                    threshold = self.low_confidence,
                    request_type = %classification.request_type.as_str(),
                    "classifier: low-confidence verdict — using the regex safe default"
                );
                self.stats.record(true);
                (regex_classification(input.query), true)
            }
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = self.classifier.name(),
                    error = %e,
                    "classifier: backend failed — falling back to the regex baseline"
                );
                self.stats.record(true);
                (regex_classification(input.query), true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    // --- request-type classifier (ports of the reference self-test) ---

    #[test]
    fn request_type_matches_reference_examples() {
        use RequestType::*;
        assert_eq!(
            classify_request_type("build me a python script that parses CSV"),
            CodeGeneration
        );
        assert_eq!(
            classify_request_type("write me a Python sort function"),
            CodeGeneration
        );
        assert_eq!(
            classify_request_type("explain what this function does"),
            CodeUnderstanding
        );
        assert_eq!(
            classify_request_type("how should I design this API?"),
            TechnicalDesign
        );
        assert_eq!(
            classify_request_type("calculate the probability that it rains tomorrow"),
            AnalyticalReasoning
        );
        assert_eq!(
            classify_request_type("draft an email to my team about the outage"),
            Writing
        );
        assert_eq!(
            classify_request_type("what is the capital of France?"),
            FactualLookup
        );
        assert_eq!(classify_request_type("hello there"), General);
    }

    #[test]
    fn request_type_round_trips_through_string() {
        for rt in [
            RequestType::CodeGeneration,
            RequestType::CodeUnderstanding,
            RequestType::TechnicalDesign,
            RequestType::AnalyticalReasoning,
            RequestType::Writing,
            RequestType::FactualLookup,
            RequestType::General,
        ] {
            assert_eq!(RequestType::from_wire(rt.as_str()), Some(rt));
        }
        assert_eq!(RequestType::from_wire("nonsense"), None);
    }

    // --- feedback signal ---

    #[test]
    fn signal_matches_reference() {
        assert_eq!(signal("perfect, that worked. thanks!"), Some(1.0));
        assert_eq!(signal("that's wrong, try again"), Some(0.0));
        assert_eq!(signal("now add error handling for missing files"), None);
        // negative wins a mixed message
        assert_eq!(signal("thanks but that's wrong"), Some(0.0));
    }

    // --- scoring primitives ---

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
            cold_start_prior(1, &[], RequestType::AnalyticalReasoning),
            0.5
        );
    }

    #[test]
    fn update_cell_matches_reference() {
        let c = update_cell(
            Cell {
                quality_mean: 0.5,
                samples: 0,
            },
            1.0,
        );
        assert_eq!(c.quality_mean, 1.0);
        assert_eq!(c.samples, 1);
        let c = update_cell(c, 0.0);
        assert!((c.quality_mean - 0.5).abs() < 1e-9);
        assert_eq!(c.samples, 2);
        let c = update_cell(
            Cell {
                quality_mean: 0.9,
                samples: MAX_SAMPLES,
            },
            0.9,
        );
        assert_eq!(c.samples, MAX_SAMPLES);
    }

    #[test]
    fn beta_sample_stays_in_unit_interval() {
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..1000 {
            let x = beta_sample(2.0, 5.0, &mut rng);
            assert!((0.0..=1.0).contains(&x), "sample out of range: {x}");
        }
        // degenerate params fall back to the mean, not NaN
        assert!(beta_sample(0.0, 0.0, &mut rng).is_finite());
    }

    // --- Thompson tier selection ---

    #[test]
    fn thompson_converges_to_the_learned_best_tier() {
        // All three tiers are well-sampled for code generation: Tier1 excellent, the others
        // poor. Once every arm's posterior is tight (no wide unexplored arm left to gamble
        // on), all-quality Thompson picks the learned best on every draw.
        let mut cells = CellMap::new();
        cells.insert(
            (Tier::Tier1, RequestType::CodeGeneration),
            Cell {
                quality_mean: 0.99,
                samples: MAX_SAMPLES,
            },
        );
        for tier in [Tier::Tier2, Tier::Tier3] {
            cells.insert(
                (tier, RequestType::CodeGeneration),
                Cell {
                    quality_mean: 0.05,
                    samples: MAX_SAMPLES,
                },
            );
        }
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..200 {
            let tier = pick_model_thompson(&cells, RequestType::CodeGeneration, 1.0, 0.0, &mut rng);
            assert_eq!(tier, Tier::Tier1);
        }
    }

    #[test]
    fn thompson_explores_a_wide_unlearned_arm() {
        // The flip side of convergence: with the best arm only *mildly* learned and a rival
        // arm still unexplored (wide posterior), exploration must sometimes pick the rival —
        // this is what generates the feedback that eventually tightens it.
        let mut cells = CellMap::new();
        cells.insert(
            (Tier::Tier1, RequestType::CodeGeneration),
            Cell {
                quality_mean: 0.7,
                samples: 8,
            },
        );
        let mut rng = StdRng::seed_from_u64(1);
        let mut distinct = std::collections::HashSet::new();
        for _ in 0..200 {
            distinct.insert(pick_model_thompson(
                &cells,
                RequestType::CodeGeneration,
                1.0,
                0.0,
                &mut rng,
            ));
        }
        assert!(
            distinct.len() > 1,
            "expected exploration across arms, got {distinct:?}"
        );
    }

    #[test]
    fn thompson_all_cost_prefers_the_cheapest_tier() {
        // No learning; pure cost weight ⇒ the cheapest tier (Tier3) always wins.
        let cells = CellMap::new();
        let mut rng = StdRng::seed_from_u64(7);
        for _ in 0..200 {
            let tier = pick_model_thompson(&cells, RequestType::General, 0.0, 1.0, &mut rng);
            assert_eq!(tier, Tier::Tier3);
        }
    }

    #[test]
    fn classify_returns_valid_tier_and_request_type() {
        let cells = CellMap::new();
        let mut rng = StdRng::seed_from_u64(3);
        let (tier, rt) = classify(
            "write a python function that sorts a list",
            "anthropic",
            &cells,
            &mut rng,
        );
        assert_eq!(rt, RequestType::CodeGeneration);
        assert!(matches!(tier, Tier::Tier1 | Tier::Tier2 | Tier::Tier3));
    }
}
#[cfg(test)]
mod request_classifier_tests {
    use super::*;
    use std::sync::Arc;

    /// Always errors — exercises the router's error fallback.
    struct FailingClassifier;
    #[async_trait]
    impl RequestClassifier for FailingClassifier {
        fn name(&self) -> &str {
            "failing"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Inference("boom".into()))
        }
    }

    /// Always returns a fixed verdict — lets a test drive the confidence branch.
    struct FixedClassifier(Classification);
    #[async_trait]
    impl RequestClassifier for FixedClassifier {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            Ok(self.0)
        }
    }

    /// Sleeps past any short timeout before answering.
    struct SlowClassifier;
    #[async_trait]
    impl RequestClassifier for SlowClassifier {
        fn name(&self) -> &str {
            "slow"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(regex_classification("write a python function"))
        }
    }

    fn verdict(rt: RequestType, confidence: f32) -> Classification {
        Classification {
            request_type: rt,
            complexity: 2,
            confidence,
        }
    }

    #[tokio::test]
    async fn regex_classifier_matches_the_function_and_uses_fixed_values() {
        let classifier = RegexClassifier;
        let c = classifier
            .classify(&ClassifyInput::query_only("write a python script to parse csv"))
            .await
            .unwrap();
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(c.complexity, REGEX_COMPLEXITY);
        assert_eq!(c.confidence, REGEX_CONFIDENCE);
    }

    #[tokio::test]
    async fn runtime_uses_the_backend_when_confident() {
        let runtime = ClassifierRuntime::new(
            Arc::new(FixedClassifier(verdict(RequestType::Writing, 0.9))),
            0.55,
        );
        let (c, fell_back) = runtime
            .classify_or_fallback(&ClassifyInput::query_only("rewrite this sentence"))
            .await;
        assert_eq!(c.request_type, RequestType::Writing);
        assert!(!fell_back);
        assert_eq!(runtime.stats().decisions(), 1);
        assert_eq!(runtime.stats().fallbacks(), 0);
    }

    #[tokio::test]
    async fn runtime_falls_back_on_low_confidence() {
        let runtime = ClassifierRuntime::new(
            Arc::new(FixedClassifier(verdict(RequestType::Writing, 0.2))),
            0.55,
        );
        let input = ClassifyInput::query_only("write a python function to sort a list");
        let (c, fell_back) = runtime.classify_or_fallback(&input).await;
        assert!(fell_back);
        // The safe default is exactly the regex verdict for the same query.
        assert_eq!(c, regex_classification(input.query));
        assert_eq!(runtime.stats().fallbacks(), 1);
        assert!((runtime.stats().fallback_rate() - 1.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn runtime_falls_back_on_backend_error() {
        let runtime = ClassifierRuntime::new(Arc::new(FailingClassifier), 0.0);
        let (c, fell_back) = runtime
            .classify_or_fallback(&ClassifyInput::query_only("what is the capital of France?"))
            .await;
        assert!(fell_back);
        assert_eq!(c.request_type, RequestType::FactualLookup);
        assert_eq!(runtime.name(), "failing");
    }

    #[tokio::test]
    async fn timeout_classifier_surfaces_a_timeout_that_falls_back() {
        let slow: Arc<dyn RequestClassifier> = Arc::new(SlowClassifier);
        let timed = Arc::new(TimeoutClassifier::new(slow, Duration::from_millis(5)));
        let runtime = ClassifierRuntime::new(timed, 0.0);
        let (_, fell_back) = runtime
            .classify_or_fallback(&ClassifyInput::query_only("write a python function"))
            .await;
        assert!(fell_back, "a timed-out backend must take the safe default");
        assert_eq!(runtime.stats().fallbacks(), 1);
    }

    #[test]
    fn backend_kind_parses_labels_and_defaults_unknown_to_none() {
        assert_eq!(BackendKind::parse(""), Some(BackendKind::Regex));
        assert_eq!(BackendKind::parse("ReGeX"), Some(BackendKind::Regex));
        assert_eq!(BackendKind::parse("local"), Some(BackendKind::Local));
        assert_eq!(BackendKind::parse(" hosted "), Some(BackendKind::Hosted));
        assert_eq!(BackendKind::parse("gpt-9000"), None);
    }

    #[tokio::test]
    async fn build_with_default_config_is_the_regex_baseline() {
        let cfg = ClassifierConfig::default();
        let runtime = ClassifierRuntime::new(
            build_request_classifier(&cfg, reqwest::Client::new()),
            cfg.low_confidence,
        );
        assert_eq!(runtime.name(), "regex");
    }

    #[tokio::test]
    async fn build_hosted_without_endpoint_degrades_to_regex() {
        let cfg = ClassifierConfig {
            backend: BackendKind::Hosted,
            endpoint: String::new(),
            ..ClassifierConfig::default()
        };
        let classifier = build_request_classifier(&cfg, reqwest::Client::new());
        assert_eq!(classifier.name(), "regex");
    }

    #[tokio::test]
    async fn build_local_uses_the_embedded_model() {
        // The embedded asset is present, so `local` builds a working, non-regex backend.
        let cfg = ClassifierConfig {
            backend: BackendKind::Local,
            timeout: Duration::ZERO,
            ..ClassifierConfig::default()
        };
        let classifier = build_request_classifier(&cfg, reqwest::Client::new());
        assert_eq!(classifier.name(), "local");
    }
}
