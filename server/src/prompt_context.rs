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
}

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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedContextProvider(&'static str);

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
}
