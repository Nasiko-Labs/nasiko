//! Prepares an agent before forwarding a call to it.
//!
//! This trait is called just before each place that forwards a call to an agent, ahead of
//! resolving where the call goes: the agent proxy, direct A2A dispatch, a routed orchestrator leg
//! (`CpCallGuard::before_call`), the HITL resume, and every caller that resolves through
//! `agent_resolver::ServerTargetResolver` (MAF steps, HITL nudges). Start on a parked agent
//! (`admin::routes::start`) asks it too, so Start is the wake the next call would get. It is not
//! called while the orchestrator lists the agents it may choose from, since listing forwards nothing.
//!
//! The platform default ([`parked::ParkedAgentWake`]) scales a parked agent's own workload back up
//! and waits until it is reachable. [`NoAgentWake`] does nothing. An edition that prepares agents
//! differently implements this trait and installs it through [`SwappableAgentWake`] on
//! [`crate::state::AppState`] at its composition root.

pub mod parked;

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use sqlx::PgConnection;
use uuid::Uuid;

#[async_trait]
pub trait AgentWake: Send + Sync {
    /// Called just before resolving where a request to an agent goes and forwarding it. Returns
    /// `Ok(())` if the agent is ready to receive traffic. An `Err` refuses the call with the given
    /// message (a 503 for an HTTP caller) and the forward is not attempted. `flow_id` is the flow
    /// the call belongs to, so a wake can be announced on its event stream.
    async fn before_forward(&self, agent_id: Uuid, flow_id: Option<&str>) -> Result<(), String>;
}

/// Prepares nothing: every agent is taken as ready.
pub struct NoAgentWake;

#[async_trait]
impl AgentWake for NoAgentWake {
    async fn before_forward(&self, _agent_id: Uuid, _flow_id: Option<&str>) -> Result<(), String> {
        Ok(())
    }
}

/// The advisory-lock key of an agent (`$1` is its id as text), taken in a transaction
/// (`pg_advisory_xact_lock`) so it is released at commit or rollback. The platform's wake
/// ([`parked::ParkedAgentWake`]) and the idle reaper's park of a running workload take it, which
/// is what makes a wake and a park of one agent exclude each other. Any park that scales a running
/// workload to zero must take it around its claim and its scale to zero, or it can scale down an
/// agent a wake has just started.
pub const AGENT_LOCK_KEY_SQL: &str = "hashtextextended($1::text, 0)";

/// Takes the agent's lock ([`AGENT_LOCK_KEY_SQL`]) on `tx`, waiting no later than `deadline`.
/// Released when `tx` commits or rolls back, including when its owner drops it mid-wait.
pub async fn lock_agent(
    tx: &mut PgConnection,
    agent_id: Uuid,
    deadline: Instant,
) -> sqlx::Result<()> {
    sqlx::query("SELECT set_config('lock_timeout', $1, true)")
        .bind(lock_timeout(
            deadline.saturating_duration_since(Instant::now()),
        ))
        .execute(&mut *tx)
        .await?;
    sqlx::query(&format!(
        "SELECT pg_advisory_xact_lock({AGENT_LOCK_KEY_SQL})"
    ))
    .bind(agent_id.to_string())
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Takes the agent's lock ([`AGENT_LOCK_KEY_SQL`]) on `tx` only if nobody holds it: `false` when a
/// wake or another park of the agent has it. For a sweep, which looks again next time rather than
/// wait behind a wake. Released when `tx` commits or rolls back.
pub async fn try_lock_agent(tx: &mut PgConnection, agent_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(&format!(
        "SELECT pg_try_advisory_xact_lock({AGENT_LOCK_KEY_SQL})"
    ))
    .bind(agent_id.to_string())
    .fetch_one(&mut *tx)
    .await
}

/// The `lock_timeout` value for the time left. Postgres reads 0 as "no timeout", so an exhausted
/// budget still asks for 1ms rather than waiting forever.
fn lock_timeout(remaining: Duration) -> String {
    format!("{}ms", remaining.as_millis().max(1))
}

/// Holds an `AgentWake` behind a lock so the implementation can be swapped after `AppState` has
/// already been cloned - needed for exactly the reason
/// `prompt_context::SwappablePromptContext` exists (see its doc comment): the EE composition
/// root's real implementation is installed by `build_ee_app`, but the HITL resume dispatcher is
/// spawned with an `AppState` clone taken earlier, inside `AppState::from_config_with_db`,
/// strictly before `build_ee_app` ever runs. Reassigning a plain `Arc<dyn AgentWake>` field only
/// rebinds *that* later clone's own pointer - the dispatcher's earlier clone would keep asking
/// the default for its entire lifetime, and a resumed agent would never be prepared. Installing
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

    /// The implementation installed now, for an edition that wraps it instead of replacing it.
    pub fn current(&self) -> Arc<dyn AgentWake> {
        self.0.read().expect("agent wake lock poisoned").clone()
    }
}

#[async_trait]
impl AgentWake for SwappableAgentWake {
    async fn before_forward(&self, agent_id: Uuid, flow_id: Option<&str>) -> Result<(), String> {
        self.current().before_forward(agent_id, flow_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Refusing;

    #[async_trait]
    impl AgentWake for Refusing {
        async fn before_forward(
            &self,
            _agent_id: Uuid,
            _flow_id: Option<&str>,
        ) -> Result<(), String> {
            Err("not ready".to_string())
        }
    }

    #[tokio::test]
    async fn no_agent_wake_never_refuses() {
        assert_eq!(
            NoAgentWake.before_forward(Uuid::new_v4(), None).await,
            Ok(())
        );
    }

    /// The bug the cell exists to prevent: a task clones `AppState` (here, the `Arc` to the cell)
    /// BEFORE the composition root installs the real implementation, and must still see it.
    #[tokio::test]
    async fn install_is_visible_to_a_clone_taken_before_the_install_call() {
        let cell = Arc::new(SwappableAgentWake::new(Arc::new(NoAgentWake)));
        let pre_install_clone = cell.clone();
        assert_eq!(
            pre_install_clone.before_forward(Uuid::new_v4(), None).await,
            Ok(())
        );

        cell.install(Arc::new(Refusing));

        assert_eq!(
            pre_install_clone.before_forward(Uuid::new_v4(), None).await,
            Err("not ready".to_string())
        );
    }

    #[test]
    fn an_exhausted_budget_still_bounds_the_lock_wait() {
        assert_eq!(lock_timeout(Duration::ZERO), "1ms");
        assert_eq!(lock_timeout(Duration::from_micros(400)), "1ms");
        assert_eq!(lock_timeout(Duration::from_millis(1500)), "1500ms");
    }
}
