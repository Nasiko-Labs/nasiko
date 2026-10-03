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

use rand::Rng;
use rand_distr::{Beta, Distribution};

use super::complexity::{
    official_complexity, score_complexity, tier_for_internal_score, ComplexityScore,
};
use super::minilm::SemanticClassifier;
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

/// Per-category vote counts for `text` — the raw material behind
/// [`classify_request_type`]. One entry per known category (in
/// [`CATEGORY_PATTERNS`](super::patterns) order); `General` never scores
/// (it is the zero-vote default, not a pattern).
fn vote_scores(text: &str) -> Vec<(RequestType, usize)> {
    CATEGORY_PATTERNS
        .iter()
        .map(|(rt, pats)| (*rt, pats.iter().filter(|p| p.is_match(text)).count()))
        .collect()
}

/// Bucket a query into a [`RequestType`] by vote count — the category matching the most
/// patterns wins, ties broken by declaration order, defaulting to `General`. Port of
/// `categories.rs::classify`.
pub fn classify_request_type(text: &str) -> RequestType {
    let mut best = RequestType::General;
    let mut best_score = 0usize;
    for (rt, score) in vote_scores(text) {
        if score > best_score {
            best_score = score;
            best = rt;
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

/// Level-3 outcome when the complexity router is enabled: request type (MiniLM or
/// regex), 0–5 internal score, official 1–5 field, and the mapped [`Tier`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComplexityRoute {
    pub tier: Tier,
    pub request_type: RequestType,
    pub complexity: ComplexityScore,
    pub semantic_confidence: Option<f32>,
}

/// Classify request type (MiniLM when available, else regex) and pick a tier from
/// the deterministic 0–5 complexity policy. Does not Thompson-sample. `provider`
/// is logged only — the policy is provider-independent; registry resolution still
/// happens in [`super::route_model`].
pub fn classify_by_complexity(
    query: &str,
    provider: &str,
    semantic: Option<&SemanticClassifier>,
) -> ComplexityRoute {
    let regex_type = classify_request_type(query);
    let (request_type, semantic_confidence) = match semantic {
        Some(clf) => clf.classify_or_fallback(query, regex_type),
        None => (regex_type, None),
    };
    let complexity = score_complexity(query, request_type);
    let tier = tier_for_internal_score(complexity.internal);
    let preview: String = query.chars().take(120).collect();
    tracing::info!(
        target: "nasiko::llm_router::classifier",
        provider = %provider,
        query_chars = query.chars().count(),
        query_preview = %preview,
        request_type = %request_type.as_str(),
        regex_request_type = %regex_type.as_str(),
        semantic_confidence = ?semantic_confidence,
        internal_complexity = complexity.internal,
        official_complexity = complexity.official,
        complexity_reason = %complexity.reason.as_str(),
        classified_tier = ?tier,
        "classifier: MiniLM/regex request type + deterministic complexity score → tier"
    );
    ComplexityRoute {
        tier,
        request_type,
        complexity,
        semantic_confidence,
    }
}

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
// 5. P2 decision interface — model-agnostic `classify(query, context)`
// --------------------------------------------------------------------------

/// Input to a [`RequestClassifier`]: the latest user prompt plus optional
/// retrieved context (tool results, docs, conversation summary).
///
/// The regex default uses the query only (behaviour unchanged when the
/// classifier is off); local/hosted backends may use the context to adjust
/// complexity and confidence.
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// One classification decision.
///
/// * `request_type` — one of the router's seven [`RequestType`]s.
/// * `complexity` — official P2 field in `1..=5` (`0` internal maps to `1`,
///   see [`official_complexity`]).
/// * `confidence` — `0..=1`. Below the configured low-confidence threshold
///   the router treats the decision as a fallback (regex tier), never an error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

impl Classification {
    /// Cost-aware tier for this decision: 1–2 → Tier3, 3 → Tier2, 4–5 → Tier1.
    /// Matches [`tier_for_internal_score`] for every official value (internal
    /// `0` and `1` both land on Tier3), so the trait path and the legacy path
    /// agree on tiers for identical inputs.
    pub fn tier(&self) -> Tier {
        match self.complexity {
            0..=2 => Tier::Tier3,
            3 => Tier::Tier2,
            _ => Tier::Tier1,
        }
    }
}

/// Why a classification failed. Any error (or timeout — see
/// [`classify_with_fallback`]) makes the caller fall back to the regex result.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClassifyError {
    /// Backend selected but not configured (e.g. hosted with no endpoint).
    #[error("classifier backend not configured: {0}")]
    NotConfigured(String),
    /// Embedding / model inference failed.
    #[error("classifier inference failed: {0}")]
    Inference(String),
    /// Hosted backend transport failure.
    #[error("classifier transport failed: {0}")]
    Transport(String),
    /// Hosted backend returned an unusable payload.
    #[error("classifier returned an invalid response: {0}")]
    InvalidResponse(String),
}

/// Model-agnostic decision interface (P2 required scope §1).
///
/// Backends: [`RegexClassifier`] (default, behaviour unchanged),
/// [`HeuristicClassifier`] (deterministic local, no model download),
/// [`SemanticClassifier`](super::minilm::SemanticClassifier) (MiniLM ONNX),
/// [`HostedClassifier`] (OpenAI-compatible endpoint). Async because hosted
/// backends make network calls. The router holds an `Arc<dyn RequestClassifier>`.
#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Stable backend name for logs and eval (`regex`, `heuristic`, `minilm`, `hosted`).
    fn name(&self) -> &str;
    /// Classify `input`. Implementations are deterministic for identical
    /// inputs (no sampling; hosted backends should run at temperature 0).
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// The regex baseline as a [`RequestClassifier`].
///
/// * Type from [`classify_request_type`] on the query only; `context` ignored.
/// * Complexity from [`score_complexity`] (query only), mapped to official 1–5.
/// * Fixed confidence: `0.30` for empty input, `0.50` for `General`,
///   `0.65` for any pattern-matched type. Infallible by construction.
pub struct RegexClassifier;

impl RegexClassifier {
    pub fn classify_sync(&self, input: &ClassifyInput<'_>) -> Classification {
        let query = input.query;
        let request_type = classify_request_type(query);
        let complexity = score_complexity(query, request_type).official;
        let confidence = if query.trim().is_empty() {
            0.30
        } else if request_type == RequestType::General {
            0.50
        } else {
            0.65
        };
        Classification {
            request_type,
            complexity,
            confidence,
        }
    }
}

#[async_trait::async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_sync(input))
    }
}

/// Deterministic local backend: regex votes + context-aware complexity.
///
/// * Type: same vote-count winner as the regex (ties → declaration order).
/// * Complexity: [`score_complexity`] on the query, plus one notch (capped at
///   5) when `context` exceeds 1500 chars — more retrieved material to
///   synthesize means a harder task. Empty query still scores official `1`.
/// * Confidence from the vote margin: no votes `0.35`, tie `0.50`,
///   margin 1 `0.65`, margin ≥ 2 `0.90`; −`0.15` when ≥ 2 categories match
///   (multi-intent), −`0.10` when the text exceeds 600 chars with ≤ 1 vote
///   (noisy/padded). Clamped to `0..=1`. Pure function of the input: no
///   sampling, no IO, no timing dependence.
///
/// Token-optimization role: confidence and complexity feed the existing
/// complexity→tier policy, so cheap (Tier3) models serve simple prompts and
/// strong (Tier1) models serve hard ones — savings come from routing, never
/// from rewriting the prompt.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicClassifier;

impl HeuristicClassifier {
    pub fn classify_sync(&self, input: &ClassifyInput<'_>) -> Classification {
        let query = input.query;
        let request_type = classify_request_type(query);
        let mut internal = score_complexity(query, request_type).internal;
        if input.context.is_some_and(|c| c.chars().count() > 1500) {
            internal = internal.saturating_add(1).min(5);
        }
        let scores = vote_scores(query);
        Classification {
            request_type,
            complexity: official_complexity(internal),
            confidence: heuristic_confidence(query, &scores),
        }
    }
}

#[async_trait::async_trait]
impl RequestClassifier for HeuristicClassifier {
    fn name(&self) -> &str {
        "heuristic"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_sync(input))
    }
}

// fn heuristic_confidence(query: &str, scores: &[(RequestType, usize)]) -> f32 {
//     if query.trim().is_empty() {
//         return 0.30;
//     }
//     let mut best = 0usize;
//     let mut second = 0usize;
//     let mut matched = 0usize;
//     for (_, s) in scores {
//         if *s > 0 {
//             matched += 1;
//         }
//         if *s > best {
//             second = best;
//             best = *s;
//         } else if *s > second {
//             second = *s;
//         }
//     }
//     let mut conf = match best {
//         0 => 0.35,
//         _ if best == second => 0.50,
//         _ if best - second == 1 => 0.65,
//         _ => 0.90,
//     };
//     if matched >= 2 {
//         conf -= 0.15;
//     }
//     if query.chars().count() > 600 && best <= 1 {
//         conf -= 0.10;
//     }
//     let mut conf: f32 = match best {
// }
fn heuristic_confidence(query: &str, scores: &[(RequestType, usize)]) -> f32 {
    if query.trim().is_empty() {
        return 0.30;
    }

    let mut best = 0usize;
    let mut second = 0usize;
    let mut matched = 0usize;

    for (_, s) in scores {
        if *s > 0 {
            matched += 1;
        }

        if *s > best {
            second = best;
            best = *s;
        } else if *s > second {
            second = *s;
        }
    }

    let mut conf: f32 = match best {
        0 => 0.35,
        _ if best == second => 0.50,
        _ if best - second == 1 => 0.65,
        _ => 0.90,
    };

    if matched >= 2 {
        conf -= 0.15;
    }

    if query.chars().count() > 600 && best <= 1 {
        conf -= 0.10;
    }

    conf.clamp(0.0, 1.0)
}
/// MiniLM as a [`RequestClassifier`]: semantic request type via
/// `classify_or_fallback` (regex backstop below confidence threshold),
/// complexity from [`score_complexity`], confidence = MiniLM cosine when
/// trusted, else the regex fixed value (`0.50`/`0.65`).
#[async_trait::async_trait]
impl RequestClassifier for super::minilm::SemanticClassifier {
    fn name(&self) -> &str {
        "minilm"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let regex_type = classify_request_type(input.query);
        let (request_type, semantic_confidence) =
            self.classify_or_fallback(input.query, regex_type);
        let complexity = score_complexity(input.query, request_type).official;
        let confidence = match semantic_confidence {
            Some(c) if c >= self.min_confidence => c.clamp(0.0, 1.0),
            _ => RegexClassifier.classify_sync(input).confidence,
        };
        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

/// Hosted backend: POSTs `{query, context}` to an OpenAI-compatible endpoint
/// and parses `{request_type, complexity, confidence}`.
///
/// Expected response shape: `{"request_type": "<snake>", "complexity": 1-5,
/// "confidence": 0-1}`. Unknown `request_type` values and out-of-range numbers
/// are errors (never guessed): the caller falls back to regex. An empty
/// endpoint is [`ClassifyError::NotConfigured`]. Run the model at temperature
/// 0 for determinism; per-call latency is measured by the caller.
pub struct HostedClassifier {
    endpoint: String,
    model: String,
    client: reqwest::Client,
}

impl HostedClassifier {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        if self.endpoint.is_empty() {
            return Err(ClassifyError::NotConfigured(
                "CLASSIFIER_ENDPOINT is empty".into(),
            ));
        }
        let res = self
            .client
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "model": self.model,
                "temperature": 0,
                "query": input.query,
                "context": input.context.unwrap_or_default(),
            }))
            .send()
            .await
            .map_err(|e| ClassifyError::Transport(e.to_string()))?;
        if !res.status().is_success() {
            return Err(ClassifyError::InvalidResponse(format!(
                "endpoint returned {}",
                res.status()
            )));
        }
        let body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| ClassifyError::InvalidResponse(e.to_string()))?;
        let rt = body
            .get("request_type")
            .and_then(|v| v.as_str())
            .and_then(RequestType::from_wire)
            .ok_or_else(|| {
                ClassifyError::InvalidResponse("missing/unknown request_type".into())
            })?;
        let complexity = body
            .get("complexity")
            .and_then(|v| v.as_u64())
            .map(|c| (c as u8).clamp(1, 5))
            .ok_or_else(|| ClassifyError::InvalidResponse("missing complexity".into()))?;
        let confidence = body
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|c| (c as f32).clamp(0.0, 1.0))
            .ok_or_else(|| ClassifyError::InvalidResponse("missing confidence".into()))?;
        Ok(Classification {
            request_type: rt,
            complexity,
            confidence,
        })
    }
}

/// Select a backend by configuration value (never reads env — see `config.rs`).
///
/// * `regex` (or unknown) → [`RegexClassifier`] — the out-of-the-box default.
/// * `heuristic` / `local` → [`HeuristicClassifier`] — deterministic, no model
///   download, no network.
/// * `minilm` / `semantic` → the startup-loaded [`SemanticClassifier`](super::minilm::SemanticClassifier)
///   when present, else regex (fail-closed to the default, never an outage).
/// * `hosted` without an endpoint → regex. Wire [`HostedClassifier`]
///   explicitly with `CLASSIFIER_ENDPOINT` when a proxy is configured.
pub fn build_classifier(
    backend: &str,
    semantic: Option<Arc<super::minilm::SemanticClassifier>>,
) -> Arc<dyn RequestClassifier> {
    match backend.trim().to_ascii_lowercase().as_str() {
        "heuristic" | "local" => Arc::new(HeuristicClassifier),
        "minilm" | "semantic" => match semantic {
            Some(s) => s,
            None => Arc::new(RegexClassifier),
        },
        _ => Arc::new(RegexClassifier),
    }
}

/// Run `primary` with a timeout; on error or timeout fall back to the regex
/// result. Returns the decision and whether the fallback fired (count it —
/// P2 reports fallback rate). The regex fallback is infallible, so this never
/// returns `Err`: routing always has a decision.
pub async fn classify_with_fallback(
    primary: &dyn RequestClassifier,
    input: &ClassifyInput<'_>,
    timeout_ms: u64,
) -> (Classification, bool) {
    let timeout = std::time::Duration::from_millis(timeout_ms.max(1));
    match tokio::time::timeout(timeout, primary.classify(input)).await {
        Ok(Ok(c)) => (c, false),
        _ => (RegexClassifier.classify_sync(input), true),
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

    #[test]
    fn complexity_route_maps_score_to_tier_without_thompson() {
        let r = classify_by_complexity("hello there", "anthropic", None);
        assert_eq!(r.request_type, RequestType::General);
        assert_eq!(r.complexity.internal, 0);
        assert_eq!(r.complexity.official, 1);
        assert_eq!(r.tier, Tier::Tier3);

        let r = classify_by_complexity(
            "write a python function that sorts a list",
            "anthropic",
            None,
        );
        assert_eq!(r.request_type, RequestType::CodeGeneration);
        assert_eq!(r.complexity.internal, 2);
        assert_eq!(r.tier, Tier::Tier3);

        let r = classify_by_complexity(
            "how should I design this API?",
            "anthropic",
            None,
        );
        assert_eq!(r.request_type, RequestType::TechnicalDesign);
        assert!(r.complexity.internal >= 4);
        assert_eq!(r.tier, Tier::Tier1);
    }

    // --- P2 decision interface ---

    #[test]
    fn classification_tier_matches_internal_policy_for_all_official_values() {
        for official in 1u8..=5 {
            let c = Classification {
                request_type: RequestType::General,
                complexity: official,
                confidence: 0.9,
            };
            assert_eq!(
                c.tier(),
                tier_for_internal_score(official),
                "official {official}"
            );
        }
    }

    #[test]
    fn regex_classifier_documents_fixed_confidence_and_clamped_complexity() {
        let r = RegexClassifier;
        let empty = r.classify_sync(&ClassifyInput {
            query: "   ",
            context: None,
        });
        assert_eq!(empty.request_type, RequestType::General);
        assert_eq!(empty.complexity, 1);
        assert!((empty.confidence - 0.30).abs() < 1e-6);

        let general = r.classify_sync(&ClassifyInput {
            query: "hello there",
            context: Some("ignored context must not change regex behaviour"),
        });
        assert_eq!(general.complexity, 1);
        assert!((general.confidence - 0.50).abs() < 1e-6);

        let code = r.classify_sync(&ClassifyInput {
            query: "write a python function that sorts a list",
            context: None,
        });
        assert_eq!(code.request_type, RequestType::CodeGeneration);
        assert!((code.confidence - 0.65).abs() < 1e-6);
        assert!((1..=5).contains(&code.complexity));
    }

    #[test]
    fn heuristic_is_deterministic_and_context_aware() {
        let h = HeuristicClassifier;
        let input = ClassifyInput {
            query: "explain what this function does",
            context: Some("fn sort(xs: &mut [i32]) { xs.sort(); }"),
        };
        let a = h.classify_sync(&input);
        let b = h.classify_sync(&input);
        assert_eq!(a, b);
        assert_eq!(a.request_type, RequestType::CodeUnderstanding);

        // A large context bumps complexity by one notch (capped at 5).
        let big_ctx = "x".repeat(2000);
        let with_big = h.classify_sync(&ClassifyInput {
            query: "explain what this function does",
            context: Some(&big_ctx),
        });
        assert!(with_big.complexity >= a.complexity);
        assert!(with_big.complexity <= 5);
    }

    #[test]
    fn heuristic_confidence_penalises_multi_intent_and_padding() {
        let h = HeuristicClassifier;
        let single = h.classify_sync(&ClassifyInput {
            query: "what is the capital of France?",
            context: None,
        });
        let multi = h.classify_sync(&ClassifyInput {
            query: "what is the capital of France? Also write me a Python sort function and draft an email about the outage.",
            context: None,
        });
        assert!(
            multi.confidence < single.confidence,
            "multi-intent {multi:?} should be less confident than {single:?}"
        );
        let padded = h.classify_sync(&ClassifyInput {
            query: &format!("hello there {}", "please note ".repeat(80)),
            context: None,
        });
        assert!(padded.confidence <= 0.5);
    }

    #[test]
    fn build_classifier_defaults_to_regex_and_names_backends() {
        assert_eq!(build_classifier("regex", None).name(), "regex");
        assert_eq!(build_classifier("nonsense", None).name(), "regex");
        assert_eq!(build_classifier("hosted", None).name(), "regex");
        assert_eq!(build_classifier("minilm", None).name(), "regex");
        assert_eq!(build_classifier("heuristic", None).name(), "heuristic");
        assert_eq!(build_classifier("LOCAL", None).name(), "heuristic");
    }

    #[tokio::test]
    async fn fallback_fires_on_timeout_and_returns_regex_result() {
        struct Slow;
        #[async_trait::async_trait]
        impl RequestClassifier for Slow {
            fn name(&self) -> &str {
                "slow"
            }
            async fn classify(
                &self,
                _input: &ClassifyInput<'_>,
            ) -> Result<Classification, ClassifyError> {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Err(ClassifyError::Inference("too late".into()))
            }
        }
        let input = ClassifyInput {
            query: "hello there",
            context: None,
        };
        let (c, fell_back) = classify_with_fallback(&Slow, &input, 10).await;
        assert!(fell_back);
        assert_eq!(c, RegexClassifier.classify_sync(&input));

        let (c, fell_back) =
            classify_with_fallback(&HeuristicClassifier, &input, 1000).await;
        assert!(!fell_back);
        assert_eq!(c.request_type, RequestType::General);
    }

    #[tokio::test]
    async fn hosted_without_endpoint_is_not_configured() {
        let h = HostedClassifier::new("", "test-model");
        let err = h
            .classify(&ClassifyInput {
                query: "hi",
                context: None,
            })
            .await
            .expect_err("empty endpoint must fail");
        assert!(matches!(err, ClassifyError::NotConfigured(_)));
    }
}
