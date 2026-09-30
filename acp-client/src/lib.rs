//! ACP (Agent Client Protocol) client.
//!
//! Speaks JSON-RPC over stdio to a coding agent (e.g. `opencode acp`), drives
//! one turn at a time, and streams the agent's `session/update`s back to the
//! caller. The transport is abstracted: a spawned child process (local) or a
//! byte stream from a container attach (Issue 2) both plug into
//! [`AcpConnection::connect`].
//!
//! Pinned to the ACP v1 schema. The `unstable_*` SDK features are deliberately
//! off; capability negotiation ([`capability`]) rejects agents that can't speak
//! v1 so the caller can fall back to the legacy hook transport (TRD §6).
//!
//! # Shape
//!
//! ```ignore
//! use nasiko_acp_client::{connect_local, AcpConnectConfig, NewSessionParams,
//!     PermissionPolicy, TurnEvent};
//!
//! let config = AcpConnectConfig { permission_policy: PermissionPolicy::ApproveAll, ..Default::default() };
//! let conn = connect_local("opencode acp", config).await?;
//! let session = conn.new_session(NewSessionParams::new(std::env::current_dir()?)).await?;
//! let mut turn = conn.prompt(session, "list the files here")?;
//! while let Some(event) = turn.recv().await {
//!     match event {
//!         TurnEvent::Update(update) => { /* map to CodingAgentEventV1 / SSE */ }
//!         TurnEvent::Finished(stop) => { println!("done: {stop:?}"); break; }
//!         TurnEvent::Failed(err)    => { eprintln!("turn failed: {err}"); break; }
//!     }
//! }
//! ```
//!
//! NAS-836 · see `docs/TECH-2026-004-acp-coding-agent.md` §3.1/§3.4.

mod capability;
mod connection;
mod error;
mod permission;
mod turn;

pub use capability::{NegotiatedCapabilities, RequiredCapabilities};
pub use connection::{AcpConnectConfig, AcpConnection, connect_local};
pub use error::AcpError;
pub use permission::PermissionPolicy;
pub use turn::{NewSessionParams, TurnEvent, TurnStream};

// Re-export the SDK types that appear in this crate's public API, so callers
// don't need a direct dependency on `agent-client-protocol` to use them.
pub use agent_client_protocol::schema::ProtocolVersion;
pub use agent_client_protocol::schema::v1::{McpServer, SessionId, SessionUpdate, StopReason};

/// The underlying SDK, re-exported for callers that build a custom transport
/// (e.g. `ByteStreams` over a container attach) to pass to
/// [`AcpConnection::connect`].
pub use agent_client_protocol as sdk;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_safe() {
        let config = AcpConnectConfig::default();
        assert_eq!(config.permission_policy, PermissionPolicy::RejectAll);
        assert_eq!(config.required.protocol_version, ProtocolVersion::V1);
    }

    #[test]
    fn incompatible_error_is_flagged() {
        let err = AcpError::Incompatible("wrong version".into());
        assert!(err.is_incompatible());
        assert!(!AcpError::ConnectionClosed.is_incompatible());
    }
}
