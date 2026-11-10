//! Where a call to an agent goes, asked of whoever knows the agent's kind.
//!
//! The control plane's resolver wakes a parked agent's sandbox, finds a resident one's
//! container, and refuses a harness. Callers that live in leaf crates (the MAF executor, the
//! HITL resume notifier) cannot reach it, so they ask through this one-method seam and the
//! server injects the real implementation. [`RuntimeAddress`] is the pre-seam behaviour (the
//! container runtime's live address, else the stored URL) for callers and tests with no
//! resolver wired.

use std::sync::Arc;

use async_trait::async_trait;
pub use nasiko_runtime::ForwardTarget;
use nasiko_runtime::{ContainerId, ContainerRuntime};
use sqlx::PgPool;
use uuid::Uuid;

/// Who is asking, as far as a resolver needs to know.
#[derive(Debug, Clone, Default)]
pub struct ResolveCaller {
    /// The user the call is for.
    pub user_id: Option<Uuid>,
    /// The flow the call belongs to, so a wake can be announced on its event stream.
    pub flow_id: Option<String>,
}

/// Why no target could be produced. `status` is the HTTP status the control plane would answer
/// with, so a caller can tell "gone" (404) from "try again" (5xx).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ResolveFailure {
    pub status: u16,
    pub message: String,
}

impl ResolveFailure {
    /// The agent is routable but has no endpoint to give (an empty stored URL).
    pub const NO_ENDPOINT: u16 = 424;
    /// The agent is a harness: its turns run in a session, not behind a URL.
    pub const HARNESS: u16 = 501;
    pub const NOT_FOUND: u16 = 404;

    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Unrecoverable by retrying: the agent does not exist.
    pub fn is_permanent(&self) -> bool {
        self.status == Self::NOT_FOUND
    }
}

#[async_trait]
pub trait AgentTargetResolver: Send + Sync {
    /// The URL (and any headers to attach) a call to `agent_id` is sent to, waking the agent
    /// first if it is parked. `context_id` is the A2A contextId the call carries.
    async fn resolve(
        &self,
        agent_id: Uuid,
        caller: &ResolveCaller,
        context_id: Option<&str>,
    ) -> Result<ForwardTarget, ResolveFailure>;
}

/// The container runtime's live address (`forwarding_endpoint()`, so the EE injection proxy
/// still sees the call), else the stored `agents.url`. Cannot wake a parked agent.
pub struct RuntimeAddress {
    db: PgPool,
    runtime: Arc<dyn ContainerRuntime>,
}

impl RuntimeAddress {
    pub fn new(db: PgPool, runtime: Arc<dyn ContainerRuntime>) -> Self {
        Self { db, runtime }
    }
}

#[async_trait]
impl AgentTargetResolver for RuntimeAddress {
    async fn resolve(
        &self,
        agent_id: Uuid,
        _caller: &ResolveCaller,
        _context_id: Option<&str>,
    ) -> Result<ForwardTarget, ResolveFailure> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT transport_path, url FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| ResolveFailure::new(500, format!("db lookup failed: {e}")))?;
        let Some((transport_path, stored_url)) = row else {
            return Err(ResolveFailure::new(
                ResolveFailure::NOT_FOUND,
                "no such agent",
            ));
        };
        // The A2A spec fixes no path: it comes from the agent's card, root when absent.
        let path = match transport_path.as_deref() {
            None | Some("/") | Some("") => "",
            Some(p) => p,
        };
        // `forwarding_endpoint()`, not `endpoint()`: where the call actually goes (the EE
        // injection proxy when memory is enabled).
        if let Ok(target) = self
            .runtime
            .forwarding_endpoint(&ContainerId::from_uuid(agent_id))
            .await
        {
            return Ok(ForwardTarget {
                url: format!("{}{path}", target.url.trim_end_matches('/')),
                headers: target.headers,
            });
        }
        match stored_url.filter(|u| !u.is_empty()) {
            Some(url) => {
                // Bypasses the injection proxy, so this call gets no L1A.
                tracing::warn!(
                    %agent_id,
                    "agent target: live runtime lookup failed, falling back to stored agents.url \
                     (bypasses the injection proxy — this call gets no L1A)"
                );
                Ok(ForwardTarget::from(format!(
                    "{}{path}",
                    url.trim_end_matches('/')
                )))
            }
            None => Err(ResolveFailure::new(
                ResolveFailure::NO_ENDPOINT,
                "no live or stored endpoint",
            )),
        }
    }
}
