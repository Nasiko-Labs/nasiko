//! Model routing — decides *which model* to call, within the destination provider the
//! resolver already fixed.
//!
//! The resolver ([`crate::resolver`]) still owns provider, credentials, and params; this
//! layer only overrides the **model**, and only at conversation/agent boundaries where
//! switching is safe. Everywhere else the model stays sticky (so tool-call state can't
//! drift). The decision follows a fixed five-level precedence — see [`route_model`].
//!
//! ```text
//! query + context ──► RequestClassifier ──► (type, complexity) ──► pick_tier() ──► Tier
//!                 ──► registry::model_for(provider, Tier) ──► model
//! ```
//!
//! A [`RequestClassifier`] (regex by default, or the [Laya](laya) sidecar) buckets the query
//! into a request type and estimates its complexity; [`classifier::pick_tier`]
//! Thompson-samples a [`Tier`] over the provider's learned quality [cells](cells), with a
//! confident complexity estimate nudging the cold-start priors toward Tier1 or Tier3; feedback
//! from the user's next turn ([`classifier::signal`]) is folded back into those cells, so the
//! router learns which tier suffices for which kind of query. See [`route_model`].

pub mod attribution;
pub mod boundary;
pub mod cache;
pub mod catalog;
pub mod cells;
pub mod classifier;
// The salience classifier itself — feature engine, weight loading, scoring, banding.
// Private to `routing`: only `salience.rs` (a sibling module) uses it directly, via
// `ClassifierSalienceGate`.
pub mod hybrid;
pub mod laya;
pub mod local_classifier;
mod patterns;
pub mod pricing_sync;
pub mod registry;
pub mod request_classifier;
pub mod salience;
mod salience_classifier;
pub mod strands;

pub use boundary::{BoundarySignals, Mode, Phase};
pub use cache::{CachedDecision, DecisionCache, NoopCache, RedisCache};
pub use cells::{CellStore, InMemoryCellStore, PgCellStore};
pub use classifier::{RequestType, Tier, classify, classify_request_type, signal};
pub use registry::{PgTierRegistry, TierRegistry};
pub use request_classifier::{
    Classification, ClassifierInput, ClassifierSource, RegexClassifier, RequestClassifier,
};
pub use salience::{AllowAllGate, ClassifierSalienceGate, SalienceGate};

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
    /// Level 3 — a model-backed classifier ran but was below
    /// [`request_classifier::LOW_CONFIDENCE_FLOOR`]: the configured/default model is served
    /// and the decision cache is **not** written, so the next boundary turn — with more
    /// context — classifies again instead of the session being pinned to a guess.
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
    /// The query plus recent context ([`request_classifier::classify_input`]) for
    /// model-backed classifiers; `None` ⇒ they read the query alone.
    pub classifier_state: Option<&'a str>,
    /// Optional reproducible tier-sampling seed, read by the binary configuration.
    /// `None` preserves existing exploration. It never overrides a cached decision.
    pub routing_seed: Option<u64>,
}

/// The outcome of routing: the model to call and how it was chosen.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub model: String,
    pub tier: Option<Tier>,
    pub source: RouteSource,
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
///    with a query present and a registry entry for `(provider, tier)`. Loads the provider's
///    learned cells, Thompson-samples a tier, and writes the decision (incl. request type)
///    through to the cache so the next turn short-circuits at Level 2.
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
            // Classify (request type + complexity), load the provider's learned quality, then
            // Thompson-sample a tier with the complexity-shifted priors. The optional seed
            // reproduces the same draw for the same classification and learned cells;
            // otherwise the existing exploration RNG is used. The RNG (`ThreadRng`)
            // is `!Send`, so it is scoped to drop before the next `.await` — the handler
            // future must stay `Send`.
            let started = std::time::Instant::now();
            let classification = classifier
                .classify(&ClassifierInput {
                    query,
                    state: inputs.classifier_state.unwrap_or(query),
                })
                .await;
            tracing::info!(
                target: "nasiko::llm_router::classifier",
                agent_id = %inputs.agent_id, %conv_id, provider = %inputs.provider,
                source = classification.source.as_str(),
                fallback_reason = classification.source.fallback_reason(),
                request_type = classification.request_type.as_str(),
                type_probabilities = ?classification.type_probabilities,
                complexity = classification.complexity,
                complexity_level = classification.complexity_level,
                complexity_confidence = classification.complexity_confidence,
                confidence = classification.confidence,
                prior_shift = classifier::prior_shift(&classification),
                latency_ms = started.elapsed().as_millis() as u64,
                query_preview = %query_preview(query),
                "classifier: classified request"
            );
            if classification.is_low_confidence() {
                tracing::info!(
                    target: "nasiko::llm_router::routing",
                    agent_id = %inputs.agent_id, %conv_id,
                    level = 3,
                    source = ?RouteSource::LowConfidence,
                    confidence = classification.confidence,
                    model = %inputs.fallback_model,
                    "route_model: LEVEL 3 (LowConfidence) — classifier unsure; serving the configured/default model without pinning"
                );
                return RouteDecision {
                    model: inputs.fallback_model.to_string(),
                    tier: None,
                    source: RouteSource::LowConfidence,
                };
            }
            let learned = cell_store.load(inputs.provider).await;
            // `request_type` is the type the tier was sampled under (drawn from the
            // classifier's distribution) — the one feedback must be credited to.
            let (tier, request_type) = match inputs.routing_seed {
                Some(seed) => {
                    use rand::SeedableRng;
                    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                    classifier::pick_tier(&classification, &learned, &mut rng)
                }
                None => {
                    let mut rng = rand::rng();
                    classifier::pick_tier(&classification, &learned, &mut rng)
                }
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
            classifier_state: None,
            routing_seed: None,
        }
    }

    #[tokio::test]
    async fn seeded_boundary_routing_repeats_with_noop_or_fresh_cache() {
        use rand::SeedableRng;

        struct FixedClassifier(Classification);
        #[async_trait]
        impl RequestClassifier for FixedClassifier {
            async fn classify(&self, _: &ClassifierInput<'_>) -> Classification {
                self.0.clone()
            }
        }
        let classification = Classification {
            request_type: RequestType::Writing,
            type_probabilities: Some(vec![
                (RequestType::Writing, 0.65),
                (RequestType::General, 0.35),
            ]),
            complexity: 3,
            complexity_level: 2.0,
            complexity_confidence: 0.8,
            confidence: 0.65,
            source: ClassifierSource::Local,
        };
        let classifier = FixedClassifier(classification.clone());
        let store = InMemoryCellStore::new();
        store
            .observe("anthropic", Tier::Tier2, RequestType::Writing, 0.9)
            .await;
        let learned = store.load("anthropic").await;
        assert!(!learned.is_empty(), "exercise the learned-state input too");
        let s = signals(Some("c1"), Phase::ColdStart, Mode::FreeFlowing);
        let mut route_inputs = inputs("anthropic", &s, None);
        for seed in [0, 7, 42, 101] {
            route_inputs.routing_seed = Some(seed);
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let (expected_tier, _) = classifier::pick_tier(&classification, &learned, &mut rng);
            let expected_model = test_support::StubRegistry
                .model_for("anthropic", expected_tier)
                .await
                .unwrap();
            for _ in 0..8 {
                let decision = route_model(
                    &NoopCache,
                    &test_support::StubRegistry,
                    &store,
                    &AllowAllGate,
                    &classifier,
                    &route_inputs,
                )
                .await;
                assert_eq!(decision.source, RouteSource::Classified);
                assert_eq!(decision.tier, Some(expected_tier));
                assert_eq!(decision.model, expected_model);
            }
            let cache = FakeCache::empty();
            let decision = route_model(
                &cache,
                &test_support::StubRegistry,
                &store,
                &AllowAllGate,
                &classifier,
                &route_inputs,
            )
            .await;
            assert_eq!(decision.tier, Some(expected_tier));
            assert_eq!(decision.model, expected_model);
            assert_eq!(cache.puts.lock().unwrap().len(), 1);
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

    /// Counts calls and records the state it was given — proves *when* Level 3 classifies.
    struct CountingClassifier {
        calls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl RequestClassifier for CountingClassifier {
        async fn classify(&self, input: &ClassifierInput<'_>) -> Classification {
            self.calls.lock().unwrap().push(input.state.to_string());
            RegexClassifier::classify_query(input.query)
        }
    }

    #[tokio::test]
    async fn classifier_runs_only_on_a_fireable_cache_miss_with_the_context_state() {
        let counting = CountingClassifier {
            calls: Mutex::new(vec![]),
        };
        let route = |cache: FakeCache, phase: Phase| {
            let counting = &counting;
            async move {
                let s = signals(Some("c1"), phase, Mode::FreeFlowing);
                let mut i = inputs("anthropic", &s, None);
                i.classifier_state = Some("Latest request:\nhello");
                route_model(
                    &cache,
                    &test_support::StubRegistry,
                    &InMemoryCellStore::new(),
                    &AllowAllGate,
                    counting,
                    &i,
                )
                .await
            }
        };
        route(FakeCache::empty(), Phase::Continue).await;
        route(FakeCache::with_hit("cached"), Phase::Switch).await;
        assert!(
            counting.calls.lock().unwrap().is_empty(),
            "a continue turn or a cache hit must not classify"
        );
        let d = route(FakeCache::empty(), Phase::Switch).await;
        assert_eq!(d.source, RouteSource::Classified);
        assert_eq!(
            *counting.calls.lock().unwrap(),
            vec!["Latest request:\nhello".to_string()]
        );
    }

    /// A model-backed classifier that is unsure between two types.
    struct UnsureClassifier;
    #[async_trait]
    impl RequestClassifier for UnsureClassifier {
        async fn classify(&self, _input: &ClassifierInput<'_>) -> Classification {
            Classification {
                request_type: RequestType::Writing,
                type_probabilities: Some(vec![
                    (RequestType::Writing, 0.35),
                    (RequestType::General, 0.33),
                    (RequestType::FactualLookup, 0.32),
                ]),
                complexity: 3,
                complexity_level: 2.0,
                complexity_confidence: 0.1,
                confidence: 0.35,
                source: ClassifierSource::Local,
            }
        }
    }

    #[tokio::test]
    async fn low_confidence_serves_the_configured_model_without_pinning() {
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &UnsureClassifier,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::LowConfidence);
        assert_eq!((d.model.as_str(), d.tier), ("cfg-model", None));
        assert!(
            cache.puts.lock().unwrap().is_empty(),
            "an unsure classification must not pin the session"
        );
    }

    #[tokio::test]
    async fn strands_probability_above_floor_still_preserves_safe_admission() {
        struct BroadStrands;
        #[async_trait]
        impl RequestClassifier for BroadStrands {
            async fn classify(&self, _: &ClassifierInput<'_>) -> Classification {
                let mut c = RegexClassifier::classify_query("hello");
                c.request_type = RequestType::Writing;
                c.confidence = 0.45;
                c.type_probabilities = Some(vec![
                    (RequestType::Writing, 0.45),
                    (RequestType::General, 0.30),
                    (RequestType::FactualLookup, 0.25),
                ]);
                c.source = ClassifierSource::Strands;
                c
            }
        }
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::ColdStart, Mode::FreeFlowing);
        let d = route_model(
            &cache,
            &test_support::StubRegistry,
            &InMemoryCellStore::new(),
            &AllowAllGate,
            &BroadStrands,
            &inputs("anthropic", &s, None),
        )
        .await;
        assert_eq!(d.source, RouteSource::LowConfidence);
        assert_eq!((d.model.as_str(), d.tier), ("cfg-model", None));
        assert!(cache.puts.lock().unwrap().is_empty());
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
}
