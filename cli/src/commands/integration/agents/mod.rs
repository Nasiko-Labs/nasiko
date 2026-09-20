//! Closed set of supported coding agents and immediate adapter delegation.

use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;

use super::catalog::{AgentSpec, Support};
use super::model::SessionSnapshot;

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;

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

    pub fn restart_required(self) -> bool {
        matches!(self, Self::OpenCode)
    }

    pub fn is_instrumented(self) -> bool {
        self.spec().support == Support::Instrumented
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrumented_adapters_have_independent_install_versions() {
        assert_eq!(Agent::Claude.install_version(), Some(3));
        assert_eq!(Agent::OpenCode.install_version(), Some(4));
        assert_eq!(Agent::Codex.install_version(), Some(2));
        assert_eq!(Agent::Cursor.install_version(), Some(2));
    }
}
