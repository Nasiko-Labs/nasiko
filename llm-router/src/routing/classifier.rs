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

    /// Every request type in canonical order. This order is a contract shared by
    /// [`regex_votes`], the local classifier's weights file (`classes`), and the hosted
    /// classifier's label letters (A–G) — never reorder it without retraining.
    pub const ALL: [RequestType; 7] = [
        RequestType::CodeGeneration,
        RequestType::CodeUnderstanding,
        RequestType::TechnicalDesign,
        RequestType::AnalyticalReasoning,
        RequestType::Writing,
        RequestType::FactualLookup,
        RequestType::General,
    ];

    /// Position of `self` in [`RequestType::ALL`].
    pub fn index(self) -> usize {
        match self {
            RequestType::CodeGeneration => 0,
            RequestType::CodeUnderstanding => 1,
            RequestType::TechnicalDesign => 2,
            RequestType::AnalyticalReasoning => 3,
            RequestType::Writing => 4,
            RequestType::FactualLookup => 5,
            RequestType::General => 6,
        }
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
    pick_model_thompson_with_shift(
        cells,
        request_type,
        w_quality,
        w_cost,
        [0.0; 3],
        [true; 3],
        rng,
    )
}

/// [`pick_model_thompson`] with a per-tier additive shift of the cold-start prior
/// (`[Tier1, Tier2, Tier3]`, the shifted prior clamped to `[0.05, 0.95]`) and an arm mask.
///
/// A Beta sample is drawn for **every** arm, masked or not, so the RNG stream is identical to
/// the unshifted selection; masked arms are only skipped in the argmax. With a zero shift and
/// every arm allowed this is bit-identical to [`pick_model_thompson`] (which delegates here).
/// An all-`false` mask is ignored rather than leaving no arm to pick.
pub fn pick_model_thompson_with_shift<R: Rng + ?Sized>(
    cells: &CellMap,
    request_type: RequestType,
    w_quality: f64,
    w_cost: f64,
    prior_shift: [f64; 3],
    allowed: [bool; 3],
    rng: &mut R,
) -> Tier {
    let allowed = if allowed.iter().any(|a| *a) {
        allowed
    } else {
        [true; 3]
    };
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
    for (i, arm) in TIER_ARMS.iter().enumerate() {
        let prior = (cold_start_prior(arm.quality_tier, arm.strengths, request_type)
            + prior_shift[i])
            .clamp(0.05, 0.95);
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
        if allowed[i] && score > best_score {
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

// --------------------------------------------------------------------------
// 5. Pluggable request classifier (Level 3 seam)
// --------------------------------------------------------------------------

/// Input to a [`RequestClassifier`].
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    /// The latest user message — what the request is asking for.
    pub query: &'a str,
    /// Optional bounded context: attached material (code, notes, logs) or recent
    /// conversation. Backends may ignore it (the regex does).
    pub context: Option<&'a str>,
}

/// A classifier's verdict on one request.
///
/// - `request_type`: one of the seven [`RequestType`]s — the label the bandit learns on.
/// - `complexity`: 1–5 on the rubric *1 = trivial single operation; 2 = straightforward;
///   3 = multi-step with limited constraints; 4 = substantial reasoning or design;
///   5 = intricate cross-component reasoning and validation*.
/// - `confidence`: a calibrated estimate of P(`request_type` is correct), in `[0, 1]`.
///   It describes the type label only; complexity carries no confidence of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

impl Classification {
    /// Build a verdict, clamping `complexity` to `1..=5` and `confidence` to `[0, 1]`
    /// (a NaN confidence becomes `0.0`), so no backend can hand the router an out-of-range
    /// value.
    pub fn new(request_type: RequestType, complexity: u8, confidence: f32) -> Self {
        let confidence = if confidence.is_nan() {
            0.0
        } else {
            confidence.clamp(0.0, 1.0)
        };
        Self {
            request_type,
            complexity: complexity.clamp(1, 5),
            confidence,
        }
    }
}

/// Why a classifier could not produce a verdict. Every variant ends in the regex fallback
/// (see [`GuardedClassifier`]); none ever fails a request.
#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("classifier timed out after {0:?}")]
    Timeout(Duration),
    /// Network failure, a non-success HTTP status, or a backend that is not configured.
    #[error("classifier backend unavailable: {0}")]
    Unavailable(String),
    #[error("classifier response was not understood: {0}")]
    InvalidResponse(String),
    /// Model load or inference failure.
    #[error("classifier model error: {0}")]
    Model(String),
}

/// Decides a request's type, complexity and confidence at routing Level 3.
///
/// The router holds one behind `Arc<dyn RequestClassifier>`, built from config by
/// `build_request_classifier`; `examples/classifier_eval.rs` builds it the same way, so the
/// eval exercises the router's code path. Async because hosted backends make network calls.
#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Short stable backend name for logs and the eval: `"regex"`, `"local"`, `"hosted"`, …
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// [`RegexClassifier`]'s fixed complexity. The regex carries no complexity signal, so it
/// reports the rubric midpoint — which also makes complexity-aware tier routing an exact
/// no-op on the regex path ([`select_tier`]).
pub const REGEX_COMPLEXITY: u8 = 3;

/// [`RegexClassifier`]'s confidence when at least one pattern fired: the regex's measured
/// request-type accuracy on that case of our validation split, so it is calibrated in
/// aggregate by construction (0.76 = 45/59 on the split described in
/// `training/request_classifier/DATASHEET.md`).
pub const REGEX_CONFIDENCE_MATCHED: f32 = 0.76;

/// [`RegexClassifier`]'s confidence when no pattern fired and it defaulted to `General`:
/// the regex's measured accuracy on that case of our validation split (0.12 = 18/154).
pub const REGEX_CONFIDENCE_DEFAULTED: f32 = 0.12;

/// Per-category regex vote counts, indexed by [`RequestType::ALL`] (`General` is always 0).
/// The same votes [`classify_request_type`] counts; exposed so the local classifier can use
/// them as features and the regex can report whether anything fired.
pub(crate) fn regex_votes(text: &str) -> [u8; 7] {
    let mut votes = [0u8; 7];
    for (rt, pats) in CATEGORY_PATTERNS.iter() {
        let n = pats.iter().filter(|p| p.is_match(text)).count();
        votes[rt.index()] = u8::try_from(n).unwrap_or(u8::MAX);
    }
    votes
}

/// The existing regex vote counter behind the [`RequestClassifier`] trait — the default
/// backend. Its request type is exactly [`classify_request_type`]'s; complexity is the fixed
/// [`REGEX_COMPLEXITY`]; confidence is [`REGEX_CONFIDENCE_MATCHED`] or
/// [`REGEX_CONFIDENCE_DEFAULTED`]. Context is ignored. Never fails, no network.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegexClassifier;

impl RegexClassifier {
    /// The regex verdict with its fixed values. This is also *the* fallback every other path
    /// uses (the guard, the router, the eval), so the regex's documented values live in one
    /// place.
    pub fn classify_sync(query: &str) -> Classification {
        let fired = regex_votes(query).iter().any(|v| *v > 0);
        let confidence = if fired {
            REGEX_CONFIDENCE_MATCHED
        } else {
            REGEX_CONFIDENCE_DEFAULTED
        };
        Classification::new(classify_request_type(query), REGEX_COMPLEXITY, confidence)
    }
}

#[async_trait::async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(Self::classify_sync(input.query))
    }
}

/// Process-wide classifier counters, shared via `Arc` between the [`GuardedClassifier`] and
/// any composite backend (a cascade counts its escalations here). Relaxed atomics: these are
/// telemetry, not synchronization.
#[derive(Debug, Default)]
pub struct ClassifierStats {
    pub calls: AtomicU64,
    pub fallback_error: AtomicU64,
    pub fallback_timeout: AtomicU64,
    /// Set at build time when the configured backend could not be constructed.
    pub fallback_load: AtomicU64,
    pub escalations: AtomicU64,
    pub escalation_failures: AtomicU64,
}

/// A point-in-time copy of [`ClassifierStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClassifierStatsSnapshot {
    pub calls: u64,
    pub fallback_error: u64,
    pub fallback_timeout: u64,
    pub fallback_load: u64,
    pub escalations: u64,
    pub escalation_failures: u64,
}

impl ClassifierStatsSnapshot {
    /// Every fallback to the regex, whatever the reason.
    pub fn fallbacks(&self) -> u64 {
        self.fallback_error + self.fallback_timeout + self.fallback_load
    }
}

impl ClassifierStats {
    pub fn snapshot(&self) -> ClassifierStatsSnapshot {
        ClassifierStatsSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            fallback_error: self.fallback_error.load(Ordering::Relaxed),
            fallback_timeout: self.fallback_timeout.load(Ordering::Relaxed),
            fallback_load: self.fallback_load.load(Ordering::Relaxed),
            escalations: self.escalations.load(Ordering::Relaxed),
            escalation_failures: self.escalation_failures.load(Ordering::Relaxed),
        }
    }
}

/// Wraps the configured backend with a time budget and the regex fallback: on an `Err` or a
/// timeout it logs why (never the query text), counts the fallback, and returns
/// [`RegexClassifier::classify_sync`]. It therefore **never returns `Err`**.
///
/// `tokio::time::timeout` can only interrupt a future at an `.await`; it cannot pre-empt
/// synchronous CPU work. The in-process local backend runs in microseconds and never
/// yields, so for it the guard's value is the error fallback; the budget bounds async
/// (network) backends.
pub struct GuardedClassifier {
    primary: Arc<dyn RequestClassifier>,
    timeout: Duration,
    stats: Arc<ClassifierStats>,
}

impl GuardedClassifier {
    pub fn new(
        primary: Arc<dyn RequestClassifier>,
        timeout: Duration,
        stats: Arc<ClassifierStats>,
    ) -> Self {
        Self {
            primary,
            timeout,
            stats,
        }
    }

    /// The shared counters (calls, fallbacks by reason, escalations).
    pub fn stats(&self) -> &Arc<ClassifierStats> {
        &self.stats
    }
}

#[async_trait::async_trait]
impl RequestClassifier for GuardedClassifier {
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(self.timeout, self.primary.classify(input)).await {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => {
                self.stats.fallback_error.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = self.primary.name(),
                    error = %e,
                    query_chars = input.query.chars().count(),
                    "request classifier failed; falling back to regex"
                );
                Ok(RegexClassifier::classify_sync(input.query))
            }
            Err(_) => {
                self.stats.fallback_timeout.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = self.primary.name(),
                    timeout_ms = self.timeout.as_millis() as u64,
                    query_chars = input.query.chars().count(),
                    "request classifier timed out; falling back to regex"
                );
                Ok(RegexClassifier::classify_sync(input.query))
            }
        }
    }
}

/// Opt-in complexity-aware tier routing (`CLASSIFIER_COMPLEXITY_ROUTING`). [`Self::OFF`]
/// makes [`select_tier`] identical to [`pick_model_thompson`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexityRouting {
    pub enabled: bool,
    /// Minimum confidence for the hard guardrail (arm exclusion) to apply.
    pub guard_confidence: f32,
}

impl ComplexityRouting {
    pub const OFF: Self = Self {
        enabled: false,
        guard_confidence: 0.6,
    };
}

/// Pick a tier for a classification: Thompson sampling over the `(tier, request_type)` cells,
/// with an opt-in complexity prior shift and guardrail.
///
/// | complexity | prior shift T1 / T2 / T3 | guardrail (if `confidence ≥ guard_confidence`) |
/// |---|---|---|
/// | 1–2 | −0.10 / 0 / +0.15 | Tier 1 excluded |
/// | 3 | 0 / 0 / 0 | none |
/// | 4–5 | +0.15 / 0 / −0.15 | Tier 3 excluded |
///
/// Disabled, or at complexity 3 (always the case for the regex), this is exactly
/// [`pick_model_thompson`]. The bandit key stays `(tier, request_type)`: keying learned cells
/// on complexity would change the persisted `router_quality_cells` schema (a migration, out
/// of scope), so complexity acts only as a cold-start prior and a constraint — learned cells
/// still dominate as evidence accumulates.
pub fn select_tier<R: Rng + ?Sized>(
    cells: &CellMap,
    c: &Classification,
    cx: ComplexityRouting,
    rng: &mut R,
) -> Tier {
    let (shift, guard) = match (cx.enabled, c.complexity) {
        (false, _) | (true, 3) => ([0.0; 3], [true; 3]),
        (true, 0..=2) => ([-0.10, 0.0, 0.15], [false, true, true]),
        (true, _) => ([0.15, 0.0, -0.15], [true, true, false]),
    };
    let allowed = if cx.enabled && c.confidence >= cx.guard_confidence {
        guard
    } else {
        [true; 3]
    };
    pick_model_thompson_with_shift(
        cells,
        c.request_type,
        DEFAULT_W_QUALITY,
        DEFAULT_W_COST,
        shift,
        allowed,
        rng,
    )
}

/// Round a probability to 4 decimal places — the precision every backend reports, so repeated
/// runs diff cleanly.
pub(crate) fn round_confidence(p: f64) -> f32 {
    ((p * 1e4).round() / 1e4) as f32
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

    // --- pluggable classifier (Level 3 seam) ---

    /// Varied probes: the router_e2e and reference examples plus misses, empties and noise.
    const CORPUS: [&str; 54] = [
        "what is the capital of France?",
        "hello there",
        "build me a python script that parses CSV",
        "how should I design this API?",
        "write me a Python sort function",
        "explain what this function does",
        "calculate the probability that it rains tomorrow",
        "draft an email to my team about the outage",
        "fix this bug in the parser",
        "refactor the module and add error handling",
        "what does this function do",
        "how does the code work",
        "walk me through this code",
        "what is this code doing",
        "system design for a chat app",
        "architecture review of our queue",
        "design a schema for orders",
        "what are the trade-offs of sharding",
        "solve 12 * 7",
        "prove that sqrt 2 is irrational",
        "how many primes are there below 100",
        "what's the average of 3, 4 and 5",
        "compose a poem about autumn",
        "rewrite this paragraph to be friendlier",
        "make this sound more formal",
        "write an article about rust",
        "define entropy",
        "who was Ada Lovelace",
        "when is the next leap year",
        "how many moons are there around Jupiter",
        "Fix typo in this Python comment",
        "What does Option::take() do in Rust?",
        "Implement a parser for CSV",
        "Summarize these release notes",
        "",
        "   ",
        "\u{1F44D}",
        "?!",
        "explain why the cache is slow and how we should design the architecture",
        "write a function, then explain how it works, then calculate its complexity",
        "Hola, ¿puedes ayudarme?",
        "thanks!",
        "ok",
        "implement an endpoint that returns the user profile",
        "generate a class for the payment module",
        "write the SQL to find duplicate rows",
        "draft a blog post and compose an email",
        "explain how the probability distribution is calculated",
        "what is the result of 2+2",
        "trade-off between architecture options for the api design",
        "lol what",
        "Please help me, I need to write a script",
        "explain what happens when you prove a theorem",
        "where were the 2012 olympics",
    ];

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    #[test]
    fn regex_classifier_matches_classify_request_type_on_corpus() {
        for q in CORPUS {
            let got = block_on(RegexClassifier.classify(&ClassifyInput {
                query: q,
                context: Some("context is ignored by the regex"),
            }))
            .expect("regex never fails");
            assert_eq!(got.request_type, classify_request_type(q), "query {q:?}");
        }
    }

    #[test]
    fn regex_votes_argmax_matches_classify_request_type() {
        for q in CORPUS {
            let votes = regex_votes(q);
            let mut best = RequestType::General;
            let mut best_votes = 0u8;
            for rt in RequestType::ALL {
                if votes[rt.index()] > best_votes {
                    best_votes = votes[rt.index()];
                    best = rt;
                }
            }
            assert_eq!(votes[RequestType::General.index()], 0);
            assert_eq!(best, classify_request_type(q), "query {q:?}");
        }
    }

    #[test]
    fn regex_classifier_reports_documented_fixed_complexity_and_confidence() {
        let matched = RegexClassifier::classify_sync("draft an email to my team");
        assert_eq!(matched.complexity, REGEX_COMPLEXITY);
        assert_eq!(matched.confidence, REGEX_CONFIDENCE_MATCHED);
        let defaulted = RegexClassifier::classify_sync("hello there");
        assert_eq!(defaulted.request_type, RequestType::General);
        assert_eq!(defaulted.complexity, REGEX_COMPLEXITY);
        assert_eq!(defaulted.confidence, REGEX_CONFIDENCE_DEFAULTED);
    }

    #[test]
    fn classification_new_clamps_out_of_range_and_nan() {
        let rt = RequestType::Writing;
        assert_eq!(Classification::new(rt, 0, 0.5).complexity, 1);
        assert_eq!(Classification::new(rt, 9, 0.5).complexity, 5);
        assert_eq!(Classification::new(rt, 3, -1.0).confidence, 0.0);
        assert_eq!(Classification::new(rt, 3, 2.0).confidence, 1.0);
        assert_eq!(Classification::new(rt, 3, f32::NAN).confidence, 0.0);
        assert_eq!(Classification::new(rt, 4, 0.25).confidence, 0.25);
    }

    #[test]
    fn request_type_all_order_matches_wire_names_and_index() {
        let wire: Vec<&str> = RequestType::ALL.iter().map(|rt| rt.as_str()).collect();
        assert_eq!(
            wire,
            [
                "code_generation",
                "code_understanding",
                "technical_design",
                "analytical_reasoning",
                "writing",
                "factual_lookup",
                "general"
            ]
        );
        for (i, rt) in RequestType::ALL.iter().enumerate() {
            assert_eq!(rt.index(), i);
        }
    }

    /// A backend that always fails.
    struct FailingClassifier;
    #[async_trait::async_trait]
    impl RequestClassifier for FailingClassifier {
        fn name(&self) -> &str {
            "failing"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Model("boom".into()))
        }
    }

    /// A backend slower than any test budget.
    struct SlowClassifier;
    #[async_trait::async_trait]
    impl RequestClassifier for SlowClassifier {
        fn name(&self) -> &str {
            "slow"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(Classification::new(RequestType::Writing, 5, 1.0))
        }
    }

    #[test]
    fn guarded_falls_back_to_regex_on_backend_error_and_counts_it() {
        let stats = Arc::new(ClassifierStats::default());
        let guard = GuardedClassifier::new(
            Arc::new(FailingClassifier),
            Duration::from_secs(1),
            stats.clone(),
        );
        let q = "draft an email to my team about the outage";
        let got = block_on(guard.classify(&ClassifyInput {
            query: q,
            context: None,
        }))
        .expect("guard never errs");
        assert_eq!(got, RegexClassifier::classify_sync(q));
        let snap = stats.snapshot();
        assert_eq!(
            (snap.calls, snap.fallback_error, snap.fallback_timeout),
            (1, 1, 0)
        );
        assert_eq!(guard.name(), "failing");
    }

    #[test]
    fn guarded_falls_back_to_regex_on_timeout_and_counts_it() {
        let stats = Arc::new(ClassifierStats::default());
        let guard = GuardedClassifier::new(
            Arc::new(SlowClassifier),
            Duration::from_millis(20),
            stats.clone(),
        );
        let q = "hello there";
        let got = block_on(guard.classify(&ClassifyInput {
            query: q,
            context: None,
        }))
        .expect("guard never errs");
        assert_eq!(got, RegexClassifier::classify_sync(q));
        let snap = stats.snapshot();
        assert_eq!((snap.fallback_error, snap.fallback_timeout), (0, 1));
        assert_eq!(snap.fallbacks(), 1);
    }

    // --- complexity-aware tier selection ---

    fn tier_counts(c: Classification, cx: ComplexityRouting, seeds: u64) -> [usize; 3] {
        let cells: CellMap = HashMap::new();
        let mut counts = [0usize; 3];
        for seed in 0..seeds {
            let tier = select_tier(&cells, &c, cx, &mut StdRng::seed_from_u64(seed));
            counts[match tier {
                Tier::Tier1 => 0,
                Tier::Tier2 => 1,
                Tier::Tier3 => 2,
            }] += 1;
        }
        counts
    }

    const ON: ComplexityRouting = ComplexityRouting {
        enabled: true,
        guard_confidence: 0.6,
    };

    #[test]
    fn zero_shift_equals_pick_model_thompson_for_seeded_rng() {
        let cells: CellMap = HashMap::new();
        for rt in RequestType::ALL {
            for seed in 0..500 {
                let a = pick_model_thompson(
                    &cells,
                    rt,
                    DEFAULT_W_QUALITY,
                    DEFAULT_W_COST,
                    &mut StdRng::seed_from_u64(seed),
                );
                for c in [1, 3, 5] {
                    let off = select_tier(
                        &cells,
                        &Classification::new(rt, c, 0.9),
                        ComplexityRouting::OFF,
                        &mut StdRng::seed_from_u64(seed),
                    );
                    assert_eq!(a, off, "rt {rt:?} seed {seed} c {c}");
                }
            }
        }
    }

    #[test]
    fn select_tier_with_neutral_complexity_equals_legacy_classify_tier() {
        let cells: CellMap = HashMap::new();
        for q in CORPUS {
            for seed in 0..50 {
                let (legacy, _) = classify(q, "openai", &cells, &mut StdRng::seed_from_u64(seed));
                let c = RegexClassifier::classify_sync(q);
                let new = select_tier(&cells, &c, ON, &mut StdRng::seed_from_u64(seed));
                assert_eq!(legacy, new, "query {q:?} seed {seed}");
            }
        }
    }

    #[test]
    fn confident_high_complexity_never_selects_tier3() {
        let c = Classification::new(RequestType::TechnicalDesign, 5, 0.9);
        assert_eq!(tier_counts(c, ON, 2000)[2], 0);
    }

    #[test]
    fn confident_trivial_never_selects_tier1() {
        let c = Classification::new(RequestType::CodeGeneration, 1, 0.9);
        assert_eq!(tier_counts(c, ON, 2000)[0], 0);
    }

    #[test]
    fn low_confidence_does_not_apply_guardrail() {
        let c = Classification::new(RequestType::General, 5, 0.3);
        assert!(tier_counts(c, ON, 2000)[2] > 0);
    }
}
