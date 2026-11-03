//! Which past sessions an import has finished with: read, and every turn they queued settled by the
//! server. A later import skips such a session without reading it again, unless the agent has written
//! to it since, so an import that was stopped picks up where it left off.
//!
//! Kept in `imported.json` beside the queue, apart per destination: a session done for one control
//! plane is read again once its agent reports to another. Losing it costs time, not data: watermarks
//! still keep a re-read session from queueing a turn twice.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::queue;
use super::state::{self, IntegrationState};

const LEDGER_FILE: &str = "imported.json";

/// Sessions done, by `<agent>/<session key>` under the destination they were read for, with the
/// last activity they had when they were read.
pub struct Ledger {
    path: PathBuf,
    sessions: Sessions,
    /// Where each agent reports now, by agent id, as a [`state::destination_key`].
    reporting_to: BTreeMap<String, String>,
}

/// The file's shape. A ledger from before destinations were kept is one flat map, which doesn't
/// parse as this and so reads as empty.
#[derive(Default, Serialize, Deserialize)]
struct Sessions {
    by_destination: BTreeMap<String, BTreeMap<String, DateTime<Utc>>>,
}

impl Ledger {
    /// This computer's ledger, for the destinations its agents report to now.
    pub fn load() -> Self {
        let reporting_to = IntegrationState::load()
            .map(|settings| destinations(&settings))
            .unwrap_or_default();
        Self::load_at(&state::integrations_dir()).reporting_to(reporting_to)
    }

    /// The ledger under `dir`; an unreadable one is empty. Agents with no destination given share
    /// one with no name.
    pub(super) fn load_at(dir: &Path) -> Self {
        let path = dir.join(LEDGER_FILE);
        let sessions = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            sessions,
            reporting_to: BTreeMap::new(),
        }
    }

    /// The same ledger, reading and recording each agent under `reporting_to[agent id]`.
    pub(super) fn reporting_to(self, reporting_to: BTreeMap<String, String>) -> Self {
        Self {
            reporting_to,
            ..self
        }
    }

    /// The session was finished with for where its agent reports now, and hasn't been active since.
    pub fn is_done(&self, agent_id: &str, key: &str, last_active: DateTime<Utc>) -> bool {
        self.sessions
            .by_destination
            .get(self.destination(agent_id))
            .and_then(|done| done.get(&entry(agent_id, key)))
            .is_some_and(|done| *done >= last_active)
    }

    /// Records sessions as done for where their agents report now, and saves the ledger.
    pub fn record<'a>(
        &mut self,
        sessions: impl IntoIterator<Item = (&'a str, &'a str, DateTime<Utc>)>,
    ) -> Result<()> {
        for (agent_id, key, last_active) in sessions {
            let destination = self.destination(agent_id).to_string();
            self.sessions
                .by_destination
                .entry(destination)
                .or_default()
                .insert(entry(agent_id, key), last_active);
        }
        queue::atomic_owner_write(&self.path, &serde_json::to_vec(&self.sessions)?)
            .with_context(|| format!("failed to save {}", self.path.display()))
    }

    fn destination(&self, agent_id: &str) -> &str {
        self.reporting_to.get(agent_id).map_or("", String::as_str)
    }
}

/// Where each installed agent reports, by agent id; agents with no binding have none.
fn destinations(settings: &IntegrationState) -> BTreeMap<String, String> {
    settings
        .agents
        .iter()
        .filter_map(|(id, agent)| {
            let binding = agent.binding.as_ref()?;
            Some((
                id.clone(),
                state::destination_key(&binding.cluster_url, binding.principal_id),
            ))
        })
        .collect()
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
    fn a_session_done_for_one_control_plane_is_read_again_once_reporting_moves() {
        let dir = tempfile::tempdir().unwrap();
        let read_at: DateTime<Utc> = "2026-10-20T10:00:00Z".parse().unwrap();
        let reporting_to =
            |server: &str| BTreeMap::from([("claude".to_string(), server.to_string())]);
        let mut ledger = Ledger::load_at(dir.path()).reporting_to(reporting_to("server-1"));
        ledger.record([("claude", "a", read_at)]).unwrap();

        let moved = Ledger::load_at(dir.path()).reporting_to(reporting_to("server-2"));
        let back = Ledger::load_at(dir.path()).reporting_to(reporting_to("server-1"));

        assert!(!moved.is_done("claude", "a", read_at));
        assert!(back.is_done("claude", "a", read_at));
    }

    #[test]
    fn a_ledger_written_before_destinations_were_kept_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LEDGER_FILE),
            r#"{"claude/a":"2026-10-20T10:00:00Z"}"#,
        )
        .unwrap();
        let read_at: DateTime<Utc> = "2026-10-20T10:00:00Z".parse().unwrap();

        assert!(!Ledger::load_at(dir.path()).is_done("claude", "a", read_at));
    }

    #[test]
    fn an_unreadable_ledger_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEDGER_FILE), "not json").unwrap();
        let at = Utc::now();

        assert!(!Ledger::load_at(dir.path()).is_done("claude", "a", at));
    }
}
