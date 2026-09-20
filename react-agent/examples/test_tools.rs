//! Minimal example exercising the public API of `nasiko-react-agent`.
//!
//! Provides the file for the `[[example]] test_tools` target declared in
//! `Cargo.toml` (the original dev scratch was removed), so
//! `cargo check --all-targets` / `cargo build --examples` resolve cleanly.
//!
//! Run with: `cargo run -p nasiko-react-agent --example test_tools`

use nasiko_react_agent::{AgentSkill, OrchestratorConfig};

fn main() {
    // Build a sample skill the way the registry/orchestrator consumes them.
    let skill = AgentSkill {
        id: "echo".to_string(),
        name: "Echo".to_string(),
        description: "Echoes the input back to the caller.".to_string(),
        tags: vec!["demo".to_string(), "text".to_string()],
    };

    // Default orchestrator configuration.
    let config = OrchestratorConfig::default();

    println!("skill: {} ({}) tags={:?}", skill.name, skill.id, skill.tags);
    println!("orchestrator config: {config:?}");
}
