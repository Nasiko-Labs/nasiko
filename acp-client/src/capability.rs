//! Capability negotiation at `initialize`.
//!
//! We pin the stable v1 protocol. If the agent comes back on a different
//! version we can't drive it, and the caller degrades to the hook path.
//! The capabilities the agent *does* advertise (load/resume/mcp) are surfaced
//! so later phases (cold-path restore, MCP-server injection) can branch on them.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::InitializeResponse;

/// What this client requires of an agent. Today: the v1 protocol.
#[derive(Debug, Clone)]
pub struct RequiredCapabilities {
    /// The single protocol version we speak.
    pub protocol_version: ProtocolVersion,
}

impl Default for RequiredCapabilities {
    fn default() -> Self {
        Self {
            protocol_version: ProtocolVersion::V1,
        }
    }
}

/// What the agent advertised back, distilled to the flags later phases care about.
#[derive(Debug, Clone)]
pub struct NegotiatedCapabilities {
    /// The protocol version the agent agreed on (always v1 once negotiation succeeds).
    pub protocol_version: ProtocolVersion,
    /// Agent supports `session/load` — cold-path restore by replay.
    pub load_session: bool,
    /// Agent supports `session/resume` — cold-path restore without replay.
    pub resume: bool,
    /// Agent accepts HTTP MCP servers in `session/new` (how we inject the gateway).
    pub mcp_http: bool,
    /// Agent accepts SSE MCP servers in `session/new`.
    pub mcp_sse: bool,
}

/// Compare the agent's `initialize` response against what we require.
///
/// `Err` means "incompatible, fall back to the hook path"; the string is a
/// human-readable reason for the warning log.
pub fn negotiate(
    init: &InitializeResponse,
    required: &RequiredCapabilities,
) -> Result<NegotiatedCapabilities, String> {
    if init.protocol_version != required.protocol_version {
        return Err(format!(
            "agent negotiated protocol {:?}, this client requires {:?}",
            init.protocol_version, required.protocol_version
        ));
    }

    let caps = &init.agent_capabilities;
    Ok(NegotiatedCapabilities {
        protocol_version: init.protocol_version,
        load_session: caps.load_session,
        resume: caps.session_capabilities.resume.is_some(),
        mcp_http: caps.mcp_capabilities.http,
        mcp_sse: caps.mcp_capabilities.sse,
    })
}
