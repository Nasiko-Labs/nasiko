//! The A2A agents `connect` set up on this machine: which catalog agent, and the file it wrote
//! the variables into (none when it showed them instead). `status` reads it to tell an agent
//! waiting for its first routed call from one never connected; `disconnect` reads it to undo.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Connected agents keyed by catalog id.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connections {
    #[serde(default)]
    pub agents: BTreeMap<Uuid, Connection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// The card's `name`, as `connect` was given it.
    pub card_name: String,
    /// The catalog name.
    pub name: String,
    /// The file holding Nasiko's block, or `None` when the variables were shown instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl Connections {
    pub fn load() -> Result<Self> {
        Self::load_from(&path())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&path())
    }

    fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// The connection whose card or catalog name is `name`.
    pub fn find(&self, name: &str) -> Option<(Uuid, &Connection)> {
        self.agents
            .iter()
            .find(|(_, connection)| connection.card_name == name || connection.name == name)
            .map(|(id, connection)| (*id, connection))
    }
}

fn path() -> PathBuf {
    super::super::state::integrations_dir().join("a2a-connections.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_round_trip_and_are_found_by_either_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a2a-connections.json");
        let id = Uuid::new_v4();
        let mut connections = Connections::load_from(&file).unwrap();
        connections.agents.insert(
            id,
            Connection {
                card_name: "Crew Researcher".into(),
                name: "crew-researcher".into(),
                file: Some(PathBuf::from("/work/crew/.env")),
            },
        );

        connections.save_to(&file).unwrap();
        let loaded = Connections::load_from(&file).unwrap();

        assert_eq!(loaded, connections);
        assert_eq!(
            loaded.find("Crew Researcher").map(|(found, _)| found),
            Some(id)
        );
        assert_eq!(
            loaded.find("crew-researcher").map(|(found, _)| found),
            Some(id)
        );
        assert!(loaded.find("writer").is_none());
    }
}
