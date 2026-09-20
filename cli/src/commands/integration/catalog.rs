//! Static metadata and lookup for the closed coding-agent catalog.

use std::path::PathBuf;

use super::agents::Agent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Instrumented,
    DetectOnly,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Self::Instrumented => "instrumentable",
            Self::DetectOnly => "detect-only",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AgentSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub binary: &'static str,
    pub agent_name: &'static str,
    pub support: Support,
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn find(id: &str) -> Option<Agent> {
    Agent::ALL
        .iter()
        .copied()
        .find(|agent| agent.spec().id == id)
}

pub fn known_ids() -> String {
    Agent::ALL
        .iter()
        .map(|agent| agent.spec().id)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_known_agent_by_id() {
        assert_eq!(find("claude").unwrap().spec().agent_name, "claude-code");
    }

    #[test]
    fn catalog_ids_are_unique() {
        let mut ids: Vec<_> = Agent::ALL.iter().map(|agent| agent.spec().id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }
}
