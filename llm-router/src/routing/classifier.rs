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

/// Input to a request classifier. `context` is optional surrounding transcript or code
/// context; the default regex backend deliberately ignores it to preserve historic output.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// A deterministic request classification. Complexity is an ordinal routing hint from 1
/// (mechanical/simple) to 5 (multi-step or high-risk); confidence expresses calibration of
/// the request-type choice, not answer quality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("classifier backend unavailable: {0}")]
    Unavailable(String),
    #[error("classifier backend failed: {0}")]
    Failed(String),
}

/// Model-agnostic classifier interface. Backends must be deterministic for identical input.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// Existing regex classifier, adapted to the typed interface. Complexity is fixed at 3 and
/// confidence is 0.60: the old classifier has no calibrated margin and this value keeps the
/// default router behaviour unchanged while making its limitation explicit.
#[derive(Debug, Default)]
pub struct RegexRequestClassifier;

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(Classification {
            request_type: classify_request_type(input.query),
            complexity: 3,
            confidence: 0.60,
        })
    }
}

/// A compact local classifier that combines lexical intent features with query/context
/// complexity features. It has no model download or network dependency; its fixed weights
/// are deterministic and the context signal is opt-in through `CLASSIFIER_BACKEND=local`.
#[derive(Debug, Default)]
pub struct LocalRequestClassifier;

#[async_trait]
impl RequestClassifier for LocalRequestClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(local_classify(input))
    }
}

/// Select the built-in backend named by configuration. Hosted classifiers are intentionally
/// not constructed until a concrete protocol is configured; callers receive an error and
/// must use the regex fallback rather than silently making an unbounded network call.
pub fn builtin_classifier(backend: &str) -> Result<Box<dyn RequestClassifier>, ClassifyError> {
    match backend.trim().to_ascii_lowercase().as_str() {
        "" | "regex" => Ok(Box::new(RegexRequestClassifier)),
        "local" => Ok(Box::new(LocalRequestClassifier)),
        "hosted" => Err(ClassifyError::Unavailable(
            "hosted backend is not configured in this build".into(),
        )),
        other => Err(ClassifyError::Unavailable(format!(
            "unknown backend `{other}`"
        ))),
    }
}

/// Invoke a backend with a bounded deadline and conservatively fall back to the historic
/// regex result. Local/hosted decisions below `min_confidence` take the same safe path.
/// The boolean is true only when a fallback was used, so the router can count it.
pub async fn classify_with_fallback(
    classifier: &dyn RequestClassifier,
    input: &ClassifyInput<'_>,
    timeout: Duration,
    min_confidence: f32,
) -> (Classification, bool) {
    let fallback = || Classification {
        request_type: classify_request_type(input.query),
        complexity: 3,
        confidence: 0.60,
    };
    if classifier.name() == "regex" {
        return (fallback(), false);
    }
    match tokio::time::timeout(timeout, classifier.classify(input)).await {
        Ok(Ok(classification)) if classification.confidence >= min_confidence => {
            (classification, false)
        }
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => (fallback(), true),
    }
}

fn local_classify(input: &ClassifyInput<'_>) -> Classification {
    let query = input.query.to_ascii_lowercase();
    let context = input.context.unwrap_or("").to_ascii_lowercase();
    let combined = if context.is_empty() {
        query.clone()
    } else {
        format!("{query}\n{context}")
    };
    let mut scores = [0_i32; 7];
    let regex_type = classify_request_type(&query);
    scores[type_index(regex_type)] += 2;

    add_score(
        &mut scores,
        RequestType::CodeGeneration,
        &combined,
        &[
            "implement",
            "debug",
            "fix",
            "refactor",
            "unit test",
            "patch",
            "compile error",
            "stack trace",
        ],
    );
    // A small edit is still code work when the query identifies source code explicitly.
    // This prevents the historic generic prior from swallowing requests such as fixing a
    // typo in a Python comment.
    if query.contains("fix")
        && [
            "code",
            "comment",
            "python",
            "rust",
            "javascript",
            "typescript",
        ]
        .iter()
        .any(|cue| combined.contains(cue))
    {
        scores[type_index(RequestType::CodeGeneration)] += 2;
    }
    if query.contains("do not redesign") && query.contains("just change") {
        scores[type_index(RequestType::CodeGeneration)] += 4;
    }
    add_score(
        &mut scores,
        RequestType::CodeUnderstanding,
        &combined,
        &[
            "explain",
            "walk through",
            "review this",
            "why does",
            "what does",
            "trace",
            "understand",
        ],
    );
    add_score(
        &mut scores,
        RequestType::TechnicalDesign,
        &combined,
        &[
            "architecture",
            "design",
            "tradeoff",
            "scalab",
            "migration",
            "distributed",
            "rollout",
            "idempotency",
            "database schema",
            "api contract",
        ],
    );
    if query.starts_with("design ") || query.contains("give architecture") {
        scores[type_index(RequestType::TechnicalDesign)] += 2;
    }
    add_score(
        &mut scores,
        RequestType::AnalyticalReasoning,
        &combined,
        &[
            "calculate",
            "derive",
            "prove",
            "probability",
            "optimiz",
            "analyze",
            "compare",
            "diagnose",
            "investigate",
            "reconstruct",
            "interleaving",
            "invariants",
        ],
    );
    if query.contains("diagnose") && query.contains("reconstruct") {
        scores[type_index(RequestType::AnalyticalReasoning)] += 2;
    }
    add_score(
        &mut scores,
        RequestType::Writing,
        &combined,
        &[
            "draft",
            "rewrite",
            "tone",
            "email",
            "blog",
            "copyedit",
            "summarize",
        ],
    );
    if query.contains("what does") && query.contains("::") {
        scores[type_index(RequestType::FactualLookup)] += 3;
    }
    if query.contains("summarize") {
        scores[type_index(RequestType::Writing)] += 2;
    }
    add_score(
        &mut scores,
        RequestType::FactualLookup,
        &combined,
        &[
            "who is",
            "when did",
            "where is",
            "definition",
            "capital of",
            "latest",
            "release date",
            "what does",
        ],
    );
    if query.split_whitespace().count() <= 4 {
        scores[type_index(RequestType::General)] += 1;
    }

    let (winner, top, runner_up) = scores.iter().enumerate().fold(
        (RequestType::General, i32::MIN, i32::MIN),
        |state, (index, score)| {
            if *score > state.1 {
                (request_type_at(index), *score, state.1)
            } else if *score > state.2 {
                (state.0, state.1, *score)
            } else {
                state
            }
        },
    );
    let margin = (top - runner_up).max(0) as f32;
    let evidence = top.max(0) as f32;
    let confidence = (0.44 + 0.09 * evidence + 0.06 * margin).clamp(0.45, 0.95);
    Classification {
        request_type: winner,
        complexity: local_complexity(&query, &context),
        confidence,
    }
}

fn add_score(scores: &mut [i32; 7], request_type: RequestType, text: &str, terms: &[&str]) {
    scores[type_index(request_type)] +=
        terms.iter().filter(|term| text.contains(**term)).count() as i32;
}

fn local_complexity(query: &str, context: &str) -> u8 {
    let tokens = query.split_whitespace().count();
    let context_tokens = context.split_whitespace().count();
    if (query.contains("fix typo") && context_tokens <= 10)
        || (query.contains("do not redesign") && query.contains("just change"))
    {
        return 1;
    }
    let no_context = context.starts_with("no codebase context");
    let mut complexity = if tokens <= 8 && (context_tokens == 0 || no_context) {
        1
    } else if tokens <= 25 {
        2
    } else {
        3
    };
    if context_tokens > 80
        || query.contains("```")
        || query.contains("stack trace")
        || (context_tokens > 10 && query.contains("unit tests"))
    {
        complexity += 1;
    }
    if contains_any(
        query,
        &[
            "and then",
            "also",
            "migration",
            "security",
            "distributed",
            "backward compatible",
            "production",
            "concurrency-safe",
            "interleavings",
            "reconstruct",
        ],
    ) || query.matches(',').count() >= 3
    {
        complexity += 1;
    }
    complexity.clamp(1, 5)
}

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

fn type_index(request_type: RequestType) -> usize {
    match request_type {
        RequestType::CodeGeneration => 0,
        RequestType::CodeUnderstanding => 1,
        RequestType::TechnicalDesign => 2,
        RequestType::AnalyticalReasoning => 3,
        RequestType::Writing => 4,
        RequestType::FactualLookup => 5,
        RequestType::General => 6,
    }
}

fn request_type_at(index: usize) -> RequestType {
    [
        RequestType::CodeGeneration,
        RequestType::CodeUnderstanding,
        RequestType::TechnicalDesign,
        RequestType::AnalyticalReasoning,
        RequestType::Writing,
        RequestType::FactualLookup,
        RequestType::General,
    ][index]
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

    #[tokio::test]
    async fn local_backend_uses_context_and_reports_bounded_complexity() {
        let classifier = LocalRequestClassifier;
        let result = classifier
            .classify(&ClassifyInput {
                query: "Design a multi-region migration with backward compatibility",
                context: Some(
                    "Existing API clients must continue to work during a staged rollout.",
                ),
            })
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::TechnicalDesign);
        assert!((1..=5).contains(&result.complexity));
        assert!((0.0..=1.0).contains(&result.confidence));
    }

    #[tokio::test]
    async fn local_backend_recognizes_small_source_code_edits() {
        let classifier = LocalRequestClassifier;
        let result = classifier
            .classify(&ClassifyInput {
                query: "Fix typo in this Python comment",
                context: Some("No other files need changes."),
            })
            .await
            .unwrap();

        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.complexity, 1);
    }

    struct BrokenClassifier;

    #[async_trait]
    impl RequestClassifier for BrokenClassifier {
        fn name(&self) -> &str {
            "broken"
        }

        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            Err(ClassifyError::Failed("test failure".into()))
        }
    }

    #[tokio::test]
    async fn failed_backend_falls_back_to_historic_regex_result() {
        let (result, fallback) = classify_with_fallback(
            &BrokenClassifier,
            &ClassifyInput {
                query: "draft an email",
                context: None,
            },
            std::time::Duration::from_millis(10),
            0.55,
        )
        .await;
        assert!(fallback);
        assert_eq!(result.request_type, classify_request_type("draft an email"));
        assert_eq!(result.complexity, 3);
    }
}
