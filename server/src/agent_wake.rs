//! Prepares an agent container before forwarding a call to it.
//!
//! This trait is called just before each place that forwards a call to an agent: the agent
//! proxy, direct A2A dispatch, a routed orchestrator leg (`CpCallGuard::before_call`) and the
//! HITL resume. It is not called while the orchestrator lists the agents it may choose from,
//! since listing forwards nothing. The open-source implementation
//! ([`NoAgentWake`]) does nothing. An edition that needs to prepare a container
//! before it receives traffic implements this trait and installs it through
//! [`SwappableAgentWake`] on [`crate::state::AppState`] at its composition root.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait AgentWake: Send + Sync {
    /// Called just before forwarding a request to an agent. Returns `Ok(())` if
    /// the agent is ready to receive traffic. An `Err` refuses the call with the
    /// given message (a 503 for an HTTP caller) and the forward is not attempted.
    async fn before_forward(&self, agent_id: Uuid) -> Result<(), String>;
}

/// The open-source behaviour: no preparation needed.
pub struct NoAgentWake;

#[async_trait]
impl AgentWake for NoAgentWake {
    async fn before_forward(&self, _agent_id: Uuid) -> Result<(), String> {
        Ok(())
    }
}

/// Holds an `AgentWake` behind a lock so the implementation can be swapped after `AppState` has
/// already been cloned - needed for exactly the reason
/// `prompt_context::SwappablePromptContext` exists (see its doc comment): the EE composition
/// root's real implementation is installed by `build_ee_app`, but the HITL resume dispatcher is
/// spawned with an `AppState` clone taken earlier, inside `AppState::from_config_with_db`,
/// strictly before `build_ee_app` ever runs. Reassigning a plain `Arc<dyn AgentWake>` field only
/// rebinds *that* later clone's own pointer - the dispatcher's earlier clone would keep asking
/// the OSS no-op for its entire lifetime, and a resumed agent would never be prepared. Installing
/// *through* this shared cell instead makes the swap visible to every existing and future clone.
pub struct SwappableAgentWake(RwLock<Arc<dyn AgentWake>>);

impl SwappableAgentWake {
    pub fn new(initial: Arc<dyn AgentWake>) -> Self {
        Self(RwLock::new(initial))
    }

    /// Replaces the underlying implementation for every clone, past and future, of the
    /// `AppState` this cell lives in.
    pub fn install(&self, new: Arc<dyn AgentWake>) {
        *self.0.write().expect("agent wake lock poisoned") = new;
    }
}

#[async_trait]
impl AgentWake for SwappableAgentWake {
    async fn before_forward(&self, agent_id: Uuid) -> Result<(), String> {
        let current = self.0.read().expect("agent wake lock poisoned").clone();
        current.before_forward(agent_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Refusing;

    #[async_trait]
    impl AgentWake for Refusing {
        async fn before_forward(&self, _agent_id: Uuid) -> Result<(), String> {
            Err("not ready".to_string())
        }
    }

    #[tokio::test]
    async fn the_default_never_refuses() {
        assert_eq!(NoAgentWake.before_forward(Uuid::new_v4()).await, Ok(()));
    }

    /// The bug the cell exists to prevent: a task clones `AppState` (here, the `Arc` to the cell)
    /// BEFORE the composition root installs the real implementation, and must still see it.
    #[tokio::test]
    async fn install_is_visible_to_a_clone_taken_before_the_install_call() {
        let cell = Arc::new(SwappableAgentWake::new(Arc::new(NoAgentWake)));
        let pre_install_clone = cell.clone();
        assert_eq!(
            pre_install_clone.before_forward(Uuid::new_v4()).await,
            Ok(())
        );

        cell.install(Arc::new(Refusing));

        assert_eq!(
            pre_install_clone.before_forward(Uuid::new_v4()).await,
            Err("not ready".to_string())
        );
    }
}
