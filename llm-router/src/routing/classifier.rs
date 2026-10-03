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
// 1.5. Pluggable classifier backend — `RequestClassifier` abstraction.
//
//    `classify_request_type` above (the regex) stays the always-available default,
//    wrapped as `RegexClassifier`. A real backend (see `super::classifier_hosted`) is
//    wired in only behind `FallbackClassifier`, so any failure — network, timeout,
//    malformed output, an out-of-range field — degrades to the regex result rather than
//    a guess or an outage. Tier selection (`pick_model_thompson` below) is unchanged by
//    any of this: it only ever consumes the resulting `RequestType`.
// --------------------------------------------------------------------------

/// Input to a [`RequestClassifier`]: the query text to classify, plus optional
/// conversation context. No backend in this crate reads `context` yet — it is reserved
/// for a caller that wants to pass more than the latest user turn.
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// Fixed complexity [`RegexClassifier`] reports — it has no actual complexity signal, so
/// this is a neutral mid-range placeholder, not a measurement.
pub const REGEX_FIXED_COMPLEXITY: u8 = 3;
/// Fixed confidence [`RegexClassifier`] reports — same reasoning as
/// [`REGEX_FIXED_COMPLEXITY`]: a documented placeholder, not a calibrated probability.
pub const REGEX_FIXED_CONFIDENCE: f32 = 0.5;

/// A classifier's verdict. Only constructible through [`Classification::new`], which
/// validates `complexity`/`confidence` — so an out-of-range value from a backend becomes
/// a [`ClassifyError`] at construction time, never a silently-accepted bad value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// `1..=5`, coarse complexity estimate.
    pub complexity: u8,
    /// `0.0..=1.0`, the classifier's confidence in `request_type`.
    pub confidence: f32,
}

impl Classification {
    /// Validate and construct. Fails closed: an out-of-range `complexity` or a
    /// non-finite/out-of-range `confidence` is a [`ClassifyError`], not a clamp.
    pub fn new(
        request_type: RequestType,
        complexity: u8,
        confidence: f32,
    ) -> Result<Self, ClassifyError> {
        if !(1..=5).contains(&complexity) {
            return Err(ClassifyError::InvalidComplexity(complexity));
        }
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(ClassifyError::InvalidConfidence(confidence));
        }
        Ok(Self {
            request_type,
            complexity,
            confidence,
        })
    }
}

/// Why a [`RequestClassifier`] could not produce a [`Classification`]. Every variant is
/// treated identically by [`FallbackClassifier`]: fall back to [`RegexClassifier`], never
/// guess.
#[derive(Debug, Clone)]
pub enum ClassifyError {
    /// The backend did not respond within its configured timeout.
    Timeout,
    /// The HTTP request itself failed (connect/send/transport).
    Network(String),
    /// The backend responded, but with an error status or an exhausted retry.
    Backend(String),
    /// The response body wasn't the expected shape (bad JSON, missing fields).
    MalformedResponse(String),
    /// `request_type` wasn't one of the known wire labels.
    InvalidRequestType(String),
    /// `complexity` was outside `1..=5`.
    InvalidComplexity(u8),
    /// `confidence` was outside `0.0..=1.0` (or non-finite).
    InvalidConfidence(f32),
}

impl std::fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClassifyError::Timeout => write!(f, "classifier timed out"),
            ClassifyError::Network(e) => write!(f, "network error: {e}"),
            ClassifyError::Backend(e) => write!(f, "backend error: {e}"),
            ClassifyError::MalformedResponse(e) => write!(f, "malformed response: {e}"),
            ClassifyError::InvalidRequestType(s) => write!(f, "invalid request_type: {s:?}"),
            ClassifyError::InvalidComplexity(c) => write!(f, "invalid complexity: {c}"),
            ClassifyError::InvalidConfidence(c) => write!(f, "invalid confidence: {c}"),
        }
    }
}

impl std::error::Error for ClassifyError {}

/// A pluggable request-type/complexity/confidence classifier.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// A short, stable name for logs/eval output (e.g. `"regex"`, `"hosted"`).
    fn name(&self) -> &str;

    /// Classify `input`. Implementations should fail (return `Err`) rather than guess —
    /// callers that need a never-fails classifier should wrap this in
    /// [`FallbackClassifier`].
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// The regex vote-count classifier ([`classify_request_type`]), wrapped to satisfy
/// [`RequestClassifier`]. Always succeeds — this is the never-fails base every other
/// backend falls back to.
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let request_type = classify_request_type(input.query);
        Classification::new(request_type, REGEX_FIXED_COMPLEXITY, REGEX_FIXED_CONFIDENCE)
    }
}

/// Wraps any [`RequestClassifier`] with a timeout and a fail-closed fallback to
/// [`RegexClassifier`]: a timeout, or any `Err` from `inner`, degrades to the regex
/// result rather than an outage or a guessed classification. Mirrors the fail-safe
/// direction [`super::salience::SalienceGate`] already takes for Level 2.5.
pub struct FallbackClassifier<C: RequestClassifier> {
    inner: C,
    regex: RegexClassifier,
    timeout: std::time::Duration,
}

impl<C: RequestClassifier> FallbackClassifier<C> {
    pub fn new(inner: C, timeout: std::time::Duration) -> Self {
        Self {
            inner,
            regex: RegexClassifier,
            timeout,
        }
    }
}

#[async_trait]
impl<C: RequestClassifier> RequestClassifier for FallbackClassifier<C> {
    /// Reports the wrapped backend's name — the fallback is an implementation detail of
    /// *how* a classification was produced, not a distinct backend identity.
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        match tokio::time::timeout(self.timeout, self.inner.classify(input)).await {
            Ok(Ok(classification)) => Ok(classification),
            Ok(Err(e)) => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = %self.inner.name(),
                    error = %e,
                    "classifier backend failed; falling back to regex"
                );
                self.regex.classify(input).await
            }
            Err(_elapsed) => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = %self.inner.name(),
                    timeout_ms = self.timeout.as_millis(),
                    "classifier backend timed out; falling back to regex"
                );
                self.regex.classify(input).await
            }
        }
    }
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

    // --- RequestClassifier / RegexClassifier / FallbackClassifier ---

    #[tokio::test]
    async fn regex_classifier_matches_reference_examples() {
        use RequestType::*;
        let cases = [
            ("build me a python script that parses CSV", CodeGeneration),
            ("explain what this function does", CodeUnderstanding),
            ("how should I design this API?", TechnicalDesign),
            (
                "calculate the probability that it rains tomorrow",
                AnalyticalReasoning,
            ),
            ("draft an email to my team about the outage", Writing),
            ("what is the capital of France?", FactualLookup),
            ("hello there", General),
        ];
        let classifier = RegexClassifier;
        for (query, expected) in cases {
            let input = ClassifyInput {
                query,
                context: None,
            };
            let result = classifier.classify(&input).await.unwrap();
            assert_eq!(result.request_type, expected, "query: {query:?}");
            assert_eq!(result.complexity, REGEX_FIXED_COMPLEXITY);
            assert_eq!(result.confidence, REGEX_FIXED_CONFIDENCE);
        }
    }

    #[tokio::test]
    async fn regex_classifier_is_deterministic() {
        let classifier = RegexClassifier;
        let input = ClassifyInput {
            query: "refactor this function to use async/await",
            context: None,
        };
        let a = classifier.classify(&input).await.unwrap();
        let b = classifier.classify(&input).await.unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn classification_rejects_invalid_complexity() {
        assert!(matches!(
            Classification::new(RequestType::General, 0, 0.5),
            Err(ClassifyError::InvalidComplexity(0))
        ));
        assert!(matches!(
            Classification::new(RequestType::General, 6, 0.5),
            Err(ClassifyError::InvalidComplexity(6))
        ));
    }

    #[test]
    fn classification_rejects_invalid_confidence() {
        assert!(matches!(
            Classification::new(RequestType::General, 3, -0.1),
            Err(ClassifyError::InvalidConfidence(_))
        ));
        assert!(matches!(
            Classification::new(RequestType::General, 3, 1.1),
            Err(ClassifyError::InvalidConfidence(_))
        ));
        assert!(matches!(
            Classification::new(RequestType::General, 3, f32::NAN),
            Err(ClassifyError::InvalidConfidence(_))
        ));
    }

    #[test]
    fn classification_accepts_boundary_values() {
        assert!(Classification::new(RequestType::General, 1, 0.0).is_ok());
        assert!(Classification::new(RequestType::General, 5, 1.0).is_ok());
    }

    /// A controllable classifier double for exercising `FallbackClassifier`'s branches.
    struct FakeClassifier {
        result: Result<Classification, ClassifyError>,
        delay: std::time::Duration,
    }

    #[async_trait]
    impl RequestClassifier for FakeClassifier {
        fn name(&self) -> &str {
            "fake"
        }

        async fn classify(
            &self,
            _input: &ClassifyInput<'_>,
        ) -> Result<Classification, ClassifyError> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn fallback_classifier_passes_through_inner_success() {
        let expected = Classification::new(RequestType::Writing, 2, 0.9).unwrap();
        let fallback = FallbackClassifier::new(
            FakeClassifier {
                result: Ok(expected),
                delay: std::time::Duration::ZERO,
            },
            std::time::Duration::from_millis(500),
        );
        let input = ClassifyInput {
            query: "irrelevant",
            context: None,
        };
        let got = fallback.classify(&input).await.unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn fallback_classifier_falls_back_to_regex_on_error() {
        let fallback = FallbackClassifier::new(
            FakeClassifier {
                result: Err(ClassifyError::Backend("boom".into())),
                delay: std::time::Duration::ZERO,
            },
            std::time::Duration::from_millis(500),
        );
        let input = ClassifyInput {
            query: "hello there",
            context: None,
        };
        let got = fallback.classify(&input).await.unwrap();
        assert_eq!(got.request_type, RequestType::General);
        assert_eq!(got.complexity, REGEX_FIXED_COMPLEXITY);
    }

    #[tokio::test]
    async fn fallback_classifier_falls_back_to_regex_on_timeout() {
        let fallback = FallbackClassifier::new(
            FakeClassifier {
                result: Ok(Classification::new(RequestType::Writing, 2, 0.9).unwrap()),
                delay: std::time::Duration::from_millis(200),
            },
            std::time::Duration::from_millis(20),
        );
        let input = ClassifyInput {
            query: "what is the capital of France?",
            context: None,
        };
        let got = fallback.classify(&input).await.unwrap();
        assert_eq!(got.request_type, RequestType::FactualLookup);
        assert_eq!(got.complexity, REGEX_FIXED_COMPLEXITY);
    }
}
