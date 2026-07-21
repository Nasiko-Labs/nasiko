//! Self-heal seam for `uploaded_build` MCP connectors, whose container's live
//! address can drift from what's stored in `mcp_connectors.url` (a container
//! restart, redeploy, or host reboot all change the underlying address).
//!
//! This crate deliberately has no `ContainerRuntime` dependency (see
//! `lib.rs`'s crate-boundary doc), so the actual refresh — which needs one —
//! can't live here. Instead, [`McpState`] holds a swappable
//! [`EndpointRefresher`], defaulting to [`NoopEndpointRefresher`]; `oss/server`
//! wires in a real, `ContainerRuntime`-backed impl once at startup (mirrors
//! `ConnectorAuthorizer`'s exact same swap pattern on the same struct).
//!
//! [`McpState`]: crate::state::McpState

use async_trait::async_trait;
use uuid::Uuid;

/// Refreshes an `uploaded_build` connector's live container address after a
/// connection-level failure (refused/timeout/DNS — not an application-level
/// MCP error). Returns `Some(new_url)` on success, `None` if refresh isn't
/// possible (e.g. the container is genuinely gone).
#[async_trait]
pub trait EndpointRefresher: Send + Sync {
    async fn refresh(&self, connector_id: Uuid) -> Option<String>;
}

/// Default — used until `oss/server` swaps in a real impl at `AppState`
/// construction. Never refreshes; every connection failure just surfaces as
/// the ordinary "backend failed" error.
pub struct NoopEndpointRefresher;

#[async_trait]
impl EndpointRefresher for NoopEndpointRefresher {
    async fn refresh(&self, _connector_id: Uuid) -> Option<String> {
        None
    }
}
