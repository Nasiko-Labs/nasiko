//! Control-plane registration for coding-agent integrations.

use anyhow::{Context, Result};
use serde_json::json;

use super::agents::Agent;
use super::state::InstallationBinding;

pub struct Registration {
    pub created: bool,
    pub agent_name: String,
    pub binding: InstallationBinding,
}

pub fn register_agent(agent: Agent) -> Result<Registration> {
    let spec = agent.spec();
    let (cluster_name, entry) = crate::config::active_cluster()?;
    let principal_id = entry
        .token
        .as_deref()
        .and_then(crate::config::token_subject)
        .and_then(|subject| uuid::Uuid::parse_str(&subject).ok())
        .context("active cluster token has no valid user UUID subject")?;
    let client = crate::api::Client::from_cluster_entry(&entry);
    let response: serde_json::Value = client.post_json(
        "/agents/coding-integrations",
        &json!({"integration_id": spec.id}),
    )?;
    let created = response
        .get("created")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let agent_name = response
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("coding-agent registration response is missing its name"))?
        .to_string();
    Ok(Registration {
        created,
        agent_name,
        binding: InstallationBinding {
            cluster_name,
            cluster_url: entry.url,
            principal_id,
        },
    })
}
