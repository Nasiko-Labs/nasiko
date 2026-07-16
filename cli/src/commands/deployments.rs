use anyhow::Result;

use crate::api::{Client, DeploymentRecord};

use super::agents::resolve_agent_id;

/// List all deployments (superusers see every deployment; others see only
/// their own). Distinct from `nasiko ps`, which lists live containers —
/// this shows the deployment history/bookkeeping rows (crash reason,
/// restart count) that back them.
pub fn ls() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let deployments: Vec<DeploymentRecord> = client.get_json("/agents/deployments")?;

    if deployments.is_empty() {
        println!("No deployments found.");
        return Ok(());
    }

    println!(
        "{:<36} {:<24} {:<10} {:<9} {:<26} CRASH",
        "DEPLOYMENT ID", "AGENT", "STATUS", "RESTARTS", "CREATED"
    );
    println!("{}", "-".repeat(115));
    for d in &deployments {
        println!(
            "{:<36} {:<24} {:<10} {:<9} {:<26} {}",
            d.id,
            d.agent_name.as_deref().unwrap_or("-"),
            d.status,
            d.restart_count,
            d.created_at,
            d.crash_reason.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} deployment(s) total.", deployments.len());
    Ok(())
}

/// Show the current (non-stopped) deployment for an agent.
pub fn get(agent: &str) -> Result<()> {
    let id = resolve_agent_id(agent)?;
    let client = Client::from_active_cluster()?;
    let d: DeploymentRecord = client.get_json(&format!("/agents/{id}/deployment"))?;

    println!("Deployment ID:  {}", d.id);
    println!("Agent:          {}", d.agent_name.as_deref().unwrap_or(agent));
    println!("Status:         {}", d.status);
    println!("Replicas:       {}", d.replicas);
    println!("Created:        {}", d.created_at);
    if let Some(url) = &d.service_url {
        println!("Service URL:    {url}");
    }
    println!("Restart count:  {}", d.restart_count);
    if let Some(reason) = &d.crash_reason {
        println!("Crash reason:   {reason}");
    }
    if let Some(at) = &d.crashed_at {
        println!("Crashed at:     {at}");
    }
    Ok(())
}

/// Pull `deployment_id` and `warnings` out of a restart response body. Pure —
/// split out from `restart()` so the parsing/filtering (missing fields,
/// non-string warning entries) is unit-testable without a live server.
fn parse_restart_response(resp: &serde_json::Value) -> (Option<String>, Vec<String>) {
    let new_id = resp.get("deployment_id").and_then(|v| v.as_str()).map(String::from);
    let warnings = resp
        .get("warnings")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|w| w.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (new_id, warnings)
}

/// Restart a specific deployment by its deployment ID (destroy + recreate on
/// Docker, or scale-to-1 on K8s). Distinct from the container-level `nasiko
/// restart`, which targets a container by agent name/id instead of a
/// deployment row — use `nasiko deployments ls` to find the deployment ID.
pub fn restart(deployment_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: serde_json::Value = client.post_json(
        &format!("/agents/deployment/{deployment_id}/restart"),
        &serde_json::json!({}),
    )?;
    println!("Restarted deployment: {deployment_id}");
    let (new_id, warnings) = parse_restart_response(&resp);
    if let Some(new_id) = new_id {
        println!("New deployment ID: {new_id}");
    }
    for w in warnings {
        println!("Warning: {w}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_restart_response_extracts_both_fields() {
        let resp = serde_json::json!({
            "deployment_id": "11111111-1111-1111-1111-111111111111",
            "warnings": ["bookkeeping write failed"],
        });
        let (id, warnings) = parse_restart_response(&resp);
        assert_eq!(id.as_deref(), Some("11111111-1111-1111-1111-111111111111"));
        assert_eq!(warnings, vec!["bookkeeping write failed".to_string()]);
    }

    #[test]
    fn parse_restart_response_handles_empty_object() {
        let (id, warnings) = parse_restart_response(&serde_json::json!({}));
        assert_eq!(id, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn parse_restart_response_filters_non_string_warnings() {
        let resp = serde_json::json!({"warnings": ["ok", 42, null, "also ok"]});
        let (_, warnings) = parse_restart_response(&resp);
        assert_eq!(warnings, vec!["ok".to_string(), "also ok".to_string()]);
    }
}
