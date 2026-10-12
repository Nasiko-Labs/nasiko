//! Error type for the ACP client.

/// Everything that can go wrong driving an ACP agent.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The agent negotiated a protocol version or capability set we don't
    /// support. The caller should fall back to the legacy hook transport
    /// rather than treat this as a hard failure (TRD §6, ACP-2/15).
    #[error("acp agent is incompatible: {0}")]
    Incompatible(String),

    /// A JSON-RPC / protocol-level error from the agent or the SDK.
    #[error("acp protocol error: {0}")]
    Protocol(String),

    /// The transport (spawn/attach) failed to establish or dropped.
    #[error("acp transport error: {0}")]
    Transport(String),

    /// The connection's background task is gone — the agent exited, the
    /// stream closed, or the handle was dropped.
    #[error("acp connection closed")]
    ConnectionClosed,

    /// The turn was cancelled by the control plane — it exceeded its time or
    /// token budget, so `session/cancel` was dispatched and the turn ended early
    /// (TRD §3.4, ACP-9).
    #[error("acp turn cancelled: {0}")]
    Cancelled(String),
}

impl AcpError {
    /// True when the failure means "this agent can't speak our protocol" —
    /// the signal to degrade to the hook path instead of erroring out.
    pub fn is_incompatible(&self) -> bool {
        matches!(self, AcpError::Incompatible(_))
    }
}

impl From<agent_client_protocol::Error> for AcpError {
    fn from(e: agent_client_protocol::Error) -> Self {
        AcpError::Protocol(e.to_string())
    }
}
