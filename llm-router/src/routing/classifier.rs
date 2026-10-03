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
use super::vector_classifier::VectorSimilarityClassifier;

/// Regex buckets are uncalibrated keyword votes. Fixed confidence so downstream ECE is
/// honest: this is not a model posterior. `General` is the catch-all, so it is lower.
pub const REGEX_CONFIDENCE: f32 = 0.55;
/// Confidence when the regex match set is empty and the type is `General`.
pub const REGEX_GENERAL_CONFIDENCE: f32 = 0.40;

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

/// Complexity 1–5 implied by a regex [`RequestType`]. The regex does not inspect length or
/// nesting; these are documented constants so bandit keys stay deterministic.
///
/// 1 factual one-shot · 2 catch-all / short writing · 3 reading existing code ·
/// 4 implementation or quantitative reasoning · 5 system design.
pub fn regex_complexity(rt: RequestType) -> u8 {
    match rt {
        RequestType::FactualLookup => 1,
        RequestType::General | RequestType::Writing => 2,
        RequestType::CodeUnderstanding => 3,
        RequestType::CodeGeneration | RequestType::AnalyticalReasoning => 4,
        RequestType::TechnicalDesign => 5,
    }
}

fn regex_confidence(rt: RequestType) -> f32 {
    if rt == RequestType::General {
        REGEX_GENERAL_CONFIDENCE
    } else {
        REGEX_CONFIDENCE
    }
}

/// Query (+ optional extra context) for [`RequestClassifier::classify`].
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// Model-agnostic decision the router and `classifier_eval` both consume.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// 1 (trivial) through 5 (hardest).
    pub complexity: u8,
    /// Calibrated [0, 1] for a model backend; fixed constants for regex.
    pub confidence: f32,
    /// True when this result came from the regex fallback (error, timeout, or low confidence).
    pub fallback: bool,
}

/// Why a non-regex backend refused to answer. The router never surfaces this to the user:
/// [`classify_with_regex_fallback`] substitutes the regex result and counts a fallback.
#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("classifier backend timed out after {0:?}")]
    Timeout(Duration),
    #[error("classifier backend unavailable: {0}")]
    Backend(String),
    #[error("classifier returned an unusable result: {0}")]
    Invalid(String),
}

/// Async so a hosted backend (Jev) can perform HTTP. Implementations must not read env
/// vars — the binary's [`crate::config::GatewayConfig`] supplies endpoint, key, timeout.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// Default out-of-the-box classifier: [`classify_request_type`] plus the fixed complexity
/// and confidence tables above. Infallible.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(regex_classification(input.query))
    }
}

/// Regex decision for `query`. Context is ignored: the keyword tables only see the text.
pub fn regex_classification(query: &str) -> Classification {
    let request_type = classify_request_type(query);
    Classification {
        request_type,
        complexity: regex_complexity(request_type),
        confidence: regex_confidence(request_type),
        fallback: false,
    }
}

/// Runs `primary`, then regex on `Err` or when `confidence` is below `min_confidence`.
///
/// Low-confidence Jev answers are treated as fallbacks (not confident wrong labels) so the
/// cost-aware bandit is not trained on an unsure class. Identical inputs are deterministic
/// for the regex path; Jev itself is a remote model.
pub struct FallbackClassifier {
    primary: Arc<dyn RequestClassifier>,
    fallback: Arc<dyn RequestClassifier>,
    min_confidence: f32,
    fallbacks: AtomicU64,
}

impl FallbackClassifier {
    pub fn new(
        primary: Arc<dyn RequestClassifier>,
        fallback: Arc<dyn RequestClassifier>,
        min_confidence: f32,
    ) -> Self {
        Self {
            primary,
            fallback,
            min_confidence: min_confidence.clamp(0.0, 1.0),
            fallbacks: AtomicU64::new(0),
        }
    }

    pub fn fallback_count(&self) -> u64 {
        self.fallbacks.load(Ordering::Relaxed)
    }

    async fn regex_fallback(
        &self,
        input: &ClassifyInput<'_>,
        reason: &str,
    ) -> Result<Classification, ClassifyError> {
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "nasiko::llm_router::classifier",
            primary = self.primary.name(),
            reason,
            "request classifier: using regex fallback"
        );
        let mut c = self.fallback.classify(input).await?;
        c.fallback = true;
        Ok(c)
    }
}

#[async_trait]
impl RequestClassifier for FallbackClassifier {
    fn name(&self) -> &str {
        self.primary.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        match self.primary.classify(input).await {
            Ok(c) if c.confidence + f32::EPSILON < self.min_confidence => {
                self.regex_fallback(
                    input,
                    &format!(
                        "confidence {:.3} below min {:.3}",
                        c.confidence, self.min_confidence
                    ),
                )
                .await
            }
            Ok(c) => Ok(c),
            Err(err) => self.regex_fallback(input, &err.to_string()).await,
        }
    }
}

/// Trait classify, then regex if the backend errors. Used at the `route_model` seam so a
/// bare Jev client still fail-closes instead of skipping Level 3.
pub async fn classify_with_regex_fallback(
    classifier: &dyn RequestClassifier,
    input: &ClassifyInput<'_>,
) -> Classification {
    match classifier.classify(input).await {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                target: "nasiko::llm_router::classifier",
                classifier = classifier.name(),
                error = %err,
                "request classifier failed; falling back to regex"
            );
            let mut c = regex_classification(input.query);
            c.fallback = true;
            c
        }
    }
}

/// Wire a classifier from already-parsed config (no env reads here).
///
/// * `CLASSIFIER_BACKEND=regex` (default) — regex only, no network.
/// * `jev` / `hosted` — Jev primary when `classifier_api_key` is set; regex if the key is
///   empty (not configured) or wrapped with [`FallbackClassifier`] for call failures.
// Updated classifier builder to use vector similarity when JEV is not configured.
pub fn build_request_classifier(cfg: &crate::config::GatewayConfig) -> Arc<dyn RequestClassifier> {
    let regex: Arc<dyn RequestClassifier> = Arc::new(RegexClassifier);
    let vector: Arc<dyn RequestClassifier> = Arc::new(VectorSimilarityClassifier);
    let backend = cfg.classifier_backend.trim().to_ascii_lowercase();
    let jev_requested = matches!(backend.as_str(), "jev" | "hosted");

    if jev_requested && !cfg.classifier_api_key.is_empty() {
        // Try to build JEV classifier
        match super::jev::JevClassifier::from_config(cfg) {
            Ok(jev) => {
                tracing::info!(
                    target: "nasiko::llm_router::startup",
                    endpoint = %cfg.classifier_endpoint,
                    model = %cfg.classifier_model,
                    timeout_ms = cfg.classifier_timeout_ms,
                    min_confidence = cfg.classifier_min_confidence,
                    "llm-router: request classifier = jev (regex fallback on timeout, HTTP error, or low confidence)"
                );
                Arc::new(FallbackClassifier::new(Arc::new(jev), regex, cfg.classifier_min_confidence))
            }
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::startup",
                    error = %e,
                    "llm-router: failed to build Jev classifier; using vector primary"
                );
                Arc::new(FallbackClassifier::new(vector, regex, cfg.classifier_min_confidence))
            }
        }
    } else {
        // JEV not requested or missing API key – use vector similarity as primary.
        if !jev_requested {
            tracing::info!(
                target: "nasiko::llm_router::startup",
                backend = %cfg.classifier_backend,
                "llm-router: request classifier = vector (regex fallback; JEV not requested)"
            );
        } else {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                "llm-router: CLASSIFIER_BACKEND={backend} but no CLASSIFIER_API_KEY; using vector primary"
            );
        }
        Arc::new(FallbackClassifier::new(vector, regex, cfg.classifier_min_confidence))
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

    struct UnavailableClassifier;

    #[async_trait]
    impl RequestClassifier for UnavailableClassifier {
        fn name(&self) -> &str {
            "unavailable-test"
        }

        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Backend("test outage".into()))
        }
    }

    #[tokio::test]
    async fn unavailable_primary_uses_regex_fallback() {
        let classifier = FallbackClassifier::new(
            Arc::new(UnavailableClassifier),
            Arc::new(RegexClassifier),
            0.60,
        );
        let result = classifier
            .classify(&ClassifyInput {
                query: "write a Python script",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert!(result.fallback);
        assert_eq!(classifier.fallback_count(), 1);
    }

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
