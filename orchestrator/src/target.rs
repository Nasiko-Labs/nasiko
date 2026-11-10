//! Where a MAF step's agent call is sent.
//!
//! The seam lives in `nasiko-hitl` (the HITL notifier needs the same one and this crate already
//! depends on it); this module re-exports it and supplies the default.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

pub use nasiko_hitl::{AgentTargetResolver, ForwardTarget, ResolveCaller, ResolveFailure};

/// The agent's address as the catalog stores it, with no way to wake anything: what a MAF step
/// used before the seam. A parked agent is refused with the reason, not left to fail as a
/// connection error against a dead address.
pub struct DirectAddress {
    db: PgPool,
}

impl DirectAddress {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AgentTargetResolver for DirectAddress {
    async fn resolve(
        &self,
        agent_id: Uuid,
        _caller: &ResolveCaller,
        _context_id: Option<&str>,
    ) -> Result<ForwardTarget, ResolveFailure> {
        use nasiko_agent_proxy::ResolveError;
        let agent = nasiko_agent_proxy::resolve(&self.db, agent_id)
            .await
            .map_err(|e| match e {
                ResolveError::NotFound => {
                    ResolveFailure::new(ResolveFailure::NOT_FOUND, e.to_string())
                }
                ResolveError::NotRunning(_) => ResolveFailure::new(503, e.to_string()),
                ResolveError::Database(_) => ResolveFailure::new(500, e.to_string()),
            })?;
        if agent.parked {
            return Err(ResolveFailure::new(
                503,
                "agent is parked and this caller cannot wake it",
            ));
        }
        agent
            .endpoint
            .map(|e| ForwardTarget::from(format!("http://{}:{}", e.host, e.port)))
            .ok_or_else(|| ResolveFailure::new(ResolveFailure::NO_ENDPOINT, "no stored endpoint"))
    }
}
