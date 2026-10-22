//! Error type for the ACP client.

/// Everything that can go wrong driving an ACP agent.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The agent negotiated a protocol version or capability set we don't
    /// support. The caller should fall back to the legacy hook transport
    /// rather than treat this as a hard failure (TRD §6, ACP-2/15).
    #[error("acp agent is incompatible: {0}")]
    Incompatible(String),

    /// A JSON-RPC / protocol-level error from the agent or the SDK. `code` is the
    /// JSON-RPC error code, kept rather than folded into the message so callers
    /// can tell e.g. `auth_required` (-32000) apart from any other failure —
    /// see [`is_auth_required`](AcpError::is_auth_required).
    #[error("acp protocol error {code}: {message}")]
    Protocol { code: i32, message: String },

    /// Caller-supplied parameters rejected before anything is sent to the agent
    /// (e.g. a relative `cwd`, which the protocol requires to be absolute). Fails
    /// fast with a clear cause instead of a late, opaque agent-side error.
    #[error("acp invalid parameters: {0}")]
    InvalidParams(String),

    /// The transport (spawn/attach) failed to establish or dropped.
    #[error("acp transport error: {0}")]
    Transport(String),

    /// The connection's background task is gone — the agent exited, the
    /// stream closed, or the handle was dropped.
    #[error("acp connection closed")]
    ConnectionClosed,

    /// A prompt was sent while another turn is still running on this connection.
    /// One turn runs at a time per connection (the platform's one-active-turn
    /// rule, TRD §9.4); the caller must await the current turn first.
    #[error("acp turn already in progress")]
    TurnInProgress,

    /// The turn was cancelled by the control plane — it exceeded its time or
    /// token budget, so `session/cancel` was dispatched and the turn ended early
    /// (TRD §3.4, ACP-9).
    #[error("acp turn cancelled: {0}")]
    Cancelled(String),

    /// A request did not complete within its configured budget — e.g. the agent
    /// hung during `initialize`/`session/new`. Bounds the handshake so a wedged
    /// agent can't block `connect()` forever and the caller can fall back.
    #[error("acp request timed out: {0}")]
    Timeout(String),
}

impl AcpError {
    /// True when the failure means "this agent can't speak our protocol" —
    /// the signal to degrade to the hook path instead of erroring out.
    pub fn is_incompatible(&self) -> bool {
        matches!(self, AcpError::Incompatible(_))
    }

    /// True when the agent answered with JSON-RPC `auth_required` (-32000) — it
    /// needs an `authenticate` step before it will open sessions. Distinct from
    /// every other protocol error, which this client surfaces but does not treat
    /// specially.
    pub fn is_auth_required(&self) -> bool {
        matches!(
            self,
            AcpError::Protocol { code, .. }
                if *code == i32::from(agent_client_protocol::ErrorCode::AuthRequired)
        )
    }
}

impl From<agent_client_protocol::Error> for AcpError {
    fn from(e: agent_client_protocol::Error) -> Self {
        AcpError::Protocol {
            code: i32::from(e.code),
            message: e.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_error_preserves_code_and_flags_auth_required() {
        let err = AcpError::from(agent_client_protocol::Error::auth_required());
        assert!(err.is_auth_required());
        match err {
            AcpError::Protocol { code, .. } => assert_eq!(code, -32000),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn a_non_auth_protocol_error_is_not_auth_required() {
        let err = AcpError::from(agent_client_protocol::Error::method_not_found());
        assert!(!err.is_auth_required());
        assert!(matches!(err, AcpError::Protocol { .. }));
        assert!(!AcpError::ConnectionClosed.is_auth_required());
    }
}
