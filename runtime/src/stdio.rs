//! Per-session stdio attach for ACP coding agents.
//!
//! An ACP ([Agent Client Protocol]) coding agent speaks JSON-RPC over stdio,
//! not HTTP. The control plane drives it by opening a **bidirectional byte
//! stream** to one agent process and exchanging newline-delimited JSON-RPC
//! frames — a fundamentally different shape from the HTTP `endpoint()` every
//! other workload exposes, which is why it gets its own trait method
//! ([`ContainerRuntime::attach_stdio`](crate::ContainerRuntime::attach_stdio))
//! rather than overloading `endpoint()`.
//!
//! [Agent Client Protocol]: https://agentclientprotocol.com

use std::collections::HashMap;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::{Result, RuntimeError};

/// Parameters for opening one coding-agent **session** as its own process
/// inside a (possibly reused) coding-agent container.
///
/// The runtime model is "one container per agent, one process per session,
/// container reused across sessions": a single coding-agent container serves
/// many sessions, and each session is a separate agent process the runtime
/// execs on attach. This carries the per-process detail that differs between
/// those sessions; the container-wide detail lives on the
/// [`DeploymentSpec`](crate::DeploymentSpec) that deployed the container.
#[derive(Debug, Clone)]
pub struct SessionAttach {
    /// Opaque per-session identifier, carried for tracing/logs. On-disk session
    /// layout is internal to the ACP agent, so this is **not** a workspace
    /// subdirectory; the agent is launched in the writable mount root. It is
    /// still validated as a single safe path segment (it rides an exec argv), by
    /// [`SessionAttach::validate`].
    pub session_id: String,
    /// The argv the container execs for this session (e.g.
    /// `["opencode", "acp"]`). Passed as an argument vector, never a shell
    /// string, so it is data to the runtime rather than code.
    pub command: Vec<String>,
    /// Extra environment for this session's process, layered over the
    /// container's own environment. The Layer-B session credential (the JWT
    /// bound to `{agent_id, user_id, session_id, flow_id}`) is delivered here
    /// per session rather than baked into the image.
    pub env: HashMap<String, String>,
}

impl SessionAttach {
    /// Validate before handing to a backend. `session_id` rides an exec argv, so
    /// it must not be able to smuggle shell/label metacharacters or `..`; the
    /// command must be non-empty (there is nothing to exec otherwise).
    pub fn validate(&self) -> Result<()> {
        // The session id becomes a folder name and part of a command, so only
        // allow plain, safe characters: reject empty, over-long, `.`/`..`, and
        // anything other than letters, digits, `-` or `_`. This stops a crafted
        // id from climbing out of its folder (`..`) or sneaking in shell tricks.
        let id = &self.session_id;
        let bad = id.is_empty()
            || id.len() > 63
            || id == "."
            || id == ".."
            || id
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        if bad {
            return Err(RuntimeError::InvalidSpec(format!(
                "session_id {id:?} must be a single path segment of [A-Za-z0-9_-], 1..=63 chars"
            )));
        }
        // There must be something to launch, or the attach is meaningless.
        if self.command.is_empty() {
            return Err(RuntimeError::InvalidSpec(
                "session command must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A live, bidirectional byte stream into one coding-agent process.
///
/// The two halves are independent and framed as **newline-delimited JSON-RPC**
/// over a raw stream (never a TTY): the ACP client writes requests to
/// [`stdin`](StdioHandle::stdin) and reads responses and `session/update`
/// notifications from [`stdout`](StdioHandle::stdout). The process's own stderr
/// is diagnostics, not protocol, so it is **not** interleaved here — backends
/// route it to their logs.
///
/// # Trait family (bridging to the ACP SDK)
///
/// The halves are **tokio** `AsyncRead`/`AsyncWrite`. The `agent-client-protocol`
/// SDK's transport (`ByteStreams`) speaks **futures-io**, so the layer that joins
/// the two wraps each half with
/// [`tokio_util::compat`](https://docs.rs/tokio-util/latest/tokio_util/compat/)
/// (`.compat()` / `.compat_write()`) before `ByteStreams::new(...)`. That means
/// adding `tokio-util` (feature `compat`) to the glue crate — it is deliberately
/// *not* a dependency here, since this crate needs no conversion of its own.
///
/// # Dropping / process lifetime
///
/// Dropping the handle closes the pipe: `stdin` reaches EOF and the read half
/// ends. A mid-turn drop is itself the turn's cancel signal, so there is no
/// separate close/cancel call on this type. The runtime does **not** explicitly
/// kill the exec'd process on drop — it relies on the agent exiting on stdin EOF
/// (OpenCode does). Whether a dropped turn should *kill* the process or keep it
/// warm for the next turn is a session-lifecycle decision owned by the caller,
/// not this handle.
pub struct StdioHandle {
    /// Write half — the agent process's stdin.
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    /// Read half — the agent process's stdout (stdout frames only).
    pub stdout: Pin<Box<dyn AsyncRead + Send>>,
}

impl std::fmt::Debug for StdioHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioHandle").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attach(session_id: &str) -> SessionAttach {
        SessionAttach {
            session_id: session_id.to_owned(),
            command: vec!["opencode".to_owned(), "acp".to_owned()],
            env: HashMap::new(),
        }
    }

    #[test]
    fn accepts_a_uuid_session_id() {
        assert!(
            attach("6b1f2c9a-0e4d-4a1b-9c3e-2f5a7d8e1b0c")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rejects_path_traversal_and_separators() {
        for id in ["..", ".", "a/b", "../etc", "a b", "a:b", ""] {
            assert!(
                attach(id).validate().is_err(),
                "session_id {id:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_empty_command() {
        let mut a = attach("s1");
        a.command.clear();
        assert!(a.validate().is_err());
    }
}
