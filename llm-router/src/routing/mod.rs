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

pub mod boundary;
pub mod cache;
pub mod cells;
pub mod classifier;
mod patterns;
pub mod registry;

pub use boundary::{BoundarySignals, Mode, Phase};
pub use cache::{CachedDecision, DecisionCache, NoopCache, RedisCache};
pub use cells::{CellStore, InMemoryCellStore, PgCellStore};
pub use classifier::{RequestType, Tier, classify, signal};
pub use registry::{PgTierRegistry, StaticTierRegistry, TierRegistry};

/// Which precedence level produced a routing decision — emitted as a structured tag so we
/// can see, per request, how the model was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    /// Level 1 — agent config is pinned; the classifier never ran.
    Pinned,
    /// Level 2 — served from the decision cache (a continuation turn).
    CacheHit,
    /// Level 3 — the classifier ran at a safe boundary.
    Classified,
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
    /// Per-request boundary tags (phase/mode/conv_id).
    pub signals: &'a BoundarySignals,
    /// The query to classify (latest user message text). `None` disables classification.
    pub query: Option<&'a str>,
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

    // Levels 2 & 3 apply only within a conversation. No `conv_id` (a direct-SDK agent not
    // driven by the orchestrator) ⇒ the router never fires and behaviour is identical to
    // before this layer — straight to the configured/default model.
    if let Some(conv_id) = inputs.signals.conv_id.as_deref() {
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

        // Level 3 — classify, but only at a boundary where re-selecting is safe.
        if inputs.signals.is_fireable_boundary()
            && let Some(query) = inputs.query
        {
            tracing::info!(
                target: "nasiko::llm_router::routing",
                agent_id = %inputs.agent_id, %conv_id, provider = %inputs.provider,
                "route_model: LEVEL 3 (Classified) — at fireable boundary with a query; invoking classifier"
            );
            // Load the provider's learned quality, then Thompson-sample a tier. Production
            // uses an entropy RNG (exploration drives learning); tests seed it. The RNG
            // (`ThreadRng`) is `!Send`, so it is scoped to drop before the next `.await` — the
            // handler future must stay `Send`.
            let learned = cell_store.load(inputs.provider).await;
            let (tier, request_type) = {
                let mut rng = rand::rng();
                classify(query, inputs.provider, &learned, &mut rng)
            };
            match registry.model_for(inputs.provider, tier).await {
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

/// Best-effort plain text of the latest `user` message — the classifier's `query` input.
/// Walks messages in reverse so the most recent user turn wins; `None` if there is none.
pub fn latest_user_query(messages: &[crate::ir::Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.text())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Message;
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
            signals,
            query: Some("hello"),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
    async fn cache_hit_with_positive_feedback_learns_and_stays_sticky() {
        // A continuation turn whose message approves the prior answer: the router returns the
        // sticky cached model (Level 2) AND folds a positive reward into the cached decision's
        // (tier, request_type) cell for this provider.
        let cache = FakeCache::with_hit("claude-opus-4-8"); // hit tier=Tier1, rt=CodeGeneration
        let cells = InMemoryCellStore::new();
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let mut i = inputs("anthropic", &s, None);
        i.query = Some("perfect, that worked. thanks!");
        let d = route_model(&cache, &StaticTierRegistry, &cells, &i).await;
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
        let d = route_model(&cache, &StaticTierRegistry, &cells, &i).await;
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
            &StaticTierRegistry,
            &InMemoryCellStore::new(),
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
        let d = route_model(&cache, &StaticTierRegistry, &InMemoryCellStore::new(), &i).await;
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
}
