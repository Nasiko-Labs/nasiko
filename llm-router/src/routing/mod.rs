//! Model routing — decides *which model* to call, within the destination provider the
//! resolver already fixed.
//!
//! The resolver ([`crate::resolver`]) still owns provider, credentials, and params; this
//! layer only overrides the **model**, and only at conversation/agent boundaries where
//! switching is safe. Everywhere else the model stays sticky (so tool-call state can't
//! drift). The decision follows a fixed five-level precedence — see [`route_model`].
//!
//! ```text
//! query + provider ──► classify() ──► Tier ──► registry::model_for(provider, Tier) ──► model
//! ```
//!
//! The [classifier](classifier::classify) buckets the query into a request type and
//! Thompson-samples a [`Tier`] over the provider's learned quality [cells](cells); feedback
//! from the user's next turn ([`classifier::signal`]) is folded back into those cells, so the
//! router learns which tier suffices for which kind of query. See [`route_model`].

pub mod attribution;
pub mod boundary;
pub mod cache;
pub mod cascade_classifier;
pub mod catalog;
pub mod cells;
pub mod classifier;
pub mod hosted_classifier;
pub mod local_classifier;
// The salience classifier itself — feature engine, weight loading, scoring, banding.
// Private to `routing`: only `salience.rs` (a sibling module) uses it directly, via
// `ClassifierSalienceGate`; `request_features.rs` shares its hashing/tokenizer helpers.
mod patterns;
pub mod pricing_sync;
pub mod registry;
pub mod request_features;
pub mod salience;
mod salience_classifier;

pub use boundary::{BoundarySignals, Mode, Phase};
pub use cache::{
    CachedDecision, DecisionCache, InMemoryDecisionCache, NoopCache, RedisCache,
    TieredDecisionCache,
};
pub use cascade_classifier::CascadeClassifier;
pub use cells::{CellStore, InMemoryCellStore, PgCellStore};
pub use classifier::{
    Classification, ClassifierStats, ClassifierStatsSnapshot, ClassifyError, ClassifyInput,
    ComplexityRouting, GuardedClassifier, RegexClassifier, RequestClassifier, RequestType, Tier,
    classify, classify_request_type, select_tier, signal,
};
pub use hosted_classifier::HostedClassifier;
pub use local_classifier::LocalClassifier;
pub use registry::{PgTierRegistry, TierRegistry};
pub use salience::{AllowAllGate, ClassifierSalienceGate, SalienceGate};

use rand::SeedableRng;
use rand::rngs::StdRng;

/// Which precedence level produced a routing decision — emitted as a structured tag so we
/// can see, per request, how the model was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    /// Level 1 — agent config is pinned; the classifier never ran.
    Pinned,
    /// Level 2 — served from the decision cache (a continuation turn).
    CacheHit,
    /// Level 2.5 — the salience gate judged the turn non-substantive (small talk); a cheap
    /// model was served WITHOUT classifying or pinning (the decision cache is not written).
    SmallTalk,
    /// Level 3 — the classifier ran at a safe boundary.
    Classified,
    /// Level 3 ran, but the classifier's confidence was below `min_confidence`: the
    /// configured/default model is served **without pinning** (no cache write), so the next
    /// boundary re-evaluates. The safe default for uncertain classifications.
    LowConfidence,
    /// Level 4 — the agent's configured (`llm_config`) model.
    Config,
    /// Level 5 — no `llm_config`: the resolver's passthrough model (the request's own
    /// provider/model, per the inbound SDK surface), or the platform default as the
    /// last-resort safety net when the request supplied none.
    Default,
}

/// Inputs the precedence chain needs, assembled by the caller from the resolved config,
/// the boundary signals, and the request.
pub struct RouteInputs<'a> {
    /// The agent's id (half of the cache key).
    pub agent_id: &'a str,
    /// The **destination** provider the resolver chose (fixes which registry we look up).
    pub provider: &'a str,
    /// The model to use when no boundary/cache/pin applies — the resolver's configured or
    /// default model (Levels 4/5).
    pub fallback_model: &'a str,
    /// Whether `fallback_model` came from explicit `llm_config` (Level 4) rather than the
    /// platform default (Level 5) — used only to tag the decision.
    pub has_llm_config: bool,
    /// The pinned model, if the agent's config is compliance-locked (Level 1). `None`
    /// until S4 wires the real field.
    pub pinned_model: Option<&'a str>,
    /// Per-config tier→model overrides from the user's `llm_configs` row. When set, the
    /// router checks these before the global `model_registry` at Level 3.
    pub tier1_model: Option<&'a str>,
    pub tier2_model: Option<&'a str>,
    pub tier3_model: Option<&'a str>,
    /// Per-request boundary tags (phase/mode/conv_id).
    pub signals: &'a BoundarySignals,
    /// The query to classify (latest user message text). `None` disables classification.
    pub query: Option<&'a str>,
    /// Bounded classification context ([`classification_context`]) — attached material or
    /// recent turns. `None` when the surface provides none.
    pub context: Option<&'a str>,
    /// Abstain threshold (`CLASSIFIER_MIN_CONFIDENCE`): below it Level 3 serves the
    /// configured model without pinning ([`RouteSource::LowConfidence`]). `0.0` never abstains.
    pub min_confidence: f32,
    /// `ROUTER_TIER_SEED`: `Some` makes tier selection a pure function of
    /// `(seed, provider, agent, conv_id, query)` and the learned cells; `None` uses entropy.
    pub tier_seed: Option<u64>,
    /// Opt-in complexity-aware tier selection ([`classifier::select_tier`]).
    pub complexity_routing: ComplexityRouting,
}

/// The outcome of routing: the model to call and how it was chosen.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub model: String,
    pub tier: Option<Tier>,
    pub source: RouteSource,
    /// The classifier's verdict when Level 3 decided (`Classified` or `LowConfidence`);
    /// `None` for every other source.
    pub classification: Option<Classification>,
}

/// Apply the five-level precedence and return the model to call.
///
/// This always returns something — router failure is never a user-visible outage:
///
/// 1. **Pinned** (`pinned_model` set) → return it directly. No classify, no cache write.
///    Compliance-locked agents.
/// 2. **Cache hit** on `(conv_id, agent_id)` → the sticky decision for this conversation.
///    Before returning it, the current turn's message is checked for a feedback
///    [`signal`](classifier::signal); if present it is credited to the cached decision's
///    `(tier, request_type)` via [`CellStore::observe`] — this is the learning write.
///    - **Salience gate (Level 2.5)** — at a fireable boundary with a cache miss, an
///      in-process classifier ([`SalienceGate`]) judges whether the turn is substantive. Small talk
///      short-circuits here: a cheap/default model is served with source
///      [`RouteSource::SmallTalk`] and the cache is **not** written, so a greeting can never
///      pin the session. Only substantive turns fall through to Level 3.
/// 3. **Classify** — only at a fireable boundary (`switch`/`cold_start` + `free_flowing`)
///    with a query present and a registry entry for `(provider, tier)`. Asks the configured
///    [`RequestClassifier`] (query + bounded context) for `{request_type, complexity,
///    confidence}`; below `min_confidence` it serves the configured model without pinning
///    ([`RouteSource::LowConfidence`]). Otherwise loads the provider's learned cells,
///    Thompson-samples a tier ([`classifier::select_tier`]; seeded when `tier_seed` is set),
///    and writes the decision (incl. request type and complexity) through to the cache so the
///    next turn short-circuits at Level 2.
/// 4. **Config** — the agent's configured model (`has_llm_config`).
/// 5. **Default** — no `llm_config`: the resolver's `fallback_model`, which is the request's
///    own provider/model (passthrough) or the platform default as the last-resort safety net.
///
/// Learning happens *across* conversations, not within one: a conversation stays sticky to
/// its first-turn tier (Level 2), while the reward it generates updates the shared
/// provider-scoped cells that shape *future* conversations' cold-start picks.
pub async fn route_model(
    cache: &dyn DecisionCache,
    registry: &dyn TierRegistry,
    cell_store: &dyn CellStore,
    gate: &dyn SalienceGate,
    classifier: &dyn RequestClassifier,
    inputs: &RouteInputs<'_>,
) -> RouteDecision {
    tracing::info!(
        target: "nasiko::llm_router::routing",
        agent_id = %inputs.agent_id,
        provider = %inputs.provider,
        fallback_model = %inputs.fallback_model,
        has_llm_config = inputs.has_llm_config,
        pinned_model = ?inputs.pinned_model,
        conv_id = ?inputs.signals.conv_id,
        phase = ?inputs.signals.phase,
        mode = ?inputs.signals.mode,
        is_fireable_boundary = inputs.signals.is_fireable_boundary(),
        has_query = inputs.query.is_some(),
        "route_model: begin 5-level precedence resolution"
    );

    // Level 1 — pinned. Return directly; never cache, never re-route (that would defeat
    // the compliance lock — an unavailable pinned model surfaces downstream, not here).
    if let Some(model) = inputs.pinned_model {
        tracing::info!(
            target: "nasiko::llm_router::routing",
            agent_id = %inputs.agent_id,
            level = 1,
            source = ?RouteSource::Pinned,
            model = %model,
            "route_model: LEVEL 1 (Pinned) — agent is compliance-locked; using pinned model, classifier skipped, cache bypassed"
        );
        return RouteDecision {
            model: model.to_string(),
            tier: None,
            source: RouteSource::Pinned,
            classification: None,
        };
    }
    tracing::debug!(
        target: "nasiko::llm_router::routing",
        "route_model: LEVEL 1 (Pinned) skipped — agent not pinned"
    );

    // Levels 2 & 3 apply only within a conversation and only when the agent has an
    // explicit llm_config (opt-in to routing). No `conv_id` or no llm_config ⇒ the
    // router never fires and behaviour is identical to before this layer — straight to
    // the configured/default model.
    if inputs.has_llm_config
        && let Some(conv_id) = inputs.signals.conv_id.as_deref()
    {
        // Level 2 — cache hit: the sticky decision for this conversation+agent.
        tracing::debug!(
            target: "nasiko::llm_router::routing",
            agent_id = %inputs.agent_id, %conv_id,
            "route_model: LEVEL 2 (CacheHit) — looking up sticky decision for (conv_id, agent_id)"
        );
        if let Some(hit) = cache.get(conv_id, inputs.agent_id).await {
            // Learning write: this turn's user message is the verdict on the previous turn's
            // answer, which the cached decision identifies. Credit it to that (tier,
            // request_type). `signal` is conservative, so a genuine new question scores None
            // and earns no false credit.
            maybe_learn(cell_store, inputs, hit.tier, hit.request_type).await;
            tracing::info!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id,
                level = 2,
                source = ?RouteSource::CacheHit,
                model = %hit.model,
                tier = ?hit.tier,
                "route_model: LEVEL 2 (CacheHit) — reusing conversation-sticky model from decision cache"
            );
            return RouteDecision {
                model: hit.model,
                tier: hit.tier,
                source: RouteSource::CacheHit,
                classification: None,
            };
        }
        tracing::debug!(
            target: "nasiko::llm_router::routing",
            agent_id = %inputs.agent_id, %conv_id,
            "route_model: LEVEL 2 (CacheHit) miss — no sticky decision cached yet"
        );

        // Levels 2.5 & 3 — classify, but only at a boundary where re-selecting is safe.
        // (has_llm_config is already guarded by the outer block.)
        if inputs.signals.is_fireable_boundary()
            && let Some(query) = inputs.query
        {
            // Level 2.5 — salience gate. An in-process classifier decides whether this
            // turn is substantive enough to classify + pin. Small talk is served cheaply
            // and NEVER pins (no cache write), so a greeting can't fix the session's
            // model. Only substantive turns fall through to Level 3.
            tracing::info!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id, provider = %inputs.provider,
                query_preview = %inputs.query.map(query_preview).unwrap_or_default(),
                "route_model: LEVEL 2.5 (SalienceGate) — cache miss at a fireable boundary; asking the gate whether to classify this turn"
            );
            if !gate.is_substantive(query).await {
                let model = small_talk_model(registry, inputs).await;
                tracing::info!(
                    target: "nasiko::llm_router::routing",
                    agent_id = %inputs.agent_id, %conv_id,
                    source = ?RouteSource::SmallTalk,
                    model = %model,
                    has_llm_config = inputs.has_llm_config,
                    "route_model: LEVEL 2.5 (SmallTalk) — gate judged this turn NON-substantive; serving a cheap model WITHOUT classifying or pinning (cache NOT written, so the next turn is re-evaluated)"
                );
                return RouteDecision {
                    model,
                    tier: None,
                    source: RouteSource::SmallTalk,
                    classification: None,
                };
            }
            tracing::info!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id,
                "route_model: LEVEL 2.5 (SalienceGate) — gate judged this turn SUBSTANTIVE; proceeding to classify + pin (Level 3)"
            );

            tracing::info!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id, provider = %inputs.provider,
                "route_model: LEVEL 3 (Classified) — at fireable boundary with a query; invoking classifier"
            );
            // Ask the configured classifier (guarded in production: it never errs). It may do
            // network I/O, so it is awaited before any RNG exists.
            let started = std::time::Instant::now();
            let c = match classifier
                .classify(&ClassifyInput {
                    query,
                    context: inputs.context,
                })
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::classifier",
                        backend = classifier.name(),
                        error = %e,
                        "route_model: unguarded classifier failed; using the regex result"
                    );
                    RegexClassifier::classify_sync(query)
                }
            };
            let abstained = c.confidence < inputs.min_confidence;
            log_classification(
                classifier.name(),
                &c,
                abstained,
                started.elapsed().as_micros() as u64,
                query,
                inputs.context,
            );
            if abstained {
                tracing::info!(
                    target: "nasiko::llm_router::routing",
                    agent_id = %inputs.agent_id, %conv_id,
                    source = ?RouteSource::LowConfidence,
                    confidence = c.confidence,
                    min_confidence = inputs.min_confidence,
                    model = %inputs.fallback_model,
                    "route_model: LEVEL 3 (LowConfidence) — classifier below the confidence threshold; serving the configured model WITHOUT pinning"
                );
                return RouteDecision {
                    model: inputs.fallback_model.to_string(),
                    tier: None,
                    source: RouteSource::LowConfidence,
                    classification: Some(c),
                };
            }
            // Load the provider's learned quality, then Thompson-sample a tier. The RNG is a
            // `StdRng` (seeded per decision when `tier_seed` is set, else from entropy) created
            // after the last `.await`, so the handler future stays `Send`.
            let learned = cell_store.load(inputs.provider).await;
            let request_type = c.request_type;
            let tier = {
                let mut rng = routing_rng(
                    inputs.tier_seed,
                    inputs.provider,
                    inputs.agent_id,
                    conv_id,
                    query,
                );
                select_tier(&learned, &c, inputs.complexity_routing, &mut rng)
            };
            // Per-config tier override takes priority over the global registry.
            let config_override = match tier {
                Tier::Tier1 => inputs.tier1_model.map(str::to_string),
                Tier::Tier2 => inputs.tier2_model.map(str::to_string),
                Tier::Tier3 => inputs.tier3_model.map(str::to_string),
            };
            let tier_model = match config_override {
                Some(ref m) => {
                    tracing::info!(
                        target: "nasiko::llm_router::routing",
                        agent_id = %inputs.agent_id,
                        provider = %inputs.provider,
                        tier = ?tier,
                        model = %m,
                        "route_model: LEVEL 3 — using per-config tier override"
                    );
                    Some(m.clone())
                }
                None => registry.model_for(inputs.provider, tier).await,
            };
            match tier_model {
                Some(model) => {
                    let decision = CachedDecision {
                        model,
                        tier: Some(tier),
                        request_type: Some(request_type),
                        complexity: Some(c.complexity),
                        confidence: Some(c.confidence),
                    };
                    // Write-through so continuation turns read the cache (Level 2).
                    cache.put(conv_id, inputs.agent_id, &decision).await;
                    tracing::info!(
                        target: "nasiko::llm_router::routing",
                        agent_id = %inputs.agent_id, %conv_id,
                        level = 3,
                        source = ?RouteSource::Classified,
                        provider = %inputs.provider,
                        tier = ?tier,
                        request_type = %request_type.as_str(),
                        model = %decision.model,
                        "route_model: LEVEL 3 (Classified) — registry resolved (provider, tier) → model; wrote decision to cache for continuation turns"
                    );
                    return RouteDecision {
                        model: decision.model,
                        tier: Some(tier),
                        source: RouteSource::Classified,
                        classification: Some(c),
                    };
                }
                None => {
                    // Registry miss for this provider ⇒ fall through to configured/default model.
                    tracing::warn!(
                        target: "nasiko::llm_router::routing",
                        agent_id = %inputs.agent_id, %conv_id,
                        provider = %inputs.provider, tier = ?tier,
                        "route_model: LEVEL 3 (Classified) — registry has no model for (provider, tier); falling through to configured/default model"
                    );
                }
            }
        } else {
            tracing::debug!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id,
                is_fireable_boundary = inputs.signals.is_fireable_boundary(),
                has_query = inputs.query.is_some(),
                "route_model: LEVEL 3 (Classified) skipped — not a fireable boundary or no query (model stays sticky)"
            );
        }
    } else {
        tracing::debug!(
            target: "nasiko::llm_router::routing",
            agent_id = %inputs.agent_id,
            "route_model: LEVELS 2 & 3 skipped — no conv_id (not an orchestrated conversation); behaviour identical to pre-router"
        );
    }

    // Levels 4/5 — configured model, else platform default.
    let source = if inputs.has_llm_config {
        RouteSource::Config
    } else {
        RouteSource::Default
    };
    tracing::info!(
        target: "nasiko::llm_router::routing",
        agent_id = %inputs.agent_id,
        level = if inputs.has_llm_config { 4 } else { 5 },
        source = ?source,
        model = %inputs.fallback_model,
        "route_model: LEVEL {} ({}) — using {} model",
        if inputs.has_llm_config { 4 } else { 5 },
        if inputs.has_llm_config { "Config" } else { "Default" },
        if inputs.has_llm_config { "agent-configured (llm_config)" } else { "request-passthrough or platform-default" }
    );
    RouteDecision {
        model: inputs.fallback_model.to_string(),
        tier: None,
        source,
        classification: None,
    }
}

/// Credit the current turn's feedback to a prior decision, if there is any to credit.
///
/// The current user message (`inputs.query`) is the verdict on the answer the cached
/// `(tier, request_type)` produced last turn. A message with a clear
/// [`signal`](classifier::signal) folds a reward into that provider-scoped cell; anything
/// ambiguous (the usual case) is a no-op. Best-effort — a store failure is swallowed.
async fn maybe_learn(
    cell_store: &dyn CellStore,
    inputs: &RouteInputs<'_>,
    tier: Option<Tier>,
    request_type: Option<RequestType>,
) {
    let (Some(query), Some(tier), Some(rt)) = (inputs.query, tier, request_type) else {
        return;
    };
    let Some(observation) = signal(query) else {
        return;
    };
    tracing::info!(
        target: "nasiko::llm_router::routing",
        agent_id = %inputs.agent_id,
        provider = %inputs.provider,
        ?tier,
        request_type = %rt.as_str(),
        observation,
        "route_model: feedback signal detected in this turn — crediting the prior sticky decision's learned cell"
    );
    cell_store
        .observe(inputs.provider, tier, rt, observation)
        .await;
}

/// A short, log-safe preview of a query (first 120 chars) — mirrors the classifier's own
/// `query_preview` so both stages format the query the same way in the logs.
fn query_preview(q: &str) -> String {
    q.chars().take(120).collect()
}

/// The model to answer a non-substantive turn with (Level 2.5). Level 2.5 is only reached
/// inside the `has_llm_config` block, so the agent is always configured here: it gets its
/// cheapest available model — the per-config `tier3_model` override, else the global
/// registry's Tier3 for the provider, else the configured model as a last resort. Never
/// pins: the caller does not write the cache for this decision.
async fn small_talk_model(registry: &dyn TierRegistry, inputs: &RouteInputs<'_>) -> String {
    if let Some(m) = inputs.tier3_model {
        return m.to_string();
    }
    if let Some(m) = registry.model_for(inputs.provider, Tier::Tier3).await {
        return m;
    }
    inputs.fallback_model.to_string()
}

/// One telemetry line per Level 3 classification. Logs lengths and a 64-bit hash of the
/// query — never query or context text.
fn log_classification(
    backend: &str,
    c: &Classification,
    abstained: bool,
    latency_us: u64,
    query: &str,
    context: Option<&str>,
) {
    tracing::info!(
        target: "nasiko::llm_router::classifier",
        backend,
        request_type = %c.request_type.as_str(),
        complexity = c.complexity,
        confidence = c.confidence,
        abstained,
        latency_us,
        query_chars = query.chars().count(),
        query_hash = %format!("{:016x}", salience_classifier::fnv1a(query.as_bytes())),
        context_chars = context.map_or(0, |c| c.chars().count()),
        "classifier: classified query"
    );
}

/// The per-decision Thompson RNG. With a seed, the stream is a pure function of `(seed,
/// provider, agent, conv_id, query)`: identical inputs and learned state give an identical
/// tier, while different conversations still explore. Without one, it is seeded from
/// entropy, as before. Always a `StdRng`, which is `Send`.
fn routing_rng(
    seed: Option<u64>,
    provider: &str,
    agent_id: &str,
    conv_id: &str,
    query: &str,
) -> StdRng {
    match seed {
        Some(seed) => {
            let key = [
                &seed.to_le_bytes()[..],
                b"\x1f",
                provider.as_bytes(),
                b"\x1f",
                agent_id.as_bytes(),
                b"\x1f",
                conv_id.as_bytes(),
                b"\x1f",
                query.as_bytes(),
            ]
            .concat();
            StdRng::seed_from_u64(salience_classifier::fnv1a(&key))
        }
        None => StdRng::from_rng(&mut rand::rng()),
    }
}

/// Bounded context for the classifier: the A2A packed-history prefix (what
/// [`latest_user_query`] strips) when present, else the text of the last ≤ 2 non-system
/// messages before the latest user message, joined by newlines. Keeps the **last**
/// `max_chars` characters (the most recent material). `None` when there is nothing.
pub fn classification_context(messages: &[crate::ir::Message], max_chars: usize) -> Option<String> {
    let last_user = messages.iter().rposition(|m| m.role == "user")?;
    let text = messages[last_user].text().unwrap_or_default();
    let raw = match text.find("\n\nCurrent message: ") {
        Some(pos) if pos > 0 => text[..pos].to_string(),
        _ => {
            let prior: Vec<String> = messages[..last_user]
                .iter()
                .rev()
                .filter(|m| m.role != "system")
                .filter_map(|m| m.text())
                .filter(|t| !t.trim().is_empty())
                .take(2)
                .collect();
            prior.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
    };
    if raw.trim().is_empty() {
        return None;
    }
    let n = raw.chars().count();
    Some(if n > max_chars {
        raw.chars().skip(n - max_chars).collect()
    } else {
        raw
    })
}

/// Best-effort plain text of the latest `user` message — the classifier's `query` input.
/// Walks messages in reverse so the most recent user turn wins; `None` if there is none.
///
/// The A2A dispatch path (`a2a_dispatch.rs`) glues conversation history into a single
/// string via `SessionHistory::with_current_query`, producing messages shaped like:
///
/// ```text
/// user: hello
/// assistant: Hello! How can I help?
///
/// Current message: refactor this function
/// ```
///
/// When the agent forwards that blob as a single `user` message to the LLM router,
/// the salience gate and classifier would see the entire transcript instead of just the
/// current turn. Detect the `\n\nCurrent message: ` marker and extract only the tail.
pub fn latest_user_query(messages: &[crate::ir::Message]) -> Option<String> {
    let text = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.text())?;

    // Strip history prefix injected by `SessionHistory::with_current_query`.
    if let Some(pos) = text.find("\n\nCurrent message: ") {
        let current = &text[pos + "\n\nCurrent message: ".len()..];
        if !current.is_empty() {
            return Some(current.to_string());
        }
    }
    Some(text)
}

/// The routing-only view of where a transcript stands, for boundary and `conv_id` derivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAnchor {
    /// Count of genuine prompts (user messages not immediately preceded by a tool result).
    pub ordinal: usize,
    /// Text of the latest genuine prompt (A2A history prefix stripped).
    pub anchor_text: Option<String>,
    /// The transcript is mid tool-loop: it ends with a tool result, or with a user message
    /// that merely accompanies one.
    pub mid_tool_loop: bool,
}

/// Locate the current prompt for boundary purposes.
///
/// Anthropic clients send a tool result and any accompanying text in one user turn (e.g. a
/// coding CLI's `<system-reminder>` block next to its `tool_result`), which the inbound
/// parser normalizes to `[…, tool, user]`. Counted naively, that trailing `user` message
/// looks like a new prompt: the turn ordinal increments, the coding-agent `conv_id` changes
/// and the tool loop reclassifies mid-flight. Here a `user` message **immediately preceded
/// by a `tool` message** is treated as part of the tool loop, not a new prompt. For
/// transcripts without such interjections this agrees exactly with [`user_turn_ordinal`],
/// [`latest_user_query`] and [`is_tool_continuation`] — which stay unchanged, because brevity
/// and compression rely on their literal meaning.
pub fn turn_anchor(messages: &[crate::ir::Message]) -> TurnAnchor {
    let is_interjection =
        |i: usize| messages[i].role == "user" && i > 0 && messages[i - 1].role == "tool";
    let genuine: Vec<usize> = (0..messages.len())
        .filter(|&i| messages[i].role == "user" && !is_interjection(i))
        .collect();
    let anchor_text = genuine
        .last()
        .and_then(|&i| latest_user_query(&messages[..=i]));
    let mid_tool_loop = match messages.len() {
        0 => false,
        n => messages[n - 1].role == "tool" || is_interjection(n - 1),
    };
    TurnAnchor {
        ordinal: genuine.len(),
        anchor_text,
        mid_tool_loop,
    }
}

/// Number of top-level user turns so far (count of `role == "user"` messages). Tool results
/// normalize to `role == "tool"` (see `inbound::anthropic`'s doc comment on `tool_result` →
/// `{role:"tool"}`), so this counts only genuine new prompts, not tool-loop continuations.
/// Combined with [`latest_user_query`], this anchors a coding-agent's `conv_id`
/// ([`BoundarySignals::for_coding_agent`]) to *this* prompt — stable across the tool loop it
/// starts, but distinct from the prompt before and after it.
pub fn user_turn_ordinal(messages: &[crate::ir::Message]) -> usize {
    messages.iter().filter(|m| m.role == "user").count()
}

/// Whether the transcript's last turn is a tool result — a coding-agent CLI mid tool-loop,
/// which [`BoundarySignals::for_coding_agent`] must keep sticky (`Phase::Continue`).
pub fn is_tool_continuation(messages: &[crate::ir::Message]) -> bool {
    messages.last().is_some_and(|m| m.role == "tool")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Message;
    use crate::routing::registry::test_support;
    use async_trait::async_trait;
    use serde_json::{Map, Value};
    use std::sync::Mutex;

    /// A cache seeded with one hit and recording every `put`, to prove read/write levels.
    struct FakeCache {
        hit: Option<CachedDecision>,
        puts: Mutex<Vec<(String, String, String)>>,
    }
    impl FakeCache {
        fn empty() -> Self {
            Self {
                hit: None,
                puts: Mutex::new(vec![]),
            }
        }
        fn with_hit(model: &str) -> Self {
            Self {
                hit: Some(CachedDecision {
                    model: model.into(),
                    tier: Some(Tier::Tier1),
                    request_type: Some(RequestType::CodeGeneration),
                    complexity: None,
                    confidence: None,
                }),
                puts: Mutex::new(vec![]),
            }
        }
    }
    #[async_trait]
    impl DecisionCache for FakeCache {
        async fn get(&self, _conv_id: &str, _agent_id: &str) -> Option<CachedDecision> {
            self.hit.clone()
        }
        async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision) {
            self.puts.lock().unwrap().push((
                conv_id.to_string(),
                agent_id.to_string(),
                decision.model.clone(),
            ));
        }
    }

    /// A gate that always judges the turn small talk — exercises the Level 2.5 branch.
    struct DenyGate;
    #[async_trait]
    impl SalienceGate for DenyGate {
        async fn is_substantive(&self, _query: &str) -> bool {
            false
        }
    }

    fn signals(conv_id: Option<&str>, phase: Phase, mode: Mode) -> BoundarySignals {
        BoundarySignals {
            conv_id: conv_id.map(str::to_string),
            phase,
            mode,
        }
    }

    fn inputs<'a>(
        provider: &'a str,
        signals: &'a BoundarySignals,
        pinned: Option<&'a str>,
    ) -> RouteInputs<'a> {
        RouteInputs {
            agent_id: "agent-1",
            provider,
            fallback_model: "cfg-model",
            has_llm_config: true,
            pinned_model: pinned,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            signals,
            query: Some("hello"),
            context: None,
            min_confidence: 0.0,
            tier_seed: None,
            complexity_routing: ComplexityRouting::OFF,
        }
    }

    #[tokio::test]
    async fn level1_pinned_bypasses_everything() {
        // Even at a fireable boundary with a cache hit available, pinning wins and never
        // writes the cache.
        let cache = FakeCache::with_hit("cached");
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, Some("pinned-model")),
        )
        .await;
        assert_eq!(d.source, RouteSource::Pinned);
        assert_eq!(d.model, "pinned-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn level2_cache_hit_short_circuits_before_classify() {
        let cache = FakeCache::with_hit("cached-model");
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::CacheHit);
        assert_eq!(d.model, "cached-model");
    }

    #[tokio::test]
    async fn level3_classifies_at_boundary_and_writes_cache() {
        // The classifier now Thompson-samples a tier, so the exact tier is stochastic — but
        // it must resolve to one of anthropic's seeded models and write that decision through
        // to the cache exactly once.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Classified);
        assert!(d.tier.is_some());
        let expected_models = ["claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5"];
        assert!(
            expected_models.contains(&d.model.as_str()),
            "unexpected model: {}",
            d.model
        );
        let puts = cache.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].0, "c1");
        assert_eq!(puts[0].1, "agent-1");
        assert_eq!(puts[0].2, d.model);
    }

    #[tokio::test]
    async fn level2_5_small_talk_serves_cheapest_config_model_and_does_not_cache() {
        // Gate says non-substantive: a configured agent gets its cheapest model (registry
        // Tier3 for anthropic = claude-haiku-4-5), tagged SmallTalk, and the cache is never
        // written — so the session is not pinned on small talk.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &DenyGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::SmallTalk);
        assert_eq!(d.tier, None);
        assert_eq!(d.model, "claude-haiku-4-5");
        assert!(
            cache.puts.lock().unwrap().is_empty(),
            "small talk must not pin"
        );
    }

    #[tokio::test]
    async fn no_llm_config_bypasses_the_gate_entirely() {
        // An agent without llm_config never enters the routing block (has_llm_config guards
        // Levels 2–3), so the salience gate never runs even when it would deny: the resolver's
        // passthrough/default model is served (Level 5) and nothing is pinned.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.has_llm_config = false;
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &DenyGate,
            &RegexClassifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::Default);
        assert_eq!(d.model, "cfg-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn level2_5_small_talk_prefers_config_tier3_override() {
        // A per-config tier3 override is the cheapest model and wins over the registry.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.tier3_model = Some("claude-cheapo");
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &DenyGate,
            &RegexClassifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::SmallTalk);
        assert_eq!(d.model, "claude-cheapo");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deny_gate_still_serves_cache_hit_before_reaching_gate() {
        // Level 2 short-circuits before Level 2.5: a cache hit is returned even when the
        // gate would deny — a pinned session is never re-gated.
        let cache = FakeCache::with_hit("cached-model");
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &DenyGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::CacheHit);
        assert_eq!(d.model, "cached-model");
    }

    #[tokio::test]
    async fn cache_hit_with_positive_feedback_learns_and_stays_sticky() {
        // A continuation turn whose message approves the prior answer: the router returns the
        // sticky cached model (Level 2) AND folds a positive reward into the cached decision's
        // (tier, request_type) cell for this provider.
        let cache = FakeCache::with_hit("claude-opus-4-8"); // hit tier=Tier1, rt=CodeGeneration
        let cells = InMemoryCellStore::new();
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.query = Some("perfect, that worked. thanks!");
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &cells,
            &AllowAllGate,
            &RegexClassifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::CacheHit);
        assert_eq!(d.model, "claude-opus-4-8");
        let learned = cells.load("anthropic").await;
        let cell = learned
            .get(&(Tier::Tier1, RequestType::CodeGeneration))
            .expect("positive feedback should have created a learned cell");
        assert_eq!(cell.quality_mean, 1.0);
        assert_eq!(cell.samples, 1);
    }

    #[tokio::test]
    async fn cache_hit_without_signal_does_not_learn() {
        // A neutral continuation turn (a plain follow-up question) carries no verdict ⇒ no
        // cell is written, but the sticky model is still served.
        let cache = FakeCache::with_hit("claude-opus-4-8");
        let cells = InMemoryCellStore::new();
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.query = Some("now also handle the empty-input case");
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &cells,
            &AllowAllGate,
            &RegexClassifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::CacheHit);
        assert!(cells.load("anthropic").await.is_empty());
    }

    #[tokio::test]
    async fn level3_registry_miss_falls_through_to_config() {
        // gemini has no seeded tiers ⇒ classification can't resolve a model ⇒ Level 4.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("gemini", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(d.model, "cfg-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn continue_turn_does_not_classify_and_uses_config() {
        // A tool-loop turn (phase=continue) with a cache miss falls to config, never classifies.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(d.model, "cfg-model");
    }

    #[tokio::test]
    async fn pinned_flow_at_switch_does_not_classify() {
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::PinnedFlow);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
    }

    #[tokio::test]
    async fn no_conv_id_skips_cache_and_classify_landing_on_config() {
        // No conversation ⇒ Levels 2 & 3 are skipped entirely (the backward-compat
        // guarantee), even at a fireable "switch" boundary. Serve the configured model
        // and never read or write the cache.
        let cache = FakeCache::with_hit("should-not-be-read");
        let s = signals(None, Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(d.model, "cfg-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn falls_to_default_when_no_llm_config() {
        let cache = FakeCache::empty();
        let s = signals(None, Phase::Continue, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.has_llm_config = false;
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &RegexClassifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::Default);
        assert_eq!(d.model, "cfg-model");
    }

    #[test]
    fn latest_user_query_picks_last_user_message() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: Some(Value::String(content.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        let messages = vec![
            msg("system", "sys"),
            msg("user", "first"),
            msg("assistant", "reply"),
            msg("user", "second"),
        ];
        assert_eq!(latest_user_query(&messages).as_deref(), Some("second"));
        assert_eq!(latest_user_query(&[msg("system", "only")]), None);
    }

    #[test]
    fn latest_user_query_strips_packed_history() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: Some(Value::String(content.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        // The A2A dispatch packs history via `SessionHistory::with_current_query`:
        //   "user: hello\nassistant: Hi!\n\nCurrent message: refactor this"
        let packed = "user: hello\nassistant: Hi there!\n\nCurrent message: refactor this function";
        let messages = vec![msg("user", packed)];
        assert_eq!(
            latest_user_query(&messages).as_deref(),
            Some("refactor this function")
        );
    }

    #[test]
    fn latest_user_query_returns_full_text_without_marker() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: Some(Value::String(content.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        // A normal message without history packing is returned as-is.
        let messages = vec![msg("user", "just a plain query")];
        assert_eq!(
            latest_user_query(&messages).as_deref(),
            Some("just a plain query")
        );
    }

    #[test]
    fn user_turn_ordinal_counts_user_messages_not_tool_results() {
        let msg = |role: &str| Message {
            role: role.into(),
            content: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        assert_eq!(user_turn_ordinal(&[msg("system"), msg("user")]), 1);
        // A tool loop after the first prompt doesn't add to the count — it's still turn 1.
        assert_eq!(
            user_turn_ordinal(&[msg("user"), msg("assistant"), msg("tool")]),
            1
        );
        // A second genuine prompt bumps the ordinal.
        assert_eq!(
            user_turn_ordinal(&[msg("user"), msg("assistant"), msg("tool"), msg("user")]),
            2
        );
    }

    #[test]
    fn is_tool_continuation_detects_a_trailing_tool_result() {
        let msg = |role: &str| Message {
            role: role.into(),
            content: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        assert!(is_tool_continuation(&[
            msg("user"),
            msg("assistant"),
            msg("tool")
        ]));
        assert!(!is_tool_continuation(&[msg("user"), msg("assistant")]));
        assert!(!is_tool_continuation(&[]));
    }

    // --- Level 3 classifier seam ---

    /// A classifier that returns a fixed verdict (or an error) and counts its calls.
    struct CountingClassifier {
        calls: std::sync::atomic::AtomicUsize,
        verdict: Option<Classification>,
    }
    impl CountingClassifier {
        fn returning(c: Classification) -> Self {
            Self {
                calls: Default::default(),
                verdict: Some(c),
            }
        }
        fn failing() -> Self {
            Self {
                calls: Default::default(),
                verdict: None,
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }
    #[async_trait]
    impl RequestClassifier for CountingClassifier {
        fn name(&self) -> &str {
            "counting"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.verdict
                .ok_or_else(|| ClassifyError::Unavailable("down".into()))
        }
    }

    /// A cache recording the full decisions it is asked to store.
    #[derive(Default)]
    struct RecordingCache {
        puts: Mutex<Vec<CachedDecision>>,
    }
    #[async_trait]
    impl DecisionCache for RecordingCache {
        async fn get(&self, _: &str, _: &str) -> Option<CachedDecision> {
            None
        }
        async fn put(&self, _: &str, _: &str, decision: &CachedDecision) {
            self.puts.lock().unwrap().push(decision.clone());
        }
    }

    fn writing_verdict(confidence: f32) -> Classification {
        Classification::new(RequestType::Writing, 4, confidence)
    }

    #[tokio::test]
    async fn level3_uses_injected_classifier_type_and_caches_complexity() {
        let cache = RecordingCache::default();
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.query = Some("write a python function that parses CSV");
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::Classified);
        assert_eq!(d.classification, Some(writing_verdict(0.9)));
        let puts = cache.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].request_type, Some(RequestType::Writing));
        assert_eq!(puts[0].complexity, Some(4));
        assert_eq!(puts[0].confidence, Some(0.9));
        assert_eq!(classifier.calls(), 1);
    }

    #[tokio::test]
    async fn failing_classifier_still_routes_via_regex_fallback() {
        let cache = RecordingCache::default();
        let classifier = CountingClassifier::failing();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.query = Some("draft an email to my team about the outage");
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::Classified);
        assert_eq!(
            d.classification,
            Some(RegexClassifier::classify_sync(
                "draft an email to my team about the outage"
            ))
        );
        assert_eq!(
            cache.puts.lock().unwrap()[0].request_type,
            Some(RequestType::Writing)
        );
    }

    #[tokio::test]
    async fn low_confidence_serves_configured_model_and_does_not_pin() {
        let cache = RecordingCache::default();
        let classifier = CountingClassifier::returning(writing_verdict(0.2));
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.min_confidence = 0.5;
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &i,
        )
        .await;
        assert_eq!(d.source, RouteSource::LowConfidence);
        assert_eq!(d.model, "cfg-model");
        assert_eq!(d.tier, None);
        assert!(
            cache.puts.lock().unwrap().is_empty(),
            "abstain must not pin"
        );
    }

    #[tokio::test]
    async fn continue_turn_never_invokes_classifier() {
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(classifier.calls(), 0);
    }

    #[tokio::test]
    async fn cache_hit_pinned_no_conv_id_and_small_talk_never_invoke_classifier() {
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        let boundary = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let no_conv = signals(None, Phase::Switch, Mode::FreeFlowing);
        // cache hit
        let d = route_model(
            &FakeCache::with_hit("cached"),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &boundary, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::CacheHit);
        // pinned
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &boundary, Some("pinned")),
        )
        .await;
        assert_eq!(d.source, RouteSource::Pinned);
        // no conv_id
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &no_conv, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        // small talk
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &DenyGate,
            &classifier,
            &inputs("anthropic", &boundary, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::SmallTalk);
        assert_eq!(classifier.calls(), 0);
    }

    #[tokio::test]
    async fn cold_start_invokes_classifier_exactly_once() {
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        let s = signals(Some("c1"), Phase::ColdStart, Mode::FreeFlowing);
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Classified);
        assert_eq!(classifier.calls(), 1);
    }

    #[tokio::test]
    async fn seeded_tier_selection_is_reproducible_and_conv_ids_explore() {
        let mut first = None;
        for _ in 0..1000 {
            let s = signals(Some("conv-fixed"), Phase::Switch, Mode::FreeFlowing);
            let mut i = inputs("anthropic", &s, None);
            i.tier_seed = Some(7);
            let d = route_model(
                &FakeCache::empty(),
                &test_support::StubRegistry,
                &InMemoryCellStore::new(),
                &AllowAllGate,
                &RegexClassifier,
                &i,
            )
            .await;
            assert_eq!(
                *first.get_or_insert(d.tier),
                d.tier,
                "seeded tier must not vary"
            );
        }
        let mut tiers = std::collections::HashSet::new();
        for n in 0..200 {
            let conv = format!("conv-{n}");
            let s = signals(Some(&conv), Phase::Switch, Mode::FreeFlowing);
            let mut i = inputs("anthropic", &s, None);
            i.tier_seed = Some(7);
            let d = route_model(
                &FakeCache::empty(),
                &test_support::StubRegistry,
                &InMemoryCellStore::new(),
                &AllowAllGate,
                &RegexClassifier,
                &i,
            )
            .await;
            tiers.insert(d.tier);
        }
        assert!(
            tiers.len() > 1,
            "different conversations should still explore"
        );
    }

    #[tokio::test]
    async fn in_flow_tool_loop_turn_never_invokes_classifier() {
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        let s = BoundarySignals::in_flow_turn("ses-1".into(), Mode::FreeFlowing, true);
        let d = route_model(
            &FakeCache::empty(),
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &classifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(classifier.calls(), 0);
    }

    /// One conversation through a real decision cache: `cold_start` classifies and pins,
    /// `continue` turns reuse that model and tier without classifying again, and a `switch`
    /// to another agent classifies once more without disturbing the first agent's pin.
    #[tokio::test]
    async fn conversation_lifecycle_classifies_only_at_boundaries_and_keeps_tier_sticky() {
        let cache = cache::InMemoryDecisionCache::new(16, std::time::Duration::from_secs(60));
        let cells = InMemoryCellStore::new();
        let classifier = CountingClassifier::returning(writing_verdict(0.9));
        fn route<'a>(
            signals: &'a BoundarySignals,
            agent_id: &'a str,
            query: &'a str,
        ) -> RouteInputs<'a> {
            let mut i = inputs("anthropic", signals, None);
            i.agent_id = agent_id;
            i.query = Some(query);
            i.tier_seed = Some(7);
            i
        }

        let cold = signals(Some("conv-1"), Phase::ColdStart, Mode::FreeFlowing);
        let first = route_model(
            &cache,
            &test_support::StubRegistry,
            &cells,
            &AllowAllGate,
            &classifier,
            &route(&cold, "agent-1", "draft the release notes for v2"),
        )
        .await;
        assert_eq!(first.source, RouteSource::Classified);
        assert!(first.tier.is_some());
        assert_eq!(classifier.calls(), 1);

        let cont = signals(Some("conv-1"), Phase::Continue, Mode::FreeFlowing);
        for query in ["now design a sharded queue", "thanks, shorten it"] {
            let d = route_model(
                &cache,
                &test_support::StubRegistry,
                &cells,
                &AllowAllGate,
                &classifier,
                &route(&cont, "agent-1", query),
            )
            .await;
            assert_eq!(d.source, RouteSource::CacheHit);
            assert_eq!(
                (d.model.as_str(), d.tier),
                (first.model.as_str(), first.tier)
            );
        }
        assert_eq!(classifier.calls(), 1, "continue must not reclassify");

        let switch = signals(Some("conv-1"), Phase::Switch, Mode::FreeFlowing);
        let other = route_model(
            &cache,
            &test_support::StubRegistry,
            &cells,
            &AllowAllGate,
            &classifier,
            &route(&switch, "agent-2", "review this design doc"),
        )
        .await;
        assert_eq!(other.source, RouteSource::Classified);
        assert_eq!(
            classifier.calls(),
            2,
            "switch to a new agent classifies once"
        );

        let back = route_model(
            &cache,
            &test_support::StubRegistry,
            &cells,
            &AllowAllGate,
            &classifier,
            &route(&cont, "agent-1", "one more tweak"),
        )
        .await;
        assert_eq!(back.source, RouteSource::CacheHit);
        assert_eq!((back.model, back.tier), (first.model, first.tier));
        assert_eq!(classifier.calls(), 2);
    }

    #[test]
    fn turn_anchor_treats_text_after_tool_result_as_part_of_the_loop() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: Some(Value::String(content.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        let plain = vec![msg("user", "a"), msg("assistant", "b"), msg("user", "c")];
        assert_eq!(
            turn_anchor(&plain),
            TurnAnchor {
                ordinal: user_turn_ordinal(&plain),
                anchor_text: latest_user_query(&plain),
                mid_tool_loop: is_tool_continuation(&plain),
            }
        );
        let looped = vec![
            msg("user", "refactor"),
            msg("assistant", "calling tool"),
            msg("tool", "result"),
            msg("user", "<system-reminder>x</system-reminder>"),
        ];
        let a = turn_anchor(&looped);
        assert_eq!(a.ordinal, 1);
        assert_eq!(a.anchor_text.as_deref(), Some("refactor"));
        assert!(a.mid_tool_loop);
        assert!(!turn_anchor(&[]).mid_tool_loop);
    }

    #[test]
    fn classification_context_prefers_packed_history_then_recent_turns_and_caps_tail() {
        let msg = |role: &str, content: &str| Message {
            role: role.into(),
            content: Some(Value::String(content.into())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        };
        let packed = vec![msg(
            "user",
            "user: hi\nassistant: hello\n\nCurrent message: refactor this",
        )];
        assert_eq!(
            classification_context(&packed, 2000).as_deref(),
            Some("user: hi\nassistant: hello")
        );
        let turns = vec![
            msg("system", "you are helpful"),
            msg("user", "first"),
            msg("assistant", "answer one"),
            msg("user", "second"),
            msg("assistant", "answer two"),
            msg("user", "latest"),
        ];
        assert_eq!(
            classification_context(&turns, 2000).as_deref(),
            Some("second\nanswer two")
        );
        assert_eq!(classification_context(&turns, 3).as_deref(), Some("two"));
        assert_eq!(classification_context(&[msg("user", "only")], 2000), None);
        assert_eq!(classification_context(&[msg("system", "s")], 2000), None);
    }
}
