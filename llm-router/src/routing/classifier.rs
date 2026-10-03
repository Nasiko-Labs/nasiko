//! Query classifier — maps an incoming query to a model [`Tier`] for the destination
//! provider.
//!
//! The classifier answers "how much model does this query need?" as a coarse tier; the
//! [tier registry](super::registry) then maps `(provider, tier)` to a concrete model.
//! Provider selection and request translation happen elsewhere (the resolver / inbound
//! spokes) — the classifier only chooses the *strength* of the model.
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
//! P2 additionally exposes a model-agnostic [`RequestClassifier`] interface. The default
//! implementation remains the existing regex classifier, while [`HostedClassifier`] can
//! call an OpenAI-compatible hosted model such as Groq. Hosted inference always falls
//! back to the regex classifier when the network, endpoint, response, or model output
//! is invalid.
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
use reqwest::Client;
use serde::Deserialize;

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

    /// Inverse of [`RequestType::as_str`]; `None` for unknown values.
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
// P2. Model-agnostic request classifier interface
// --------------------------------------------------------------------------

/// Input to a model-backed request classifier.
#[derive(Debug, Clone)]
pub struct ClassifyInput {
    pub query: String,
    pub context: Option<String>,
}

/// Structured classification returned by a [`RequestClassifier`].
#[derive(Debug, Clone)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
    pub fallback: bool,
}

/// Model-agnostic interface for request classification.
///
/// Implementations can use the existing regex classifier, a local model, or
/// a hosted model. The rest of the router does not need to know which
/// backend produced the classification.
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    async fn classify(&self, input: ClassifyInput) -> Classification;
}

/// Adapter around the existing regex classifier.
///
/// This preserves the original Nasiko classifier as the default backend and
/// provides the deterministic fallback required by P2.
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    async fn classify(&self, input: ClassifyInput) -> Classification {
        Classification {
            request_type: classify_request_type(&input.query),
            complexity: 3,
            confidence: 0.0,
            fallback: false,
        }
    }
}

/// OpenAI-compatible hosted classifier.
///
/// Groq exposes an OpenAI-compatible chat-completions endpoint, so the
/// implementation is generic and can also work with another compatible
/// provider by changing the endpoint, model, and API key.
pub struct HostedClassifier {
    client: Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl HostedClassifier {
    /// Construct a hosted classifier.
    ///
    /// The HTTP client and timeout are initialized once. Per-request
    /// classification therefore measures inference rather than client setup.
    pub fn new(
        endpoint: String,
        model: String,
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder().timeout(timeout).build()?;

        Ok(Self {
            client,
            endpoint,
            model,
            api_key,
        })
    }

    /// Safe deterministic fallback to the existing regex classifier.
    pub fn fallback(query: &str) -> Classification {
        Classification {
            request_type: classify_request_type(query),
            complexity: 3,
            confidence: 0.0,
            fallback: true,
        }
    }

    /// Parse raw model response text into a [`Classification`].
    ///
    /// Handles optional markdown JSON fences, validates structure, clamps
    /// complexity to 1..=5 and confidence to 0.0..=1.0, and safely falls back
    /// to regex classification on invalid JSON, unknown request types, or non-finite confidence.
    pub fn parse_content(content: &str, query: &str) -> Classification {
        let trimmed = content.trim();
        let json_content = trimmed
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .or_else(|| {
                trimmed
                    .strip_prefix("```")
                    .and_then(|s| s.strip_suffix("```"))
            })
            .map(str::trim)
            .unwrap_or(trimmed);

        let parsed: ModelClassification = match serde_json::from_str(json_content) {
            Ok(parsed) => parsed,

            Err(_error) => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    "hosted classifier request failed; using regex fallback"
                );

                return Self::fallback(query);
            }
        };

        if !parsed.confidence.is_finite() {
            tracing::warn!(
                target: "nasiko::llm_router::classifier",
                confidence = ?parsed.confidence,
                "hosted classifier returned non-finite confidence; using regex fallback"
            );

            return Self::fallback(query);
        }

        let request_type = match RequestType::from_wire(&parsed.request_type) {
            Some(request_type) => request_type,

            None => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    request_type = %parsed.request_type,
                    "hosted classifier returned unknown request type; using regex fallback"
                );

                return Self::fallback(query);
            }
        };

        Classification {
            request_type,
            complexity: parsed.complexity.clamp(1, 5),
            confidence: parsed.confidence.clamp(0.0, 1.0),
            fallback: false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct HostedResponse {
    choices: Vec<HostedChoice>,
}

#[derive(Debug, Deserialize)]
struct HostedChoice {
    message: HostedMessage,
}

#[derive(Debug, Deserialize)]
struct HostedMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct ModelClassification {
    request_type: String,
    complexity: u8,
    confidence: f32,
}

#[async_trait]
impl RequestClassifier for HostedClassifier {
    async fn classify(&self, input: ClassifyInput) -> Classification {
        let system = r#"
You are a deterministic request classifier for an LLM router.

IMPORTANT CLASSIFICATION RULES:
- Classify the user's actual requested operation, not the subject matter or
  background context.
- Focus on what the user is asking the assistant to DO.
- Do not infer a more complex task from words describing a larger problem,
  proposed future work, comments, TODOs, or surrounding context.
- If the requested operation is a small code edit, classify it as
  code_generation even if the surrounding text discusses a large software
  redesign.
- If the user asks to rewrite, summarize, edit, draft, or transform prose,
  classify it as writing unless the primary task is technical design or
  analytical reasoning.
- Use general only when the requested operation genuinely does not fit any
  of the other categories.


Classify the user's PRIMARY task into exactly one request_type:

- code_generation
  The user wants code written, modified, fixed, or implemented.

- code_understanding
  The user wants existing code explained, interpreted, traced, or debugged.

- technical_design
  The user primarily wants a system, architecture, API, migration, component,
  or implementation design. The central deliverable is a proposed design.

- analytical_reasoning
  The user primarily wants diagnosis, investigation, deduction, comparison,
  reconstruction of events, causal reasoning, evidence analysis, uncertainty
  analysis, or validation of competing explanations. A request can mention a
  repair or design as a secondary deliverable and still be analytical_reasoning.

- writing
  The user wants prose rewritten, drafted, summarized, edited, or adapted
  for an audience.

- factual_lookup
  The user primarily wants a straightforward fact, definition, or short answer.

- general
  The request does not clearly fit another category.

Important distinction between technical_design and analytical_reasoning:

Choose analytical_reasoning when the PRIMARY work is to determine what happened,
why something happened, what can be inferred from evidence, reconstruct possible
interleavings/events, identify uncertainty, test competing explanations, or
derive conclusions from supplied facts.

Choose technical_design when the PRIMARY work is to propose how a system should
be built or changed, including architecture, components, interfaces, migration
plans, rollout plans, or failure-recovery design.

If a request contains both diagnosis and a proposed fix, classify according to
the PRIMARY intellectual task. Detailed investigation, evidence reconstruction,
event-by-event reasoning, invariants, uncertainty, or competing hypotheses
should favor analytical_reasoning even when a repair/design is requested.

Use the supplied context as evidence about the request, but classify the user's
actual task rather than merely counting technical or code-related keywords.

For complexity, judge the work actually requested, not the complexity of the
background system or subject being discussed:

1 = trivial single operation, tiny edit, direct lookup, or simple transformation
2 = straightforward task with limited constraints or a short explanation
3 = multi-step task with moderate constraints, several requirements, or
    meaningful code/prose transformation
4 = substantial reasoning, design, multiple interacting constraints, or
    non-trivial implementation
5 = intricate cross-component reasoning, temporal reasoning, investigation,
    validation, competing hypotheses, or many interacting constraints
Calibration examples:

- "Explain what this old value function does and how it behaves"
  -> code_understanding, complexity 2.
  Explanation of existing code is not automatically a multi-step task.

- "Build a parser that handles nested structures, validates constraints,
  and includes tests"
  -> code_generation, complexity 3.
  Multiple implementation requirements and tests make this more than a
  trivial edit.

- "Diagnose a distributed refresh/session concurrency problem involving
  interleavings and determine what happened"
  -> analytical_reasoning, complexity 5.
  Cross-component concurrency, temporal ordering, and causal investigation
  are highly complex even when no code is requested.

- "Summarize these release notes while preserving the important changes and
  constraints"
  -> writing, complexity 3.
  Summarization and transformation of supplied prose is writing, not general.

When choosing between categories, prefer the explicit requested operation
over the topic or nouns appearing in the request.

A request must not receive a higher complexity merely because its context
mentions a complicated system. A one-line change remains complexity 1 even
if the surrounding problem is architecturally complex.

For confidence:
Return your estimated confidence in the selected request_type, from 0.0 to 1.0.

Return ONLY a JSON object with exactly these fields:

{
  "request_type": "...",
  "complexity": 1,
  "confidence": 0.0
}

Do not include markdown.
Do not include explanations.
"#;

        let user = match input.context.as_deref() {
            Some(context) if !context.is_empty() => {
                format!("Context:\n{}\n\nUser request:\n{}", context, input.query)
            }
            _ => {
                format!("User request:\n{}", input.query)
            }
        };

        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "messages": [
                {
                    "role": "system",
                    "content": system
                },
                {
                    "role": "user",
                    "content": user
                }
            ]
        });

        let response = match self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,

            Err(_error) => {
                return Self::fallback(&input.query);
            }
        };

        if !response.status().is_success() {
            return Self::fallback(&input.query);
        }

        let payload: HostedResponse = match response.json().await {
            Ok(payload) => payload,

            Err(_error) => {
                return Self::fallback(&input.query);
            }
        };

        let content = match payload.choices.first() {
            Some(choice) => choice.message.content.as_str(),

            None => {
                tracing::warn!(
                    target: "nasiko::llm_router::classifier",
                    "hosted classifier returned no choices; using regex fallback"
                );

                return Self::fallback(&input.query);
            }
        };

        Self::parse_content(content, &input.query)
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

/// Sample cap for the running mean — past this the mean stops chasing new observations, so a
/// cell's estimate is stable once well-sampled. Port of the reference `MAX_SAMPLES`.
pub const MAX_SAMPLES: i64 = 200;

/// Strength of the cold-start prior, in Beta pseudo-observations. Port of the reference
/// `PRIOR_PSEUDO_COUNT`.
const PRIOR_PSEUDO_COUNT: f64 = 4.0;

/// Quality/cost blend weights (`w_quality`, `w_cost`). The reference default: quality leads,
/// cost trims. Tunable — learning corrects any cold-start bias over time.
pub const DEFAULT_W_QUALITY: f64 = 0.7;
pub const DEFAULT_W_COST: f64 = 0.3;

/// A tier as a bandit arm: its nominal quality tier (for the cold-start prior), a relative
/// cost, and the request types it is expected to be good at (a prior bonus).
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
/// well-sampled estimate stays stable. Port of the reference `scoring.rs::update_cell`.
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
/// `cells` are the provider's learned quality estimates (empty → pure cold-start priors);
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

    // --- request-type classifier ---

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

        assert!(beta_sample(0.0, 0.0, &mut rng).is_finite());
    }

    // --- Thompson tier selection ---

    #[test]
    fn thompson_converges_to_the_learned_best_tier() {
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

    // --- P2 model-agnostic classifier ---

    #[tokio::test]
    async fn regex_request_classifier_uses_existing_classifier() {
        let classifier = RegexClassifier;

        let result = classifier
            .classify(ClassifyInput {
                query: "write a python function that sorts a list".to_string(),
                context: None,
            })
            .await;

        assert_eq!(result.request_type, RequestType::CodeGeneration);

        assert_eq!(result.complexity, 3);

        assert_eq!(result.confidence, 0.0);
    }

    #[tokio::test]
    async fn hosted_classifier_falls_back_on_failure() {
        let classifier = HostedClassifier::new(
            "http://127.0.0.1:1/invalid".to_string(),
            "test-model".to_string(),
            "test-key".to_string(),
            Duration::from_millis(100),
        )
        .expect("client");

        let result = classifier
            .classify(ClassifyInput {
                query: "write a python function that sorts a list".to_string(),
                context: None,
            })
            .await;

        assert_eq!(result.request_type, RequestType::CodeGeneration);
        assert_eq!(result.complexity, 3);
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn hosted_response_parsing_valid_json() {
        let json = r#"{"request_type":"technical_design","complexity":4,"confidence":0.95}"#;
        let c = HostedClassifier::parse_content(json, "query");
        assert_eq!(c.request_type, RequestType::TechnicalDesign);
        assert_eq!(c.complexity, 4);
        assert!((c.confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn hosted_response_parsing_markdown_fence() {
        let json =
            "```json\n{\"request_type\":\"writing\",\"complexity\":2,\"confidence\":0.85}\n```";
        let c = HostedClassifier::parse_content(json, "query");
        assert_eq!(c.request_type, RequestType::Writing);
        assert_eq!(c.complexity, 2);
        assert!((c.confidence - 0.85).abs() < 1e-6);

        let json_plain_ticks = "```\n{\"request_type\":\"analytical_reasoning\",\"complexity\":5,\"confidence\":0.9}\n```";
        let c2 = HostedClassifier::parse_content(json_plain_ticks, "query");
        assert_eq!(c2.request_type, RequestType::AnalyticalReasoning);
        assert_eq!(c2.complexity, 5);
        assert!((c2.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn invalid_hosted_response_fallback() {
        let query = "build me a python script that parses CSV";
        // Malformed JSON falls back to regex classification
        let c1 = HostedClassifier::parse_content("not a json payload", query);
        assert_eq!(c1.request_type, RequestType::CodeGeneration);
        assert_eq!(c1.complexity, 3);
        assert_eq!(c1.confidence, 0.0);

        // Unknown request type falls back to regex
        let unknown = r#"{"request_type":"unknown_type","complexity":4,"confidence":0.9}"#;
        let c2 = HostedClassifier::parse_content(unknown, query);
        assert_eq!(c2.request_type, RequestType::CodeGeneration);
        assert_eq!(c2.complexity, 3);
        assert_eq!(c2.confidence, 0.0);

        // Non-finite confidence falls back to regex
        let nan_conf = r#"{"request_type":"code_generation","complexity":4,"confidence":NaN}"#;
        let c3 = HostedClassifier::parse_content(nan_conf, query);
        assert_eq!(c3.request_type, RequestType::CodeGeneration);
        assert_eq!(c3.complexity, 3);
        assert_eq!(c3.confidence, 0.0);
    }

    #[test]
    fn complexity_bounds_clamped() {
        let low = r#"{"request_type":"writing","complexity":0,"confidence":0.9}"#;
        let c_low = HostedClassifier::parse_content(low, "query");
        assert_eq!(c_low.complexity, 1);

        let high = r#"{"request_type":"writing","complexity":10,"confidence":0.9}"#;
        let c_high = HostedClassifier::parse_content(high, "query");
        assert_eq!(c_high.complexity, 5);
    }

    #[test]
    fn confidence_bounds_clamped() {
        let low = r#"{"request_type":"writing","complexity":2,"confidence":-0.5}"#;
        let c_low = HostedClassifier::parse_content(low, "query");
        assert_eq!(c_low.confidence, 0.0);

        let high = r#"{"request_type":"writing","complexity":2,"confidence":1.5}"#;
        let c_high = HostedClassifier::parse_content(high, "query");
        assert_eq!(c_high.confidence, 1.0);
    }

    #[tokio::test]
    async fn hosted_network_failure_fallback() {
        let classifier = HostedClassifier::new(
            "http://127.0.0.1:1/nonexistent".to_string(),
            "test-model".to_string(),
            "test-key".to_string(),
            Duration::from_millis(50),
        )
        .expect("client");

        let result = classifier
            .classify(ClassifyInput {
                query: "how should I design this API?".to_string(),
                context: None,
            })
            .await;

        assert_eq!(result.request_type, RequestType::TechnicalDesign);
        assert_eq!(result.complexity, 3);
        assert_eq!(result.confidence, 0.0);
    }

    #[tokio::test]
    async fn classifier_fallback_behavior_matches_regex() {
        let query = "what is the capital of France?";
        let fallback = HostedClassifier::fallback(query);
        let regex = RegexClassifier
            .classify(ClassifyInput {
                query: query.to_string(),
                context: None,
            })
            .await;

        assert_eq!(fallback.request_type, regex.request_type);
        assert_eq!(fallback.complexity, regex.complexity);
        assert_eq!(fallback.confidence, regex.confidence);
    }
}
