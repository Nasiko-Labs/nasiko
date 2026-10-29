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
    /// Whether `connect` minted it an MCP gateway credential, which `disconnect` revokes.
    #[serde(default)]
    pub mcp: bool,
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

    /// The connection whose catalog name is `name`, else the one whose card name it is. `Err`
    /// lists the catalog names when several connected copies share that card name.
    pub fn find(&self, name: &str) -> Result<Option<(Uuid, &Connection)>, Vec<String>> {
        if let Some((id, connection)) = self.agents.iter().find(|(_, c)| c.name == name) {
            return Ok(Some((*id, connection)));
        }
        let mut copies = self.agents.iter().filter(|(_, c)| c.card_name == name);
        match (copies.next(), copies.next()) {
            (None, _) => Ok(None),
            (Some((id, connection)), None) => Ok(Some((*id, connection))),
            _ => Err(self
                .agents
                .values()
                .filter(|c| c.card_name == name)
                .map(|c| c.name.clone())
                .collect()),
        }
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
                mcp: true,
            },
        );

        connections.save_to(&file).unwrap();
        let loaded = Connections::load_from(&file).unwrap();

        assert_eq!(loaded, connections);
        let found = |name: &str| loaded.find(name).map(|found| found.map(|(id, _)| id));
        assert_eq!(found("Crew Researcher"), Ok(Some(id)));
        assert_eq!(found("crew-researcher"), Ok(Some(id)));
        assert_eq!(found("writer"), Ok(None));
    }

    #[test]
    fn copies_sharing_a_card_name_are_found_only_by_their_catalog_names() {
        let mut connections = Connections::default();
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        for (id, name) in [(first, "planner"), (second, "planner-3fa2")] {
            connections.agents.insert(
                id,
                Connection {
                    card_name: "Planner".into(),
                    name: name.into(),
                    file: None,
                    mcp: false,
                },
            );
        }
        let found = |name: &str| connections.find(name).map(|found| found.map(|(id, _)| id));

        assert_eq!(found("planner"), Ok(Some(first)));
        assert_eq!(found("planner-3fa2"), Ok(Some(second)));
        let mut names = found("Planner").unwrap_err();
        names.sort();
        assert_eq!(names, ["planner", "planner-3fa2"]);
    }
}
