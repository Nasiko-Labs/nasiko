//! Which past sessions an import has finished with: read, and every turn they queued settled by the
//! server. A later import skips such a session without reading it again, unless the agent has written
//! to it since, so an import that was stopped picks up where it left off.
//!
//! Kept in `imported.json` beside the queue. Losing it costs time, not data: watermarks still keep a
//! re-read session from queueing a turn twice.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::queue;

const LEDGER_FILE: &str = "imported.json";

/// Sessions done, by `<agent>/<session key>`, with the last activity they had when they were read.
pub struct Ledger {
    path: PathBuf,
    sessions: BTreeMap<String, DateTime<Utc>>,
}

impl Ledger {
    pub fn load() -> Self {
        Self::load_at(&super::state::integrations_dir())
    }

    /// The ledger under `dir`; an unreadable one is empty.
    pub(super) fn load_at(dir: &Path) -> Self {
        let path = dir.join(LEDGER_FILE);
        let sessions = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, sessions }
    }

    /// The session was finished with and hasn't been active since.
    pub fn is_done(&self, agent_id: &str, key: &str, last_active: DateTime<Utc>) -> bool {
        self.sessions
            .get(&entry(agent_id, key))
            .is_some_and(|done| *done >= last_active)
    }

    /// Records sessions as done and saves the ledger.
    pub fn record<'a>(
        &mut self,
        sessions: impl IntoIterator<Item = (&'a str, &'a str, DateTime<Utc>)>,
    ) -> Result<()> {
        for (agent_id, key, last_active) in sessions {
            self.sessions.insert(entry(agent_id, key), last_active);
        }
        queue::atomic_owner_write(&self.path, &serde_json::to_vec(&self.sessions)?)
            .with_context(|| format!("failed to save {}", self.path.display()))
    }
}

fn entry(agent_id: &str, key: &str) -> String {
    format!("{agent_id}/{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_done_until_it_is_active_again_and_that_survives_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let read_at: DateTime<Utc> = "2026-10-20T10:00:00Z".parse().unwrap();
        let mut ledger = Ledger::load_at(dir.path());
        assert!(!ledger.is_done("claude", "a", read_at));

        ledger.record([("claude", "a", read_at)]).unwrap();

        let ledger = Ledger::load_at(dir.path());
        assert!(ledger.is_done("claude", "a", read_at));
        assert!(!ledger.is_done("claude", "a", read_at + chrono::Duration::seconds(1)));
        assert!(!ledger.is_done("codex", "a", read_at));
    }

    #[test]
    fn an_unreadable_ledger_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEDGER_FILE), "not json").unwrap();
        let at = Utc::now();

        assert!(!Ledger::load_at(dir.path()).is_done("claude", "a", at));
    }
}
