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
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Beta, Distribution};
use serde::{Deserialize, Serialize};

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
///
/// The serde form is the same snake_case string as [`RequestType::as_str`] (checked by a
/// test), so eval rows, cache entries and API payloads all agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

    /// Every request type, in the fixed order used wherever a stable ordering matters (eval
    /// confusion matrices, hosted-choice option order, hashing).
    pub const ALL: [RequestType; 7] = [
        RequestType::CodeGeneration,
        RequestType::CodeUnderstanding,
        RequestType::TechnicalDesign,
        RequestType::AnalyticalReasoning,
        RequestType::Writing,
        RequestType::FactualLookup,
        RequestType::General,
    ];
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
    classify_request_type_scored(text).0
}

/// [`classify_request_type`] plus the winning vote count (`0` ⇒ nothing matched and
/// `General` is the default, not a verdict). The regex backend uses the count only to pick
/// which of its two fixed confidence values to report.
pub fn classify_request_type_scored(text: &str) -> (RequestType, usize) {
    let mut best = RequestType::General;
    let mut best_score = 0usize;
    for (rt, pats) in CATEGORY_PATTERNS.iter() {
        let score = pats.iter().filter(|p| p.is_match(text)).count();
        if score > best_score {
            best_score = score;
            best = *rt;
        }
    }
    (best, best_score)
}

/// Compile the regex tables now rather than on the first query, so one-time setup is not
/// billed to the first request's decision latency.
pub fn warm_regex_tables() {
    let _ = CATEGORY_PATTERNS.len();
    let _ = NEGATIVE_SIGNALS.len();
    let _ = POSITIVE_SIGNALS.len();
}

// --------------------------------------------------------------------------
// 1b. The RequestClassifier contract — what every backend (regex, hosted, …) implements
// --------------------------------------------------------------------------

/// What a classifier sees. `query` is the latest user request; `context` is optional,
/// bounded, role-labelled surrounding material (earlier turns, a supplied code snippet) —
/// never the system prompt, credentials or the whole transcript. See
/// [`super::context::classifier_context`] for how the router builds it.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// A classifier's verdict. `complexity` follows the eval rubric (1 trivial single operation
/// … 5 intricate cross-component reasoning); `confidence` is the probability the backend
/// assigns to `request_type` — for a hosted backend the predicted-type probability, for the
/// regex backend a fixed, **uncalibrated** placeholder (see [`RegexClassifier`]).
/// Complexity carries no confidence of its own: a sure request type says nothing about how
/// reliable the difficulty estimate is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub request_type: RequestType,
    /// `1..=5`.
    pub complexity: u8,
    /// Finite, `0.0..=1.0`.
    pub confidence: f32,
}

impl Classification {
    /// The contract every backend must honour; the service rejects anything else as
    /// invalid output and falls back, so a buggy backend can never route a request on a
    /// nonsense verdict.
    pub fn validate(&self) -> Result<(), ClassifyError> {
        if !(1..=5).contains(&self.complexity) {
            return Err(ClassifyError::InvalidResponse(format!(
                "complexity {} outside 1..=5",
                self.complexity
            )));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(ClassifyError::InvalidResponse(format!(
                "confidence {} outside 0..=1",
                self.confidence
            )));
        }
        Ok(())
    }
}

/// Why a backend could not answer. Each variant maps onto one counted fallback reason
/// ([`super::classifier_service::FallbackReason`]); messages never contain credentials or
/// the request text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClassifyError {
    /// The backend was never usable (missing key, bad endpoint, model failed to load).
    #[error("classifier backend not initialized: {0}")]
    Init(String),
    /// The overall deadline (connection + inference + validation + any retry) expired.
    #[error("classifier call timed out")]
    Timeout,
    /// Could not reach the backend (DNS, connect, TLS, reset mid-body).
    #[error("classifier network error: {0}")]
    Network(String),
    /// The backend answered with a failure status. `status` is the HTTP code (`429`,
    /// `529`, `5xx`, …) so diagnostics can tell rate limiting from a server fault.
    #[error("classifier upstream error (HTTP {status}): {detail}")]
    Upstream { status: u16, detail: String },
    /// The backend answered 2xx but the body violated the documented contract (unknown
    /// label, probabilities that do not sum to one, non-finite number, missing answer…).
    #[error("classifier returned an invalid response: {0}")]
    InvalidResponse(String),
}

/// Optional, backend-specific detail about one answer. Kept off [`Classification`] so the
/// routing hot path and the eval row stay small; the service, the eval sidecar and the UI
/// preview surface it when present.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BackendDiagnostics {
    /// The model version the backend reports it actually ran (e.g. Jev's response `model`).
    pub model_version: Option<String>,
    /// Full request-type distribution, in [`RequestType::ALL`] order.
    pub type_probabilities: Vec<(RequestType, f32)>,
    /// Full complexity distribution over levels 1..=5.
    pub complexity_probabilities: Vec<f32>,
    /// Probability-weighted expected complexity on the 1..=5 scale, if the backend gives
    /// a distribution.
    pub complexity_expected: Option<f32>,
    /// The vendor's own spread-based confidence statistic for the type question. Not a
    /// probability of correctness; kept separately from [`Classification::confidence`].
    pub vendor_type_confidence: Option<f32>,
    /// Same, for the complexity question.
    pub vendor_complexity_confidence: Option<f32>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// HTTP attempts made for this answer (1 = no retry).
    pub attempts: u32,
    /// The backend cut the input to its own window (e.g. a local model's token budget), as
    /// opposed to the service's character caps.
    #[serde(default)]
    pub input_truncated: bool,
}

/// A backend answer plus optional diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub classification: Classification,
    pub diagnostics: Option<BackendDiagnostics>,
}

/// The pluggable classifier contract (P2 §1). Async because hosted backends make network
/// calls; `Send + Sync` so one instance is shared across handler tasks. Implementations are
/// built **once** (clients, keys, compiled tables) and reused for every request.
///
/// The router never calls a backend directly — it goes through
/// [`super::classifier_service::ClassifierService`], which owns timing, validation,
/// the confidence gate and the regex fallback, so every caller (router, eval, UI preview)
/// shares one inference path.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Stable backend label: `"regex"`, `"jev"`, ….
    fn name(&self) -> &str;

    /// Classify one request. Errors are the backend's failure modes; the service turns
    /// them into counted regex fallbacks.
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;

    /// [`classify`](Self::classify) plus whatever diagnostics the backend has. The default
    /// carries none; a backend with a distribution overrides this and implements
    /// `classify` in terms of it.
    async fn classify_detailed(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classified, ClassifyError> {
        Ok(Classified {
            classification: self.classify(input).await?,
            diagnostics: None,
        })
    }
}

/// Fixed complexity the regex backend reports: the rubric midpoint, because keyword votes
/// carry no difficulty signal at all. It is a placeholder, not an estimate.
pub const REGEX_COMPLEXITY: u8 = 3;
/// Fixed confidence the regex backend reports when at least one category pattern voted.
/// **Uncalibrated**: it is not a measured accuracy, just a constant that lets regex rows
/// flow through the same `{request_type, complexity, confidence}` contract. The eval
/// report measures the actual calibration error this constant produces.
pub const REGEX_CONFIDENCE_MATCHED: f32 = 0.5;
/// Fixed confidence when no pattern matched and `General` was returned by default — lower
/// than the matched value because the label is a fallthrough, not a vote. Equally
/// uncalibrated.
pub const REGEX_CONFIDENCE_DEFAULT: f32 = 0.3;

/// The default backend: wraps [`classify_request_type`] unchanged. Needs no network, key or
/// model; ignores `context` (the legacy classifier only ever saw the query). Reports the
/// fixed [`REGEX_COMPLEXITY`] and one of the two fixed confidence constants above.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

impl RegexClassifier {
    /// Synchronous form, for callers already outside async (the service's fallback path).
    pub fn classify_sync(&self, input: &ClassifyInput<'_>) -> Classification {
        let (request_type, votes) = classify_request_type_scored(input.query);
        Classification {
            request_type,
            complexity: REGEX_COMPLEXITY,
            confidence: if votes > 0 {
                REGEX_CONFIDENCE_MATCHED
            } else {
                REGEX_CONFIDENCE_DEFAULT
            },
        }
    }
}

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_sync(input))
    }
}

/// Build the RNG that Thompson-samples a tier for one classification.
///
/// `None` seed ⇒ the legacy entropy RNG (exploration as before). `Some(seed)` ⇒ a
/// [`StdRng`] seeded from a stable hash of `(seed, provider, request_type, query, learned
/// cells)`, so identical inputs, config and learned state always pick the same tier, while
/// different queries still explore differently across the population. Cells are hashed in
/// sorted `(tier, request_type)` order; any change to a cell's mean or sample count
/// changes the seed, so a tier chosen before and after learning may legitimately differ.
/// `DefaultHasher::new()` uses fixed keys, so the seed is stable across processes.
pub fn tier_rng(
    seed: Option<u64>,
    provider: &str,
    request_type: RequestType,
    query: &str,
    cells: &CellMap,
) -> TierRng {
    let Some(seed) = seed else {
        return TierRng::Entropy(rand::rng());
    };
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    provider.hash(&mut h);
    request_type.as_str().hash(&mut h);
    query.hash(&mut h);
    let mut ordered: Vec<_> = cells.iter().collect();
    ordered.sort_by_key(|((tier, rt), _)| (tier.as_level(), rt.as_str()));
    for ((tier, rt), cell) in ordered {
        tier.as_level().hash(&mut h);
        rt.as_str().hash(&mut h);
        cell.quality_mean.to_bits().hash(&mut h);
        cell.samples.hash(&mut h);
    }
    TierRng::Seeded(Box::new(StdRng::seed_from_u64(h.finish())))
}

/// Either RNG the router may sample tiers with. `ThreadRng` is `!Send`, so callers scope
/// this to drop before the next `.await` exactly as the legacy code did.
pub enum TierRng {
    Entropy(rand::rngs::ThreadRng),
    Seeded(Box<StdRng>),
}

impl TierRng {
    /// Thompson-sample a tier with the default quality/cost blend.
    pub fn pick(&mut self, cells: &CellMap, request_type: RequestType) -> Tier {
        match self {
            TierRng::Entropy(r) => {
                pick_model_thompson(cells, request_type, DEFAULT_W_QUALITY, DEFAULT_W_COST, r)
            }
            TierRng::Seeded(r) => {
                pick_model_thompson(cells, request_type, DEFAULT_W_QUALITY, DEFAULT_W_COST, r)
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
    fn request_type_serde_matches_as_str() {
        for rt in RequestType::ALL {
            let json = serde_json::to_string(&rt).unwrap();
            assert_eq!(json, format!("\"{}\"", rt.as_str()));
            let back: RequestType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, rt);
        }
        assert!(serde_json::from_str::<RequestType>("\"nonsense\"").is_err());
    }

    #[test]
    fn scored_classifier_agrees_with_legacy_and_reports_votes() {
        let (rt, votes) = classify_request_type_scored("write me a Python sort function");
        assert_eq!(rt, RequestType::CodeGeneration);
        assert!(votes > 0);
        assert_eq!(rt, classify_request_type("write me a Python sort function"));
        let (rt, votes) = classify_request_type_scored("hello there");
        assert_eq!((rt, votes), (RequestType::General, 0));
    }

    #[tokio::test]
    async fn regex_backend_wraps_legacy_and_reports_fixed_values() {
        let c = RegexClassifier;
        assert_eq!(c.name(), "regex");
        for q in [
            "write me a Python sort function",
            "what is the capital of France?",
            "hello there",
            "",
            "日本語のテキスト 🚀",
        ] {
            let out = c
                .classify(&ClassifyInput {
                    query: q,
                    context: Some("ignored by regex"),
                })
                .await
                .unwrap();
            assert_eq!(
                out.request_type,
                classify_request_type(q),
                "label for {q:?}"
            );
            assert_eq!(out.complexity, REGEX_COMPLEXITY);
            out.validate().unwrap();
        }
        let matched = c.classify_sync(&ClassifyInput {
            query: "write me a Python sort function",
            context: None,
        });
        assert_eq!(matched.confidence, REGEX_CONFIDENCE_MATCHED);
        let unmatched = c.classify_sync(&ClassifyInput {
            query: "hello there",
            context: None,
        });
        assert_eq!(unmatched.confidence, REGEX_CONFIDENCE_DEFAULT);
        // Default-trait diagnostics are absent for the regex backend.
        let d = c
            .classify_detailed(&ClassifyInput {
                query: "hi",
                context: None,
            })
            .await
            .unwrap();
        assert!(d.diagnostics.is_none());
    }

    #[test]
    fn classification_validation_rejects_out_of_contract_values() {
        let ok = Classification {
            request_type: RequestType::General,
            complexity: 1,
            confidence: 0.0,
        };
        ok.validate().unwrap();
        for (complexity, confidence) in [(0, 0.5), (6, 0.5), (3, -0.1), (3, 1.01), (3, f32::NAN)] {
            let bad = Classification {
                request_type: RequestType::General,
                complexity,
                confidence,
            };
            assert!(
                matches!(bad.validate(), Err(ClassifyError::InvalidResponse(_))),
                "({complexity}, {confidence}) should be invalid"
            );
        }
    }

    #[test]
    fn seeded_tier_rng_is_repeatable_and_sensitive_to_state() {
        let cells = CellMap::new();
        let pick = |seed, query: &str, cells: &CellMap| {
            tier_rng(seed, "openai", RequestType::Writing, query, cells)
                .pick(cells, RequestType::Writing)
        };
        // Same seed/inputs/state ⇒ same tier, every time.
        for _ in 0..20 {
            assert_eq!(
                pick(Some(7), "draft an email", &cells),
                pick(Some(7), "draft an email", &cells)
            );
        }
        // Different seeds do not all agree (the seed actually matters): over many seeds at
        // least two distinct tiers must appear for an unlearned, mid-strength request type.
        let distinct: std::collections::HashSet<_> = (0..64u64)
            .map(|s| pick(Some(s), "draft an email", &cells))
            .collect();
        assert!(distinct.len() > 1, "seed had no effect: {distinct:?}");
        // Learned state is part of the seed derivation: a changed cell may change the pick,
        // and the derivation must not panic on a populated map.
        let mut learned = CellMap::new();
        learned.insert(
            (Tier::Tier3, RequestType::Writing),
            Cell {
                quality_mean: 0.99,
                samples: MAX_SAMPLES,
            },
        );
        let _ = pick(Some(7), "draft an email", &learned);
        // No seed ⇒ entropy RNG still works (legacy path).
        let _ = pick(None, "draft an email", &cells);
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
