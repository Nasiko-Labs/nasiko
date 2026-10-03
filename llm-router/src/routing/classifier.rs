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

// --------------------------------------------------------------------------
// 5. Model-agnostic decision interface (hackathon classifier track).
//
// The Thompson tier machinery above stays the single place that maps
// (query, provider) → Tier. This section adds the *decision interface* the
// router and the evaluator share: `classify(query, context)` producing
// `{request_type, complexity, confidence}` behind one trait, with the regex
// as the default backend and an optional hosted (OpenAI-compatible) backend
// that fails back to the regex. The hot routing path (`route_model`) is
// untouched: existing behavior is unchanged until an operator opts in.
// --------------------------------------------------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Input to a [`RequestClassifier`]: the latest user query plus optional
/// retrieved context (code snippet, prior summary, tool output).
#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

/// The classifier's decision for one request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    /// Coarse kind of work the request represents.
    pub request_type: RequestType,
    /// 1 = trivial single operation … 5 = intricate cross-component
    /// reasoning. Mirrors the public eval rubric.
    pub complexity: u8,
    /// 0.0–1.0 self-reported certainty. Below
    /// [`LOW_CONFIDENCE_THRESHOLD`] the caller should use the safe default
    /// tier instead of trusting the label.
    pub confidence: f32,
}

/// Below this confidence the label must NOT drive tier selection: route to
/// the safe default (the agent's configured model) and count a fallback.
pub const LOW_CONFIDENCE_THRESHOLD: f32 = 0.6;

/// Fixed complexity the regex backend reports. The vote-count regex carries
/// no complexity signal, so it reports the neutral middle rather than
/// pretending to know.
pub const REGEX_COMPLEXITY: u8 = 3;

/// Fixed confidence the regex backend reports. Deliberately below
/// [`LOW_CONFIDENCE_THRESHOLD`]: keyword votes are evidence, not certainty.
pub const REGEX_CONFIDENCE: f32 = 0.5;

impl Classification {
    /// Build a decision, rejecting out-of-range complexity/confidence
    /// fail-closed instead of clamping silently.
    pub fn new(
        request_type: RequestType,
        complexity: u8,
        confidence: f32,
    ) -> Result<Self, ClassifyError> {
        if !(1..=5).contains(&complexity) {
            return Err(ClassifyError::InvalidOutput(format!(
                "complexity {complexity} outside 1-5"
            )));
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(ClassifyError::InvalidOutput(format!(
                "confidence {confidence} outside 0-1"
            )));
        }
        Ok(Self {
            request_type,
            complexity,
            confidence,
        })
    }

    /// Whether this decision is too uncertain to drive tier selection.
    pub fn is_low_confidence(&self) -> bool {
        self.confidence < LOW_CONFIDENCE_THRESHOLD
    }
}

/// Every way classification can fail. Any variant routes the caller to the
/// regex fallback — a failed classifier must never break routing.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClassifyError {
    /// HTTP/network failure talking to the hosted backend.
    #[error("classifier transport error: {0}")]
    Transport(String),
    /// The request exceeded its deadline.
    #[error("classifier timed out")]
    Timeout,
    /// The backend answered, but the payload was not a valid decision
    /// (non-JSON, unknown request type, out-of-range numbers, …).
    #[error("invalid classifier output: {0}")]
    InvalidOutput(String),
    /// The backend is not configured well enough to run (missing endpoint
    /// or model for a hosted backend).
    #[error("classifier misconfigured: {0}")]
    Misconfigured(String),
}

/// Model-agnostic classifier: `classify(query, context)` → decision.
///
/// Implementations must be deterministic for identical input (temperature 0,
/// no sampling; a hosted model is as deterministic as the provider makes
/// it at temperature 0). `Send + Sync` so the router can hold one behind an
/// `Arc` across handler tasks.
#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    /// Stable backend name for logs and the eval report (`"regex"`, …).
    fn name(&self) -> &str;

    /// Classify one request. Returns `Err` on any failure; the caller falls
    /// back to the regex result and counts it.
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

/// The existing regex classifier as a [`RequestClassifier`].
///
/// Wraps [`classify_request_type`] on the **query only**: the vote-count
/// regex has no context model, so `context` is accepted by the interface
/// but ignored here (model backends use it). Reports the fixed
/// [`REGEX_COMPLEXITY`] / [`REGEX_CONFIDENCE`] documented above. Infallible
/// in practice — this is what every other backend falls back to.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexClassifier;

#[async_trait::async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let request_type = classify_request_type(input.query);
        Classification::new(request_type, REGEX_COMPLEXITY, REGEX_CONFIDENCE)
            .map_err(|e| ClassifyError::InvalidOutput(e.to_string()))
    }
}

/// System prompt for the hosted backend. Asks for exactly one JSON object
/// with the three decision fields and pins the 1–5 rubric to the public
/// eval's wording, so scores mean the same thing in both harnesses.
const HOSTED_SYSTEM_PROMPT: &str = "Classify the user request for model routing. Reply with exactly one JSON object, no other text: {\"request_type\": one of code_generation|code_understanding|technical_design|analytical_reasoning|writing|factual_lookup|general, \"complexity\": integer 1-5 where 1 = trivial single operation, 2 = straightforward, 3 = multi-step with limited constraints, 4 = substantial reasoning or design, 5 = intricate cross-component reasoning and validation, \"confidence\": number 0-1}.";

/// A hosted LLM used as the classifier over any OpenAI-compatible
/// `/chat/completions` endpoint (the Bedrock mantle endpoint speaks this
/// shape). Temperature 0, strict output validation, any failure surfaces as
/// `Err` for the fallback wrapper — never a guessed decision.
#[derive(Debug, Clone)]
pub struct HostedClassifier {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    timeout: Duration,
    http: reqwest::Client,
}

impl HostedClassifier {
    /// Build against `{endpoint}/chat/completions`. Empty endpoint or model
    /// is a [`ClassifyError::Misconfigured`] at *use* time (fail-closed);
    /// prefer [`ClassifierService::from_config`], which selects the regex
    /// instead when configuration is incomplete.
    pub fn new(
        endpoint: String,
        model: String,
        api_key: Option<String>,
        timeout: Duration,
        http: reqwest::Client,
    ) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            model,
            api_key,
            timeout,
            http,
        }
    }

    fn request_body(&self, input: &ClassifyInput<'_>) -> serde_json::Value {
        let user = match input.context {
            Some(ctx) if !ctx.trim().is_empty() => {
                format!("Request:\n{}\n\nContext:\n{}", input.query, ctx)
            }
            _ => format!("Request:\n{}", input.query),
        };
        serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "messages": [
                {"role": "system", "content": HOSTED_SYSTEM_PROMPT},
                {"role": "user", "content": user},
            ],
        })
    }
}

#[async_trait::async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        if self.endpoint.is_empty() || self.model.is_empty() {
            return Err(ClassifyError::Misconfigured(
                "hosted backend needs endpoint and model".to_string(),
            ));
        }
        let url = format!("{}/chat/completions", self.endpoint);
        let mut req = self
            .http
            .post(&url)
            .timeout(self.timeout)
            .json(&self.request_body(input));
        if let Some(key) = self.api_key.as_deref() {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                ClassifyError::Timeout
            } else {
                ClassifyError::Transport(e.to_string())
            }
        })?;
        if !resp.status().is_success() {
            return Err(ClassifyError::Transport(format!(
                "status {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClassifyError::InvalidOutput(format!("bad body: {e}")))?;
        let text = body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ClassifyError::InvalidOutput("missing message content".to_string()))?;
        parse_hosted_decision(text)
    }
}

/// Parse the first JSON object in model text into a validated decision.
/// Leading/trailing prose is tolerated; anything else is `InvalidOutput`.
fn parse_hosted_decision(text: &str) -> Result<Classification, ClassifyError> {
    let invalid = |why: &str| ClassifyError::InvalidOutput(why.to_string());
    let start = text.find('{').ok_or_else(|| invalid("no JSON object"))?;
    let mut stream =
        serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
    let value: serde_json::Value = stream
        .next()
        .ok_or_else(|| invalid("empty JSON"))?
        .map_err(|e| invalid(&format!("bad JSON: {e}")))?;
    let obj = value.as_object().ok_or_else(|| invalid("not an object"))?;
    let request_type = obj
        .get("request_type")
        .and_then(serde_json::Value::as_str)
        .and_then(RequestType::from_wire)
        .ok_or_else(|| invalid("bad request_type"))?;
    let complexity = obj
        .get("complexity")
        .and_then(serde_json::Value::as_u64)
        .filter(|c| (1..=5).contains(c))
        .ok_or_else(|| invalid("bad complexity"))? as u8;
    let confidence = obj
        .get("confidence")
        .and_then(serde_json::Value::as_f64)
        .filter(|c| c.is_finite() && (0.0..=1.0).contains(c))
        .ok_or_else(|| invalid("bad confidence"))? as f32;
    Classification::new(request_type, complexity, confidence).map_err(|e| invalid(&e.to_string()))
}

/// Transparent regex fallback around any backend. On inner `Err` the regex
/// result is returned and the fallback counter increments — the router stays
/// usable if the optional classifier disappears. [`classify_detailed`]
/// reports whether the fallback fired, so evals can measure fallback rate.
pub struct FallbackClassifier<C: RequestClassifier> {
    inner: C,
    fallback: RegexClassifier,
    fallbacks: AtomicU64,
}

impl<C: RequestClassifier> FallbackClassifier<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            fallback: RegexClassifier,
            fallbacks: AtomicU64::new(0),
        }
    }

    /// Classify, falling back to the regex on any inner failure.
    /// Returns the decision and whether the fallback fired.
    pub async fn classify_detailed(&self, input: &ClassifyInput<'_>) -> (Classification, bool) {
        match self.inner.classify(input).await {
            Ok(c) => (c, false),
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    backend = self.inner.name(),
                    error = %e,
                    "classifier backend failed; falling back to regex"
                );
                self.fallbacks.fetch_add(1, Ordering::Relaxed);
                // The regex backend is infallible; a failure here would be
                // a bug, so surface it rather than inventing a decision.
                let c = self
                    .fallback
                    .classify(input)
                    .await
                    .expect("regex fallback is infallible");
                (c, true)
            }
        }
    }

    /// How many fallbacks have fired since construction.
    pub fn fallbacks(&self) -> u64 {
        self.fallbacks.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl<C: RequestClassifier> RequestClassifier for FallbackClassifier<C> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.classify_detailed(input).await.0)
    }
}

/// The configured classifier the router and the evaluator share.
///
/// Holds the selected backend behind an `Arc<dyn RequestClassifier>` (what
/// the router would store) plus the fallback counter. Built by
/// [`ClassifierService::from_config`]; selection is explicit strings, never
/// environment reads — the binary's `config.rs` owns env parsing.
pub struct ClassifierService {
    inner: Arc<dyn RequestClassifier>,
    detailed: Arc<FallbackClassifier<HostedOrRegex>>,
    backend_name: String,
}

/// The two backends the service can hold. `HostedOrRegex::Regex` covers the
/// default path and every degraded configuration.
enum HostedOrRegex {
    Hosted(HostedClassifier),
    Regex(RegexClassifier),
}

#[async_trait::async_trait]
impl RequestClassifier for HostedOrRegex {
    fn name(&self) -> &str {
        match self {
            HostedOrRegex::Hosted(h) => h.name(),
            HostedOrRegex::Regex(r) => r.name(),
        }
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        match self {
            HostedOrRegex::Hosted(h) => h.classify(input).await,
            HostedOrRegex::Regex(r) => r.classify(input).await,
        }
    }
}

impl ClassifierService {
    /// Select the backend from explicit configuration (see
    /// `GatewayConfig::classifier_*`; env names live in `config.rs`).
    /// `"regex"` (or empty) is the default; `"hosted"` needs a non-empty
    /// endpoint and model, otherwise — like `"local"` (no local model is
    /// vendored) and unknown values — it degrades to the regex with a
    /// warning. Degrading, never failing, is what keeps routing usable.
    pub fn from_config(
        backend: &str,
        endpoint: &str,
        model: &str,
        api_key: Option<&str>,
        timeout: Duration,
        http: reqwest::Client,
    ) -> Self {
        let backend_name = match backend.trim().to_ascii_lowercase().as_str() {
            "hosted" | "remote" | "http" if !endpoint.is_empty() && !model.is_empty() => {
                "hosted".to_string()
            }
            other => {
                if !other.is_empty() && other != "regex" {
                    tracing::warn!(
                        target: "nasiko::llm_router::classifier",
                        backend = other,
                        "unknown or unprovisioned classifier backend; using regex"
                    );
                }
                "regex".to_string()
            }
        };
        let inner = if backend_name == "hosted" {
            HostedOrRegex::Hosted(HostedClassifier::new(
                endpoint.to_string(),
                model.to_string(),
                api_key.map(str::to_string),
                timeout,
                http,
            ))
        } else {
            HostedOrRegex::Regex(RegexClassifier)
        };
        let detailed = Arc::new(FallbackClassifier::new(inner));
        Self {
            inner: detailed.clone(),
            detailed,
            backend_name,
        }
    }

    /// Backend selected at construction (`"regex"` or `"hosted"`).
    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    /// Classify with fallback visibility: the decision plus whether the
    /// regex fallback fired for this call.
    pub async fn classify_detailed(&self, input: &ClassifyInput<'_>) -> (Classification, bool) {
        self.detailed.classify_detailed(input).await
    }

    /// Total fallbacks since construction.
    pub fn fallbacks(&self) -> u64 {
        self.detailed.fallbacks()
    }
}

#[async_trait::async_trait]
impl RequestClassifier for ClassifierService {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        self.inner.classify(input).await
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

    // --- decision interface: construction bounds ---

    #[test]
    fn classification_rejects_out_of_range_values() {
        assert!(Classification::new(RequestType::General, 0, 0.5).is_err());
        assert!(Classification::new(RequestType::General, 6, 0.5).is_err());
        assert!(Classification::new(RequestType::General, 3, -0.1).is_err());
        assert!(Classification::new(RequestType::General, 3, 1.1).is_err());
        assert!(Classification::new(RequestType::General, 3, f32::NAN).is_err());
        let ok = Classification::new(RequestType::Writing, 1, 0.0).unwrap();
        assert!(ok.is_low_confidence());
        assert!(
            !Classification::new(RequestType::Writing, 5, 1.0)
                .unwrap()
                .is_low_confidence()
        );
    }

    // --- decision interface: regex backend ---

    #[tokio::test]
    async fn regex_classifier_matches_the_wrapped_function_on_every_type() {
        let cases = [
            (
                "build me a python script that parses CSV",
                RequestType::CodeGeneration,
            ),
            (
                "explain what this function does",
                RequestType::CodeUnderstanding,
            ),
            (
                "how should I design this API?",
                RequestType::TechnicalDesign,
            ),
            (
                "calculate the probability that it rains tomorrow",
                RequestType::AnalyticalReasoning,
            ),
            (
                "draft an email to my team about the outage",
                RequestType::Writing,
            ),
            ("what is the capital of France?", RequestType::FactualLookup),
            ("hello there", RequestType::General),
        ];
        let backend = RegexClassifier;
        assert_eq!(backend.name(), "regex");
        for (query, expected) in cases {
            // With and without context: the regex has no context model, so
            // both must equal the query-only function result.
            for context in [None, Some("some surrounding code")] {
                let input = ClassifyInput { query, context };
                let c = backend.classify(&input).await.unwrap();
                assert_eq!(c.request_type, expected, "query: {query}");
                assert_eq!(c.request_type, classify_request_type(query));
                assert_eq!(c.complexity, REGEX_COMPLEXITY);
                assert_eq!(c.confidence, REGEX_CONFIDENCE);
            }
        }
    }

    #[tokio::test]
    async fn regex_reports_documented_fixed_values() {
        // The regex carries no complexity/confidence signal: fixed neutral
        // values, with confidence below the low-confidence threshold.
        let c = RegexClassifier
            .classify(&ClassifyInput {
                query: "anything",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(c.complexity, REGEX_COMPLEXITY);
        assert_eq!(c.confidence, REGEX_CONFIDENCE);
        assert!(c.is_low_confidence());
    }

    #[tokio::test]
    async fn regex_is_deterministic() {
        let backend = RegexClassifier;
        let input = ClassifyInput {
            query: "write a python function that sorts a list",
            context: Some("ctx"),
        };
        let a = backend.classify(&input).await.unwrap();
        let b = backend.classify(&input).await.unwrap();
        assert_eq!(a, b);
    }

    // --- decision interface: hosted backend response parsing ---

    #[test]
    fn hosted_parses_a_valid_decision_with_surrounding_prose() {
        let c = parse_hosted_decision(
            "Sure. {\"request_type\":\"writing\",\"complexity\":2,\"confidence\":0.81} done.",
        )
        .unwrap();
        assert_eq!(c.request_type, RequestType::Writing);
        assert_eq!(c.complexity, 2);
        assert!((c.confidence - 0.81).abs() < 1e-6);
    }

    #[test]
    fn hosted_rejects_every_malformed_payload() {
        for bad in [
            "no json here",
            "{}",
            "{\"request_type\":\"coding\",\"complexity\":2,\"confidence\":0.5}",
            "{\"request_type\":\"writing\",\"complexity\":0,\"confidence\":0.5}",
            "{\"request_type\":\"writing\",\"complexity\":6,\"confidence\":0.5}",
            "{\"request_type\":\"writing\",\"complexity\":2.5,\"confidence\":0.5}",
            "{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":1.5}",
            "{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":\"high\"}",
            "{\"request_type\":\"writing\",\"complexity\":2}",
            "[1,2]",
        ] {
            assert!(parse_hosted_decision(bad).is_err(), "should reject: {bad}");
        }
    }

    // --- decision interface: hosted backend over HTTP (mockito) ---

    fn mock_ok_body() -> serde_json::Value {
        serde_json::json!({
            "choices": [{"message": {"content":
                "{\"request_type\":\"code_generation\",\"complexity\":4,\"confidence\":0.77}"
            }}]
        })
    }

    async fn mock_server(body: serde_json::Value, status: usize) -> mockito::ServerGuard {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/v1/chat/completions")
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create();
        server
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn hosted_success_returns_the_model_decision() {
        let server = mock_server(mock_ok_body(), 200).await;
        let backend = HostedClassifier::new(
            format!("{}/v1", server.url()),
            "test-model".into(),
            None,
            Duration::from_secs(10),
            test_client(),
        );
        let (c, fallback) = FallbackClassifier::new(backend)
            .classify_detailed(&ClassifyInput {
                query: "q",
                context: None,
            })
            .await;
        assert!(!fallback);
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(c.complexity, 4);
    }

    #[tokio::test]
    async fn hosted_http_error_falls_back_and_counts() {
        let server = mock_server(serde_json::json!({"error": "boom"}), 500).await;
        let inner = HostedClassifier::new(
            format!("{}/v1", server.url()),
            "m".into(),
            None,
            Duration::from_secs(10),
            test_client(),
        );
        let service = FallbackClassifier::new(inner);
        let (c, fallback) = service
            .classify_detailed(&ClassifyInput {
                query: "write a python function",
                context: None,
            })
            .await;
        assert!(fallback);
        assert_eq!(service.fallbacks(), 1);
        // Fallback is the regex decision for the query.
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(c.complexity, REGEX_COMPLEXITY);
    }

    #[tokio::test]
    async fn hosted_garbage_body_falls_back() {
        let server = mock_server(serde_json::json!({"choices": []}), 200).await;
        let inner = HostedClassifier::new(
            format!("{}/v1", server.url()),
            "m".into(),
            None,
            Duration::from_secs(10),
            test_client(),
        );
        let service = FallbackClassifier::new(inner);
        let (_, fallback) = service
            .classify_detailed(&ClassifyInput {
                query: "hello",
                context: None,
            })
            .await;
        assert!(fallback);
        assert_eq!(service.fallbacks(), 1);
    }

    #[tokio::test]
    async fn hosted_out_of_range_values_fall_back() {
        let server = mock_server(
            serde_json::json!({"choices": [{"message": {"content":
                "{\"request_type\":\"writing\",\"complexity\":9,\"confidence\":0.5}"
            }}]}),
            200,
        )
        .await;
        let inner = HostedClassifier::new(
            format!("{}/v1", server.url()),
            "m".into(),
            None,
            Duration::from_secs(10),
            test_client(),
        );
        let (c, fallback) = FallbackClassifier::new(inner)
            .classify_detailed(&ClassifyInput {
                query: "draft an email",
                context: None,
            })
            .await;
        assert!(fallback);
        assert_eq!(c.request_type, RequestType::Writing);
    }

    #[tokio::test]
    async fn unreachable_host_falls_back_without_hanging() {
        // 127.0.0.1:9 is the discard port: connection refused immediately,
        // so this exercises the transport-failure fallback deterministically.
        let inner = HostedClassifier::new(
            "http://127.0.0.1:9".into(),
            "m".into(),
            None,
            Duration::from_secs(5),
            test_client(),
        );
        let (c, fallback) = FallbackClassifier::new(inner)
            .classify_detailed(&ClassifyInput {
                query: "hello there",
                context: None,
            })
            .await;
        assert!(fallback);
        assert_eq!(c.request_type, RequestType::General);
    }

    #[tokio::test]
    async fn misconfigured_hosted_errors_instead_of_guessing() {
        let backend = HostedClassifier::new(
            String::new(),
            String::new(),
            None,
            Duration::from_secs(5),
            test_client(),
        );
        assert!(matches!(
            backend
                .classify(&ClassifyInput {
                    query: "hi",
                    context: None
                })
                .await,
            Err(ClassifyError::Misconfigured(_))
        ));
    }

    // --- decision interface: service selection ---

    #[test]
    fn service_selects_backends_explicitly() {
        let http = test_client();
        let regex = ClassifierService::from_config(
            "regex",
            "",
            "",
            None,
            Duration::from_secs(1),
            http.clone(),
        );
        assert_eq!(regex.backend_name(), "regex");
        assert_eq!(regex.fallbacks(), 0);

        // Unknown and local backends degrade to regex (documented).
        for backend in ["local", "bogus", ""] {
            let svc = ClassifierService::from_config(
                backend,
                "",
                "",
                None,
                Duration::from_secs(1),
                http.clone(),
            );
            assert_eq!(svc.backend_name(), "regex", "backend: {backend}");
        }

        // "hosted" without endpoint/model degrades instead of misfiring.
        let degraded = ClassifierService::from_config(
            "hosted",
            "",
            "",
            None,
            Duration::from_secs(1),
            http.clone(),
        );
        assert_eq!(degraded.backend_name(), "regex");

        let hosted = ClassifierService::from_config(
            "hosted",
            "https://example.test/v1",
            "m",
            None,
            Duration::from_secs(1),
            http,
        );
        assert_eq!(hosted.backend_name(), "hosted");
    }

    #[tokio::test]
    async fn service_regex_path_is_deterministic() {
        let svc = ClassifierService::from_config(
            "regex",
            "",
            "",
            None,
            Duration::from_secs(1),
            test_client(),
        );
        let input = ClassifyInput {
            query: "what is the capital of France?",
            context: Some("geo"),
        };
        let (a, fa) = svc.classify_detailed(&input).await;
        let (b, fb) = svc.classify_detailed(&input).await;
        assert_eq!(a, b);
        assert!(!fa && !fb);
        assert_eq!(a.request_type, RequestType::FactualLookup);
        assert_eq!(svc.fallbacks(), 0);
    }
}
