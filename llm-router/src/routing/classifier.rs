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
    let tier = classify_typed(request_type, cells, rng);
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
// 4. Pluggable request classifier — `RequestClassifier`
// --------------------------------------------------------------------------
//
// `classify_request_type` answers from the query text alone with keyword votes. The trait below
// is the seam that lets a different decision model (a local model or a hosted API) answer the
// same question, with the regex as the default and as the fallback for every failure mode.
//
// ## Contract
//
// * **`request_type`** — the router's [`RequestType`]. Same label set as the regex.
// * **`complexity`** — 1..=5 on a documented rubric ([`estimate_complexity`]). Reported with the
//   decision; the tier bandit is still keyed on `(provider, tier, request_type)`, so complexity
//   does not change tier selection today (no feedback simulation is needed).
// * **`confidence`** — in `[0, 1]`: the backend's probability for the chosen `request_type`
//   (local: temperature-scaled posterior; hosted: the model's self-report, clamped). Regex
//   returns fixed values (see [`RegexClassifier`]) because a vote count is not a probability.
//
// ## Failure and low-confidence behaviour ([`GuardedClassifier`])
//
// * backend `Err` or timeout -> the regex answer, counted as a fallback;
// * `confidence < min_confidence` -> the *safe default* `RequestType::General` (the router's
//   neutral cell), counted as a low-confidence fallback — never a confident wrong guess.
//
// Determinism: the regex and local backends are pure functions of `(query, context)`.

/// What the classifier is asked about. `context` is optional supporting text (e.g. the code or
/// file the request refers to); the regex baseline ignores it, learned backends may use it.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// A classifier verdict. See the module notes for the meaning of each field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// 1 (trivial) ..= 5 (multi-constraint / multi-step engineering or analysis).
    pub complexity: u8,
    /// Probability-like score in `[0, 1]` for `request_type`.
    pub confidence: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("classifier backend failed: {0}")]
    Backend(String),
    #[error("classifier timed out")]
    Timeout,
    #[error("classifier returned invalid output: {0}")]
    InvalidOutput(String),
}

/// A request-type decision model. Implementations must be cheap to call concurrently.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// Fixed complexity the regex baseline reports (it cannot measure complexity).
pub const REGEX_COMPLEXITY: u8 = 3;
/// Fixed confidence when at least one category pattern matched.
pub const REGEX_CONFIDENCE_MATCHED: f32 = 0.5;
/// Fixed confidence when nothing matched and `General` is just the default.
pub const REGEX_CONFIDENCE_DEFAULT: f32 = 0.25;

/// The default classifier: [`classify_request_type`] unchanged. Complexity is fixed at
/// [`REGEX_COMPLEXITY`]; confidence is [`REGEX_CONFIDENCE_MATCHED`] if any pattern matched, else
/// [`REGEX_CONFIDENCE_DEFAULT`]. Infallible and never touches the network.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let request_type = classify_request_type(input.query);
        let matched = request_type != RequestType::General
            || CATEGORY_PATTERNS
                .iter()
                .any(|(_, pats)| pats.iter().any(|p| p.is_match(input.query)));
        Ok(Classification {
            request_type,
            complexity: REGEX_COMPLEXITY,
            confidence: if matched {
                REGEX_CONFIDENCE_MATCHED
            } else {
                REGEX_CONFIDENCE_DEFAULT
            },
        })
    }
}

/// Complexity rubric (1..=5), a deterministic function of the text:
///
/// | points | signal |
/// |---|---|
/// | 0-3 | query length: <=8 words 0, <=25 words 1, <=60 words 2, longer 3 |
/// | +1 | supporting context longer than 150 words, or any code fence |
/// | +1 | two or more sequencing markers (first/then/finally/step/numbered list) |
/// | +1 / +2 | one / two-or-more constraint or scale markers (must, without, at most, per second, multi-region, trade-off, thread-safe, ...) |
///
/// `complexity = 1 + points / 2`, clamped to 5 (0-1 pts -> 1, 2-3 -> 2, 4-5 -> 3, 6-7 -> 4, 8+ -> 5).
pub fn estimate_complexity(query: &str, context: Option<&str>) -> u8 {
    const SEQUENCING: [&str; 6] = ["first", "then", "finally", "step", "after that", "1."];
    const CONSTRAINT: [&str; 22] = [
        "must",
        "without",
        "at most",
        "at least",
        "exactly",
        "per second",
        "per day",
        "scale",
        "distributed",
        "concurren",
        "thread",
        "secur",
        "trade-off",
        "tradeoff",
        "multi-region",
        "benchmark",
        "fault",
        "latency",
        "throughput",
        "consisten",
        "sharding",
        "stress test",
    ];
    let q = query.to_lowercase();
    let words = q.split_whitespace().count();
    let mut points = match words {
        0..=8 => 0,
        9..=25 => 1,
        26..=60 => 2,
        _ => 3,
    };
    let ctx_words = context.map_or(0, |c| c.split_whitespace().count());
    if ctx_words > 150 || q.contains("```") || context.is_some_and(|c| c.contains("```")) {
        points += 1;
    }
    if SEQUENCING.iter().filter(|m| q.contains(**m)).count() >= 2 {
        points += 1;
    }
    points += match CONSTRAINT.iter().filter(|m| q.contains(**m)).count() {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    (1 + points / 2).min(5) as u8
}

/// Counters for how often the guard had to step in. Cheap to read for metrics.
#[derive(Debug, Default)]
pub struct ClassifierStats {
    calls: AtomicU64,
    errors: AtomicU64,
    timeouts: AtomicU64,
    low_confidence: AtomicU64,
}

/// Point-in-time copy of [`ClassifierStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub calls: u64,
    pub errors: u64,
    pub timeouts: u64,
    pub low_confidence: u64,
}

impl StatsSnapshot {
    /// Fraction of calls that did not use the backend's own answer.
    pub fn fallback_rate(&self) -> f64 {
        if self.calls == 0 {
            return 0.0;
        }
        (self.errors + self.timeouts + self.low_confidence) as f64 / self.calls as f64
    }
}

impl ClassifierStats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            low_confidence: self.low_confidence.load(Ordering::Relaxed),
        }
    }
}

/// Wraps any backend with a timeout, regex fallback on failure, and the low-confidence safe
/// default. This is what the router and `classifier_eval` both hold, so they exercise one path.
pub struct GuardedClassifier {
    inner: Arc<dyn RequestClassifier>,
    timeout: Duration,
    min_confidence: f32,
    stats: ClassifierStats,
}

impl GuardedClassifier {
    pub fn new(inner: Arc<dyn RequestClassifier>, timeout: Duration, min_confidence: f32) -> Self {
        Self {
            inner,
            timeout,
            min_confidence: min_confidence.clamp(0.0, 1.0),
            stats: ClassifierStats::default(),
        }
    }
    pub fn stats(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }
}

#[async_trait]
impl RequestClassifier for GuardedClassifier {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = match tokio::time::timeout(self.timeout, self.inner.classify(input)).await {
            Ok(r) => r,
            Err(_) => {
                self.stats.timeouts.fetch_add(1, Ordering::Relaxed);
                Err(ClassifyError::Timeout)
            }
        };
        match outcome {
            Ok(c) if c.confidence.is_finite() && c.confidence >= self.min_confidence => Ok(c),
            Ok(c) => {
                self.stats.low_confidence.fetch_add(1, Ordering::Relaxed);
                Ok(Classification {
                    request_type: RequestType::General,
                    ..c
                })
            }
            Err(e) => {
                if !matches!(e, ClassifyError::Timeout) {
                    self.stats.errors.fetch_add(1, Ordering::Relaxed);
                }
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = %self.inner.name(), error = %e,
                    "classifier backend failed; falling back to the regex classifier"
                );
                RegexClassifier.classify(input).await
            }
        }
    }
}

/// [`classify`] with the request type already decided (by a [`RequestClassifier`]); only the
/// tier is Thompson-sampled. `classify` is this plus the regex type.
pub fn classify_typed<R: Rng + ?Sized>(
    request_type: RequestType,
    cells: &CellMap,
    rng: &mut R,
) -> Tier {
    pick_model_thompson(cells, request_type, DEFAULT_W_QUALITY, DEFAULT_W_COST, rng)
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

    // --- RequestClassifier / GuardedClassifier ---

    struct Scripted(Result<Classification, fn() -> ClassifyError>, Duration);
    #[async_trait]
    impl RequestClassifier for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(self.1).await;
            self.0.map_err(|f| f())
        }
    }
    fn verdict(rt: RequestType, confidence: f32) -> Classification {
        Classification {
            request_type: rt,
            complexity: 4,
            confidence,
        }
    }
    fn guard(inner: Scripted, min: f32) -> GuardedClassifier {
        GuardedClassifier::new(Arc::new(inner), Duration::from_millis(50), min)
    }
    const Q: ClassifyInput<'static> = ClassifyInput {
        query: "write me a Python sort function",
        context: None,
    };

    #[tokio::test]
    async fn regex_classifier_wraps_the_baseline_with_fixed_values() {
        let c = RegexClassifier.classify(&Q).await.unwrap();
        assert_eq!(c.request_type, classify_request_type(Q.query));
        assert_eq!(
            (c.complexity, c.confidence),
            (REGEX_COMPLEXITY, REGEX_CONFIDENCE_MATCHED)
        );
        let g = RegexClassifier
            .classify(&ClassifyInput {
                query: "hmm",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(
            (g.request_type, g.confidence),
            (RequestType::General, REGEX_CONFIDENCE_DEFAULT)
        );
    }

    #[tokio::test]
    async fn guard_passes_a_confident_answer_through() {
        let g = guard(
            Scripted(Ok(verdict(RequestType::Writing, 0.9)), Duration::ZERO),
            0.4,
        );
        assert_eq!(
            g.classify(&Q).await.unwrap(),
            verdict(RequestType::Writing, 0.9)
        );
        assert_eq!(g.stats().fallback_rate(), 0.0);
    }

    #[tokio::test]
    async fn guard_falls_back_to_regex_on_error_and_counts_it() {
        let g = guard(
            Scripted(
                Err(|| ClassifyError::Backend("boom".into())),
                Duration::ZERO,
            ),
            0.0,
        );
        let c = g.classify(&Q).await.unwrap();
        assert_eq!(c.request_type, classify_request_type(Q.query));
        let s = g.stats();
        assert_eq!((s.calls, s.errors, s.timeouts), (1, 1, 0));
    }

    #[tokio::test]
    async fn guard_falls_back_to_regex_on_timeout_and_counts_it() {
        let g = guard(
            Scripted(
                Ok(verdict(RequestType::Writing, 0.99)),
                Duration::from_millis(500),
            ),
            0.0,
        );
        let c = g.classify(&Q).await.unwrap();
        assert_eq!(c.request_type, classify_request_type(Q.query));
        let s = g.stats();
        assert_eq!((s.timeouts, s.errors), (1, 0));
        assert_eq!(s.fallback_rate(), 1.0);
    }

    #[tokio::test]
    async fn low_confidence_routes_to_the_safe_default_and_counts_as_fallback() {
        let g = guard(
            Scripted(
                Ok(verdict(RequestType::CodeGeneration, 0.3)),
                Duration::ZERO,
            ),
            0.5,
        );
        let c = g.classify(&Q).await.unwrap();
        assert_eq!(c.request_type, RequestType::General);
        assert_eq!(c.complexity, 4, "complexity is kept");
        assert_eq!(g.stats().low_confidence, 1);
    }

    #[tokio::test]
    async fn non_finite_confidence_is_treated_as_low() {
        let g = guard(
            Scripted(Ok(verdict(RequestType::Writing, f32::NAN)), Duration::ZERO),
            0.1,
        );
        assert_eq!(
            g.classify(&Q).await.unwrap().request_type,
            RequestType::General
        );
    }

    #[test]
    fn complexity_rubric_is_bounded_and_monotonic() {
        let trivial = estimate_complexity("fix typo", None);
        let hard = estimate_complexity(
            "First design a distributed rate limiter, then benchmark it, and it must be fault tolerant at 50k requests per second with strong consistency across regions, without a single point of failure",
            Some(&"context ".repeat(200)),
        );
        assert_eq!(trivial, 1);
        assert!(hard >= 4 && hard <= 5, "hard={hard}");
        assert_eq!(
            estimate_complexity("same", None),
            estimate_complexity("same", None)
        );
    }
}
