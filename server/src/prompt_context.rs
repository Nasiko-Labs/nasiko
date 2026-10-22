//! Generic hook for enterprise-only supplemental context added to an agent's prompt, keyed by
//! agent id. Deliberately generic-named — no enterprise or memory-specific naming — matching the
//! `FinopsUserScope` precedent (`oss/server/src/observability/handler.rs`): OSS defines the
//! shape and ships a no-op default, EE composition roots override it, same as `routing_engine` /
//! `resource_stats` (see `AppState`'s own doc comments for those).
//!
//! Unlike `FinopsUserScope` (a per-request `Extension` populated by prior middleware), this is
//! an `AppState` field: the candidate-agent list this needs to react to (`orchestrator_stream`'s
//! agent selection) isn't known until partway through the handler, so there's no point before
//! the handler runs where per-request middleware could precompute anything — the handler calls
//! this service inline once it has that data, the same way it already calls `state.auth` inline.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait PromptContextProvider: Send + Sync {
    /// Supplemental text for each of the given candidate agents, before any one of them has been
    /// chosen (e.g. the ReAct planner's "Available Agents" list, built before the LLM decides
    /// anything) — ranked against `query` in addition to any pinned content, exactly like
    /// `context_for_agent` does for a single already-chosen agent. Agents with nothing relevant
    /// to add are simply absent from the returned map.
    async fn context_for_agents(&self, agent_ids: &[Uuid], query: &str) -> HashMap<Uuid, String>;

    /// Supplemental text for one already-chosen agent, given the actual query text so
    /// query-relevant content can be ranked in addition to any pinned content. `None` when
    /// there's nothing to add.
    async fn context_for_agent(&self, agent_id: Uuid, query: &str) -> Option<String>;

    /// Text that goes to **every** agent invocation, whichever agent is being called.
    ///
    /// Takes no agent id and no query on purpose, and that is the whole distinction from the two
    /// methods above: this content is not selected, ranked, or tailored to anything — it applies
    /// unconditionally or it is not this kind of content. Two consequences callers depend on:
    /// it can be resolved **once per turn** and reused across every leg of a fan-out (nothing
    /// varies per leg to invalidate it), and it is placed **ahead of** per-agent context, which is
    /// how it outranks anything more specific that follows it.
    ///
    /// Defaulted rather than required, so an implementation that only has per-agent content — the
    /// only kind that existed before this method — stays correct without naming this concern at
    /// all.
    async fn org_context(&self) -> Option<String> {
        None
    }

    /// Supplemental text scoped to the **user**, not to any one candidate agent — the planner's
    /// own recall, present whether or not the turn ends up delegating to anyone at all. This is
    /// what lets a zero-leg answer ("what did I tell you last week?") be memory-aware: unlike
    /// per-agent context, which reaches only the agent(s) that get called, this reaches the
    /// planner's own preamble before any delegation decision is made, so it is the one seam a
    /// minted sub-leg's own outbound call (`nasiko-react-agent`, no path back to this trait)
    /// cannot substitute for.
    ///
    /// Defaulted rather than required, so an implementation with nothing user-scoped to add —
    /// which is every implementation until a per-user store exists — stays correct without
    /// naming this concern at all. `None` is a real, common answer, not just the OSS default:
    /// even an enterprise deployment returns it for a user with nothing recalled yet.
    async fn context_for_user(&self, _user_id: Uuid, _query: &str) -> Option<String> {
        None
    }
}

/// How a supplemental section is joined onto the text it qualifies.
///
/// Re-exported from `nasiko-react-agent` rather than defined here: the orchestrator's minted
/// sub-legs apply the same convention from inside that crate, which cannot depend on this one, so
/// the definition lives in the lower crate and every call site in the tree shares it. Imported
/// through this module anyway, because this is where server-side callers already look for
/// prompt-assembly concerns.
pub use nasiko_react_agent::prefixed;

/// OSS default — no enterprise context source is wired up, so every call is a no-op.
pub struct NoopPromptContextProvider;

#[async_trait]
impl PromptContextProvider for NoopPromptContextProvider {
    async fn context_for_agents(&self, _agent_ids: &[Uuid], _query: &str) -> HashMap<Uuid, String> {
        HashMap::new()
    }

    async fn context_for_agent(&self, _agent_id: Uuid, _query: &str) -> Option<String> {
        None
    }
}

/// Holds a `PromptContextProvider` behind a lock so the implementation can be swapped after
/// `AppState` has already been cloned — needed because the EE composition root's real
/// implementation is installed by `build_ee_app`, but some background tasks (e.g. the HITL resume
/// dispatcher, `oss/server/src/state.rs`) are spawned with an `AppState` clone taken earlier,
/// inside `AppState::from_config_with_db`, strictly before `build_ee_app` ever runs. Reassigning a
/// plain `Arc<dyn PromptContextProvider>` field only rebinds *that* later clone's own pointer — a
/// task already holding an earlier clone keeps seeing whatever was installed at construction time
/// for its entire lifetime (this is exactly how the HITL resume path silently kept using the OSS
/// no-op even once the EE implementation was correctly wired up elsewhere). Installing *through*
/// this shared cell instead makes the swap visible to every existing and future clone, since
/// `AppState::clone()` only clones the `Arc<SwappablePromptContext>` pointer — every clone keeps
/// pointing at this one instance.
pub struct SwappablePromptContext(RwLock<Arc<dyn PromptContextProvider>>);

impl SwappablePromptContext {
    pub fn new(initial: Arc<dyn PromptContextProvider>) -> Self {
        Self(RwLock::new(initial))
    }

    /// Replaces the underlying implementation for every clone, past and future, of the `AppState`
    /// this cell lives in.
    pub fn install(&self, new: Arc<dyn PromptContextProvider>) {
        *self.0.write().expect("prompt context lock poisoned") = new;
    }

    fn current(&self) -> Arc<dyn PromptContextProvider> {
        self.0.read().expect("prompt context lock poisoned").clone()
    }

    pub async fn context_for_agents(
        &self,
        agent_ids: &[Uuid],
        query: &str,
    ) -> HashMap<Uuid, String> {
        self.current().context_for_agents(agent_ids, query).await
    }

    pub async fn context_for_agent(&self, agent_id: Uuid, query: &str) -> Option<String> {
        self.current().context_for_agent(agent_id, query).await
    }

    pub async fn org_context(&self) -> Option<String> {
        self.current().org_context().await
    }

    pub async fn context_for_user(&self, user_id: Uuid, query: &str) -> Option<String> {
        self.current().context_for_user(user_id, query).await
    }
}

/// Adapts the prompt-context seam to `nasiko_orchestrator`'s `OutboundPrefix`.
///
/// The MAF worker is spawned once at startup and outlives every request, so it cannot be handed
/// a resolved string the way the chat path's `OrchestratorConfig::outbound_prefix` is — it needs
/// something it can ask, per execution, which is what that trait is. This is the adapter: it owns
/// nothing, and forwards to the same `SwappablePromptContext` every request path reads, so the
/// two orchestrators can never end up carrying different text.
///
/// It also supplies the join, so `nasiko-orchestrator` does not need its own copy of
/// [`prefixed`] — that crate cannot reach `nasiko-react-agent`, and a second implementation of a
/// three-line convention is exactly the kind of thing that drifts unnoticed.
pub struct PromptContextOutbound(Arc<SwappablePromptContext>);

impl PromptContextOutbound {
    pub fn new(cell: Arc<SwappablePromptContext>) -> Self {
        Self(cell)
    }
}

impl std::fmt::Debug for PromptContextOutbound {
    /// `OutboundPrefix` requires `Debug` because the config that carries it is logged on error
    /// paths; the cell behind this holds a trait object with nothing useful to print, so name the
    /// adapter and stop there rather than making the seam un-loggable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PromptContextOutbound")
    }
}

#[async_trait]
impl nasiko_orchestrator::OutboundPrefix for PromptContextOutbound {
    async fn resolve(&self) -> Option<String> {
        self.0.org_context().await
    }

    fn compose(&self, prefix: Option<&str>, body: &str) -> String {
        prefixed(prefix, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedContextProvider(&'static str);

    struct OrgContextProvider(&'static str);

    #[async_trait]
    impl PromptContextProvider for OrgContextProvider {
        async fn context_for_agents(
            &self,
            _agent_ids: &[Uuid],
            _query: &str,
        ) -> HashMap<Uuid, String> {
            HashMap::new()
        }

        async fn context_for_agent(&self, _agent_id: Uuid, _query: &str) -> Option<String> {
            None
        }

        async fn org_context(&self) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[async_trait]
    impl PromptContextProvider for FixedContextProvider {
        async fn context_for_agents(
            &self,
            _agent_ids: &[Uuid],
            _query: &str,
        ) -> HashMap<Uuid, String> {
            HashMap::new()
        }

        async fn context_for_agent(&self, _agent_id: Uuid, _query: &str) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    /// The MAF worker reaches org context through `OutboundPrefix` rather than the request-path
    /// methods, so the adapter has to read the SAME cell — a worker left holding its own copy
    /// would keep injecting whatever was installed at boot for the life of the process.
    #[tokio::test]
    async fn the_outbound_adapter_reads_through_the_shared_cell() {
        use nasiko_orchestrator::OutboundPrefix as _;

        let cell = Arc::new(SwappablePromptContext::new(Arc::new(
            NoopPromptContextProvider,
        )));
        let outbound = PromptContextOutbound::new(cell.clone());

        assert_eq!(outbound.resolve().await, None, "no provider installed yet");
        assert_eq!(
            outbound.compose(None, "step prompt"),
            "step prompt",
            "nothing to add must leave a step's prompt byte-identical"
        );

        cell.install(Arc::new(OrgContextProvider("POLICY")));

        assert_eq!(outbound.resolve().await, Some("POLICY".to_string()));
        assert_eq!(
            outbound.compose(Some("POLICY"), "step prompt"),
            "POLICY\n\nstep prompt",
            "the join must match the one every other call site uses"
        );
    }

    /// Reproduces the exact HITL-resume-dispatcher bug: a background task clones `AppState`
    /// (here, just the `Arc<SwappablePromptContext>` field) BEFORE the EE composition root
    /// installs the real implementation. The clone must still see the swap, because
    /// `AppState::clone()` only clones the `Arc` pointer to this one shared cell.
    #[tokio::test]
    async fn install_is_visible_to_a_clone_taken_before_the_install_call() {
        let cell = Arc::new(SwappablePromptContext::new(Arc::new(
            NoopPromptContextProvider,
        )));

        // Simulates `hitl_state = state.clone()`, captured before `build_ee_app` runs.
        let pre_install_clone = cell.clone();
        assert_eq!(
            pre_install_clone.context_for_agent(Uuid::nil(), "q").await,
            None,
            "still the OSS no-op before install"
        );

        // Simulates `build_ee_app`'s `state.prompt_context.install(...)`.
        cell.install(Arc::new(FixedContextProvider("ee context")));

        // The EARLIER clone — not `cell` itself — must now see the installed implementation.
        assert_eq!(
            pre_install_clone.context_for_agent(Uuid::nil(), "q").await,
            Some("ee context".to_string()),
            "a clone taken before install() must still observe the swap"
        );
    }

    /// A provider that only has per-agent content must not have to know that org-wide content is
    /// a concept — `FixedContextProvider` names no `org_context`, and gets the silent default.
    #[tokio::test]
    async fn a_provider_that_ignores_org_context_contributes_none() {
        assert_eq!(FixedContextProvider("ee context").org_context().await, None);
    }

    /// Same silent default for the user-scoped seam: a provider that only has per-agent content
    /// names no `context_for_user` and must still compile and answer `None`, not force every
    /// existing implementor (including the OSS no-op) to learn about a per-user store that may
    /// not exist yet.
    #[tokio::test]
    async fn a_provider_that_ignores_context_for_user_contributes_none() {
        assert_eq!(
            FixedContextProvider("ee context")
                .context_for_user(Uuid::nil(), "q")
                .await,
            None
        );
        assert_eq!(
            NoopPromptContextProvider
                .context_for_user(Uuid::nil(), "q")
                .await,
            None
        );
    }

    /// `SwappablePromptContext` must forward this method through the same shared cell as every
    /// other one — the exact property `install_is_visible_to_a_clone_taken_before_the_install_call`
    /// pins for `context_for_agent`, repeated here so a future implementor of this method inherits
    /// the same swap-visibility guarantee without re-deriving it.
    #[tokio::test]
    async fn context_for_user_is_forwarded_through_the_swappable_cell() {
        struct UserContextProvider(&'static str);

        #[async_trait]
        impl PromptContextProvider for UserContextProvider {
            async fn context_for_agents(
                &self,
                _agent_ids: &[Uuid],
                _query: &str,
            ) -> HashMap<Uuid, String> {
                HashMap::new()
            }

            async fn context_for_agent(&self, _agent_id: Uuid, _query: &str) -> Option<String> {
                None
            }

            async fn context_for_user(&self, _user_id: Uuid, _query: &str) -> Option<String> {
                Some(self.0.to_string())
            }
        }

        let cell = Arc::new(SwappablePromptContext::new(Arc::new(
            NoopPromptContextProvider,
        )));
        assert_eq!(cell.context_for_user(Uuid::nil(), "q").await, None);

        cell.install(Arc::new(UserContextProvider("what we know about you")));
        assert_eq!(
            cell.context_for_user(Uuid::nil(), "q").await,
            Some("what we know about you".to_string())
        );
    }
}
