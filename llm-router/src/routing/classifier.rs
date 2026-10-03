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
use std::fmt;
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

// --------------------------------------------------------------------------
// Pluggable request classifier — the model-agnostic decision interface
// --------------------------------------------------------------------------

/// The text a classifier decides on: the latest user query plus optional context (surrounding
/// transcript, attached snippet, system framing). Borrowed so a call allocates nothing on the
/// hot path.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// A classifier's verdict: the request type, a complexity band, and how confident the
/// classifier is in the (request_type) label.
///
/// `confidence` is the classifier's own probability that `request_type` is correct, in
/// `[0, 1]`. It is *not* a calibrated guarantee — a caller that cares (see
/// [`ResilientClassifier`]) decides what to do with a low value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// 1–5, per [`super::request_classifier`]'s rubric (mirrors the public eval rubric).
    pub complexity: u8,
    /// In `[0, 1]`.
    pub confidence: f32,
}

/// Why a classifier could not produce a verdict. Every variant is recoverable: the router
/// treats any `Err` as "use the regex fallback and count it", never as a request failure.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassifyError {
    /// The model or endpoint is not configured or could not be loaded.
    ModelUnavailable(String),
    /// The model ran but failed (bad input, internal error).
    Inference(String),
    /// The backend exceeded its timeout.
    Timeout,
    /// The backend answered in a shape we can't trust (e.g. an unknown label).
    InvalidResponse(String),
    /// The model's top label did not clear the configured confidence floor. The caller is
    /// expected to fall back rather than act on a guess.
    LowConfidence(f32),
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassifyError::ModelUnavailable(m) => write!(f, "classifier model unavailable: {m}"),
            ClassifyError::Inference(m) => write!(f, "classifier inference failed: {m}"),
            ClassifyError::Timeout => write!(f, "classifier timed out"),
            ClassifyError::InvalidResponse(m) => {
                write!(f, "classifier returned an invalid response: {m}")
            }
            ClassifyError::LowConfidence(p) => {
                write!(f, "classifier confidence {p:.3} below the floor")
            }
        }
    }
}

impl std::error::Error for ClassifyError {}

/// A backend that maps `(query, context)` to a [`Classification`].
///
/// Implementations must be `Send + Sync` (the router holds one behind an `Arc` and calls it
/// from request handlers) and deterministic for identical input and state. Async because a
/// hosted backend makes a network call; a local backend simply doesn't await anything.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// A short, stable identifier for logs and the eval output (e.g. `"regex"`,
    /// `"local_linear"`, `"hosted"`).
    fn name(&self) -> &str;

    /// Classify one input. `Err` means "no trustworthy verdict" — never a guessed one.
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;

    /// `(fallbacks, total_calls)` when this classifier tracks a fallback path; `None`
    /// otherwise. Lets a caller report the fallback rate without downcasting. The default
    /// is `None`, which is correct for backends with no fallback of their own.
    fn fallback_stats(&self) -> Option<(u64, u64)> {
        None
    }
}

/// The out-of-the-box backend: wraps the existing keyword/regex
/// [`classify_request_type`], so behaviour is unchanged unless an operator opts into another
/// backend.
///
/// It has no complexity signal, so it reports [`REGEX_COMPLEXITY`] (the neutral midpoint) for
/// every input, and [`REGEX_CONFIDENCE`] — deliberately low, because a regex vote count is a
/// weak signal. The low confidence is also what makes the regex safe as the *fallback*: a
/// caller that gates on confidence will never prefer regex over a confident model.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

/// Complexity the regex backend reports for every input (it has no complexity signal).
pub const REGEX_COMPLEXITY: u8 = 3;
/// Confidence the regex backend reports for every input (fixed, deliberately unconfident).
pub const REGEX_CONFIDENCE: f32 = 0.5;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(Classification {
            request_type: classify_request_type(input.query),
            complexity: REGEX_COMPLEXITY,
            confidence: REGEX_CONFIDENCE,
        })
    }
}

/// Wraps a backend with a timeout, a confidence floor, and a regex fallback, counting every
/// fallback so the rate is observable.
///
/// This is where "fail closed" lives for the classifier. Any of:
/// - the backend returns `Err`,
/// - it exceeds `timeout`,
/// - it returns a verdict whose `confidence` is below `min_confidence`,
///
/// produces the regex verdict instead and increments the fallback counter. The wrapper itself
/// never returns `Err` — the regex backend cannot fail — so downstream routing always has a
/// usable answer.
///
/// When the configured backend *is* regex, the builder returns a bare [`RegexClassifier`]
/// rather than wrapping it: a fallback-to-self would double-count every low-confidence regex
/// verdict and add a pointless timeout around a synchronous computation.
pub struct ResilientClassifier {
    inner: Arc<dyn RequestClassifier>,
    fallback: RegexClassifier,
    timeout: Duration,
    min_confidence: f32,
    calls: AtomicU64,
    fallbacks: AtomicU64,
}

impl ResilientClassifier {
    /// Wrap `inner`. `min_confidence` is a floor in `[0, 1]`.
    pub fn new(inner: Arc<dyn RequestClassifier>, timeout: Duration, min_confidence: f32) -> Self {
        Self {
            inner,
            fallback: RegexClassifier,
            timeout,
            min_confidence,
            calls: AtomicU64::new(0),
            fallbacks: AtomicU64::new(0),
        }
    }

    /// `(fallbacks, total_calls)` observed so far.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.fallbacks.load(Ordering::Relaxed),
            self.calls.load(Ordering::Relaxed),
        )
    }

    /// Fraction of calls that fell back to regex, or `0.0` before any call.
    pub fn fallback_rate(&self) -> f64 {
        let (fallbacks, calls) = self.stats();
        if calls == 0 {
            0.0
        } else {
            fallbacks as f64 / calls as f64
        }
    }
}

#[async_trait]
impl RequestClassifier for ResilientClassifier {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn fallback_stats(&self) -> Option<(u64, u64)> {
        Some(self.stats())
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let error = match tokio::time::timeout(self.timeout, self.inner.classify(input)).await {
            Ok(Ok(c)) if c.confidence >= self.min_confidence => return Ok(c),
            Ok(Ok(c)) => ClassifyError::LowConfidence(c.confidence),
            Ok(Err(e)) => e,
            Err(_) => ClassifyError::Timeout,
        };

        self.fallbacks.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            backend = self.inner.name(),
            error = %error,
            min_confidence = self.min_confidence,
            fallback_rate = self.fallback_rate(),
            "request classifier: falling back to the regex classifier"
        );
        self.fallback.classify(input).await
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
mod backend_tests {
    use super::*;

    /// Returns a fixed verdict with a chosen confidence.
    struct Fixed {
        request_type: RequestType,
        confidence: f32,
    }

    #[async_trait]
    impl RequestClassifier for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            Ok(Classification {
                request_type: self.request_type,
                complexity: 2,
                confidence: self.confidence,
            })
        }
    }

    struct Failing;
    #[async_trait]
    impl RequestClassifier for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Inference("boom".into()))
        }
    }

    struct Slow;
    #[async_trait]
    impl RequestClassifier for Slow {
        fn name(&self) -> &str {
            "slow"
        }
        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(Classification {
                request_type: RequestType::Writing,
                complexity: 2,
                confidence: 0.99,
            })
        }
    }

    fn input<'a>(query: &'a str) -> ClassifyInput<'a> {
        ClassifyInput {
            query,
            context: None,
        }
    }

    #[tokio::test]
    async fn regex_classifier_is_exactly_the_legacy_classifier() {
        // The default backend must be behaviour-preserving: same request type, fixed
        // complexity 3 and confidence 0.5.
        for query in [
            "write a python function that sorts a list",
            "explain what this function does",
            "what is the capital of France?",
            "hello there",
        ] {
            let c = RegexClassifier.classify(&input(query)).await.unwrap();
            assert_eq!(c.request_type, classify_request_type(query));
            assert_eq!(c.complexity, REGEX_COMPLEXITY);
            assert_eq!(c.confidence, REGEX_CONFIDENCE);
        }
    }

    #[tokio::test]
    async fn confident_model_verdict_passes_through_and_is_counted() {
        let inner = Arc::new(Fixed {
            request_type: RequestType::CodeUnderstanding,
            confidence: 0.9,
        });
        let c = ResilientClassifier::new(inner, Duration::from_millis(50), 0.35);
        let out = c.classify(&input("anything")).await.unwrap();
        assert_eq!(out.request_type, RequestType::CodeUnderstanding);
        assert_eq!(c.stats(), (0, 1));
        assert_eq!(c.fallback_rate(), 0.0);
    }

    #[tokio::test]
    async fn inference_error_falls_back_to_regex_and_counts() {
        let c = ResilientClassifier::new(Arc::new(Failing), Duration::from_millis(50), 0.35);
        let out = c.classify(&input("write a python function")).await.unwrap();
        assert_eq!(out.request_type, RequestType::CodeGeneration); // regex verdict
        assert_eq!(out.confidence, REGEX_CONFIDENCE);
        assert_eq!(c.stats(), (1, 1));
    }

    #[tokio::test]
    async fn timeout_falls_back_to_regex_and_counts() {
        let c = ResilientClassifier::new(Arc::new(Slow), Duration::from_millis(1), 0.35);
        let out = c.classify(&input("hello")).await.unwrap();
        assert_eq!(out.request_type, classify_request_type("hello"));
        assert_eq!(c.stats(), (1, 1));
    }

    #[tokio::test]
    async fn low_confidence_falls_back_to_regex() {
        let inner = Arc::new(Fixed {
            request_type: RequestType::TechnicalDesign,
            confidence: 0.10,
        });
        let c = ResilientClassifier::new(inner, Duration::from_millis(50), 0.35);
        let out = c
            .classify(&input("what is the capital of France?"))
            .await
            .unwrap();
        assert_eq!(out.request_type, RequestType::FactualLookup); // regex, not the model's guess
        assert_eq!(c.stats(), (1, 1));
    }

    #[tokio::test]
    async fn fallback_rate_reflects_the_mix() {
        let c = ResilientClassifier::new(Arc::new(Failing), Duration::from_millis(50), 0.35);
        assert_eq!(c.fallback_rate(), 0.0, "no calls yet");
        c.classify(&input("a")).await.unwrap();
        c.classify(&input("b")).await.unwrap();
        assert_eq!(c.stats(), (2, 2));
        assert_eq!(c.fallback_rate(), 1.0);
    }

    #[tokio::test]
    async fn classification_is_deterministic() {
        let c = ResilientClassifier::new(
            Arc::new(Fixed {
                request_type: RequestType::Writing,
                confidence: 0.8,
            }),
            Duration::from_millis(50),
            0.35,
        );
        let a = c.classify(&input("draft an email")).await.unwrap();
        let b = c.classify(&input("draft an email")).await.unwrap();
        assert_eq!(a, b);
    }
}
