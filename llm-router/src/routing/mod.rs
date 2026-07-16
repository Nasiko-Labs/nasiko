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
//! The [classifier](classifier::classify) body is a placeholder for now — this module is
//! the machinery around the agreed signature (S1), built and tested end-to-end before the
//! real classification logic lands.

pub mod boundary;
pub mod cache;
pub mod classifier;
pub mod registry;

pub use boundary::{BoundarySignals, Mode, Phase};
pub use cache::{CachedDecision, DecisionCache, NoopCache, RedisCache};
pub use classifier::{Tier, classify};
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
/// 3. **Classify** — only at a fireable boundary (`switch`/`cold_start` + `free_flowing`)
///    with a query present and a registry entry for `(provider, tier)`. Write-through to
///    the cache so the next turn short-circuits at Level 2.
/// 4. **Config** — the agent's configured model (`has_llm_config`).
/// 5. **Default** — no `llm_config`: the resolver's `fallback_model`, which is the request's
///    own provider/model (passthrough) or the platform default as the last-resort safety net.
pub async fn route_model(
    cache: &dyn DecisionCache,
    registry: &dyn TierRegistry,
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
            let tier = classify(query, inputs.provider);
            match registry.model_for(inputs.provider, tier).await {
                Some(model) => {
                    let decision = CachedDecision {
                        model,
                        tier: Some(tier),
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
            Self { hit: None, puts: Mutex::new(vec![]) }
        }
        fn with_hit(model: &str) -> Self {
            Self {
                hit: Some(CachedDecision { model: model.into(), tier: Some(Tier::Tier1) }),
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
        BoundarySignals { conv_id: conv_id.map(str::to_string), phase, mode }
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
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, Some("pinned-model"))).await;
        assert_eq!(d.source, RouteSource::Pinned);
        assert_eq!(d.model, "pinned-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn level2_cache_hit_short_circuits_before_classify() {
        let cache = FakeCache::with_hit("cached-model");
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, None)).await;
        assert_eq!(d.source, RouteSource::CacheHit);
        assert_eq!(d.model, "cached-model");
    }

    #[tokio::test]
    async fn level3_classifies_at_boundary_and_writes_cache() {
        // Placeholder classifier returns Tier2 ⇒ anthropic Tier2 = claude-sonnet-4-6.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, None)).await;
        assert_eq!(d.source, RouteSource::Classified);
        assert_eq!(d.model, "claude-sonnet-4-6");
        assert_eq!(d.tier, Some(Tier::Tier2));
        let puts = cache.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0], ("c1".into(), "agent-1".into(), "claude-sonnet-4-6".into()));
    }

    #[tokio::test]
    async fn level3_registry_miss_falls_through_to_config() {
        // gemini has no seeded tiers ⇒ classification can't resolve a model ⇒ Level 4.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::FreeFlowing);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("gemini", &s, None)).await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(d.model, "cfg-model");
        assert!(cache.puts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn continue_turn_does_not_classify_and_uses_config() {
        // A tool-loop turn (phase=continue) with a cache miss falls to config, never classifies.
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Continue, Mode::FreeFlowing);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, None)).await;
        assert_eq!(d.source, RouteSource::Config);
        assert_eq!(d.model, "cfg-model");
    }

    #[tokio::test]
    async fn pinned_flow_at_switch_does_not_classify() {
        let cache = FakeCache::empty();
        let s = signals(Some("c1"), Phase::Switch, Mode::PinnedFlow);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, None)).await;
        assert_eq!(d.source, RouteSource::Config);
    }

    #[tokio::test]
    async fn no_conv_id_skips_cache_and_classify_landing_on_config() {
        // No conversation ⇒ Levels 2 & 3 are skipped entirely (the backward-compat
        // guarantee), even at a fireable "switch" boundary. Serve the configured model
        // and never read or write the cache.
        let cache = FakeCache::with_hit("should-not-be-read");
        let s = signals(None, Phase::Switch, Mode::FreeFlowing);
        let d = route_model(&cache, &StaticTierRegistry, &inputs("anthropic", &s, None)).await;
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
        let d = route_model(&cache, &StaticTierRegistry, &i).await;
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
