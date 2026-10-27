//! Control-plane registration for coding-agent integrations and discovered A2A agents.

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
    let binding = installation_binding()?;
    let (_, entry) = crate::config::active_cluster()?;
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
        binding,
    })
}

/// The delivery destination for events captured from now on: the active cluster and the
/// logged-in user, whose agents the server checks every event against.
pub fn installation_binding() -> Result<InstallationBinding> {
    let (cluster_name, entry) = crate::config::active_cluster()?;
    let principal_id = entry
        .token
        .as_deref()
        .and_then(crate::config::token_subject)
        .and_then(|subject| uuid::Uuid::parse_str(&subject).ok())
        .context("active cluster token has no valid user UUID subject")?;
    Ok(InstallationBinding {
        cluster_name,
        cluster_url: entry.url,
        principal_id,
    })
}

/// Seconds a registration call from `status` may take; `status` never waits on a slow server.
const DISCOVERY_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One A2A agent as the control plane registered it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RegisteredA2aAgent {
    /// The card's `name`, as reported.
    pub card_name: String,
    pub id: uuid::Uuid,
    /// Its catalog name.
    pub name: String,
    /// When the router last handled an LLM call made with this agent's key.
    pub last_llm_call_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(serde::Deserialize)]
struct RegisteredA2aAgents {
    agents: Vec<RegisteredA2aAgent>,
}

/// Make the active cluster's catalog match the A2A agents running on `machine`: `cards` are
/// every agent with a valid card; the machine's others are marked stopped. `None` when not
/// logged in, so nothing is registered.
pub fn register_discovered(
    machine: &str,
    cards: &[serde_json::Value],
) -> Result<Option<Vec<RegisteredA2aAgent>>> {
    let (_, entry) = crate::config::active_cluster()?;
    if entry.token.is_none() {
        return Ok(None);
    }
    let client =
        crate::api::Client::from_cluster_entry_with_timeout(&entry, Some(DISCOVERY_SYNC_TIMEOUT));
    let agents: Vec<serde_json::Value> = cards.iter().map(|card| json!({"card": card})).collect();
    let response: RegisteredA2aAgents = client.post_json_quiet(
        "/agents/discovered",
        &json!({"machine": machine, "agents": agents}),
    )?;
    Ok(Some(response.agents))
}

#[derive(serde::Deserialize)]
struct KeyEnvelope {
    data: RoutingKey,
}

#[derive(serde::Deserialize)]
struct RoutingKey {
    token: String,
}

/// A registered A2A agent's LLM routing key (the year-long key a deployed agent gets), and
/// the URL of the cluster that issued it.
pub fn routing_key(agent_id: uuid::Uuid) -> Result<(String, String)> {
    let (_, entry) = crate::config::active_cluster()?;
    let client = crate::api::Client::from_cluster_entry(&entry);
    let response: KeyEnvelope =
        client.post_json_quiet(&format!("/agents/{agent_id}/llm-token"), &json!({}))?;
    Ok((response.data.token, entry.url))
}
