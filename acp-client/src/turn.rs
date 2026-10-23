//! Turn-level types: what a caller passes to open a session, and what streams
//! back while a prompt runs.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{McpServer, SessionUpdate, StopReason};
use tokio::sync::mpsc;

use crate::error::AcpError;

/// Parameters for `session/new`.
#[derive(Debug, Clone)]
pub struct NewSessionParams {
    /// Absolute working directory for the session (required by the protocol).
    pub cwd: PathBuf,
    /// MCP servers the agent should connect to. Empty for now; Issue 4 injects
    /// the Nasiko MCP gateway here with the session token in its headers.
    pub mcp_servers: Vec<McpServer>,
}

impl NewSessionParams {
    /// A session in `cwd` with no MCP servers.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            mcp_servers: Vec::new(),
        }
    }

    /// Set the MCP servers for this session.
    pub fn with_mcp_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.mcp_servers = servers;
        self
    }
}

/// One event in a running turn.
///
/// A turn produces zero or more `Update`s followed by exactly one terminal
/// event (`Finished` or `Failed`). `SessionUpdate` is the SDK's own type, so
/// callers (telemetry, SSE translation) see the full detail.
#[derive(Debug)]
pub enum TurnEvent {
    /// A streamed `session/update` notification from the agent. Boxed because
    /// `SessionUpdate` is large relative to the terminal variants.
    Update(Box<SessionUpdate>),
    /// The turn ended; carries the agent's stop reason.
    Finished(StopReason),
    /// The turn failed (protocol error or dropped connection).
    Failed(AcpError),
}

impl TurnEvent {
    /// True for the terminal events (`Finished` / `Failed`).
    pub fn is_terminal(&self) -> bool {
        matches!(self, TurnEvent::Finished(_) | TurnEvent::Failed(_))
    }
}

/// The stream of events for a single prompt. Drain with [`TurnStream::recv`]
/// until a terminal event arrives.
pub struct TurnStream {
    pub(crate) events: mpsc::Receiver<TurnEvent>,
}

impl TurnStream {
    /// Await the next event, or `None` once the stream is exhausted.
    pub async fn recv(&mut self) -> Option<TurnEvent> {
        self.events.recv().await
    }

    /// Convenience: drain the whole turn and return its stop reason,
    /// discarding the intermediate updates. Handy for tests and simple callers.
    pub async fn wait_for_stop(mut self) -> Result<StopReason, AcpError> {
        while let Some(event) = self.events.recv().await {
            match event {
                TurnEvent::Finished(reason) => return Ok(reason),
                TurnEvent::Failed(err) => return Err(err),
                TurnEvent::Update(_) => {}
            }
        }
        Err(AcpError::ConnectionClosed)
    }
}
