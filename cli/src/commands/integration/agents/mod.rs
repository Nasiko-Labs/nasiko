//! Closed set of supported coding agents and immediate adapter delegation.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Instant;

use super::catalog::{AgentSpec, Support};
use super::model::SessionSnapshot;

pub mod claude;
pub mod codex;
mod codex_history;
pub mod cursor;
mod cursor_history;
pub mod opencode;
mod opencode_history;

/// One session an agent kept on disk from before it was connected, not read yet: listing them is cheap
/// (no transcript is parsed), so an import knows its total and can order sessions newest first before
/// reading any.
pub struct PastSession {
    /// Names the session within its agent, stably across imports (an id or a path).
    pub key: String,
    /// When the session was last written to: the file's modification time or the store's own record.
    pub last_active: DateTime<Utc>,
    pub source: SessionSource,
}

/// How a [`PastSession`] is read.
pub enum SessionSource {
    /// The payload the agent's own hook would carry; its `snapshot` reads the session from it.
    Payload(String),
    /// Reads the session straight from the agent's store. For agents whose hooks build a turn from live
    /// events, where replaying them would stamp every turn with the time of the import.
    Load(Box<dyn FnOnce() -> Result<SessionSnapshot>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
    Cursor,
}

impl Agent {
    pub const ALL: &'static [Self] = &[Self::Claude, Self::Codex, Self::OpenCode, Self::Cursor];

    pub fn spec(self) -> AgentSpec {
        match self {
            Self::Claude => claude::SPEC,
            Self::Codex => codex::SPEC,
            Self::OpenCode => opencode::SPEC,
            Self::Cursor => cursor::SPEC,
        }
    }

    pub fn config_path(self) -> PathBuf {
        match self {
            Self::Claude => claude::config_path(),
            Self::OpenCode => opencode::config_path(),
            Self::Codex => codex::config_path(),
            Self::Cursor => cursor::config_path(),
        }
    }

    pub fn install_version(self) -> Option<u32> {
        match self {
            Self::Claude => Some(claude::INSTALL_VERSION),
            Self::OpenCode => Some(opencode::INSTALL_VERSION),
            Self::Codex => Some(codex::INSTALL_VERSION),
            Self::Cursor => Some(cursor::INSTALL_VERSION),
        }
    }

    pub fn install(self) -> Result<(PathBuf, PathBuf)> {
        match self {
            Self::Claude => claude::install(),
            Self::OpenCode => opencode::install(),
            Self::Codex => codex::install(),
            Self::Cursor => cursor::install(),
        }
    }

    pub fn uninstall(self) -> Result<()> {
        match self {
            Self::Claude => claude::uninstall(),
            Self::OpenCode => opencode::uninstall(),
            Self::Codex => codex::uninstall(),
            Self::Cursor => cursor::uninstall(),
        }
    }

    pub fn installed_version(self) -> Option<u32> {
        match self {
            Self::Claude => claude::installed_version(),
            Self::OpenCode => opencode::installed_version(),
            Self::Codex => codex::installed_version(),
            Self::Cursor => cursor::installed_version(),
        }
    }

    pub fn snapshot(self, raw: &str, deadline: Instant) -> Result<SessionSnapshot> {
        match self {
            Self::Claude => claude::snapshot(raw, deadline),
            Self::OpenCode => opencode::snapshot(raw),
            Self::Codex => codex::snapshot(raw),
            Self::Cursor => cursor::snapshot(raw),
        }
    }

    /// Every session a hook's payload has finished turns in, each read on its own so one that can't be
    /// read doesn't cost the others. One session for most agents; Claude Code keeps each subagent in a
    /// transcript of its own, so its hooks report those too.
    pub fn hook_snapshots(
        self,
        raw: &str,
        deadline: Instant,
    ) -> Result<Vec<Result<SessionSnapshot>>> {
        match self {
            Self::Claude => claude::hook_snapshots(raw, deadline),
            agent => Ok(vec![Ok(agent.snapshot(raw, deadline)?)]),
        }
    }

    /// Reads a past session once. Unlike a hook's [`Self::snapshot`] it never waits for the agent to
    /// finish writing: a session that ended long ago won't, and an unfinished last turn stays uncaptured
    /// until a later import or hook finds it complete.
    pub fn past_snapshot(self, source: SessionSource) -> Result<SessionSnapshot> {
        match (self, source) {
            (Self::Claude, SessionSource::Payload(raw)) => claude::snapshot_once(&raw),
            (agent, SessionSource::Payload(raw)) => agent.snapshot(&raw, Instant::now()),
            (_, SessionSource::Load(load)) => load(),
        }
    }

    /// The sessions this agent kept on disk, oldest first. An unreadable store is an error; a missing
    /// one is no sessions.
    pub fn past_sessions(self) -> Result<Vec<PastSession>> {
        match self {
            Self::Claude => Ok(claude::past_sessions()),
            Self::Codex => Ok(codex_history::past_sessions()),
            Self::OpenCode => opencode_history::past_sessions(),
            Self::Cursor => cursor_history::past_sessions(),
        }
    }

    pub fn restart_required(self) -> bool {
        matches!(self, Self::OpenCode)
    }

    pub fn is_instrumented(self) -> bool {
        self.spec().support == Support::Instrumented
    }
}

/// A store's milliseconds-since-epoch as a session's last activity; out of range is the epoch.
pub(super) fn epoch_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or(DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrumented_adapters_have_independent_install_versions() {
        assert_eq!(Agent::Claude.install_version(), Some(4));
        assert_eq!(Agent::OpenCode.install_version(), Some(8));
        assert_eq!(Agent::Codex.install_version(), Some(2));
        assert_eq!(Agent::Cursor.install_version(), Some(2));
    }
}
