use anyhow::Result;

use crate::api::{Client, ContainerStatus};

/// List running agents.
pub fn ps(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if json {
        let raw: serde_json::Value = client.get_json("/containers")?;
        println!("{}", serde_json::to_string_pretty(&raw)?);
        return Ok(());
    }

    let containers: Vec<ContainerStatus> = client.get_json("/containers")?;

    if containers.is_empty() {
        println!("No agents running.");
        return Ok(());
    }

    println!("{:<24} {:<10} {:<40} {:<6} NODE", "NAME", "STATE", "IMAGE", "PORT");
    for c in &containers {
        println!("{:<24} {:<10} {:<40} {:<6} {}", c.name, c.state, c.image, c.port, c.node_id);
    }
    Ok(())
}

/// Stream agent container logs.
pub fn logs(agent: &str, tail: u32) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let logs = client.get_text(&format!("/containers/{agent}/logs?tail={tail}"))?;
    print!("{logs}");
    Ok(())
}

/// Stop an agent container.
pub fn stop(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let _: serde_json::Value = client.post_empty(&format!("/containers/{agent}/stop"))?;
    println!("Stopped: {agent}");
    Ok(())
}

/// Restart an agent container.
pub fn restart(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let _: serde_json::Value = client.post_empty(&format!("/containers/{agent}/restart"))?;
    println!("Restarted: {agent}");
    Ok(())
}

/// Remove (terminate + deregister) an agent.
pub fn rm(agent: &str, force: bool) -> Result<()> {
    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Terminate '{agent}' and deregister?"))
            .default(false)
            .interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let client = Client::from_active_cluster()?;
    client.delete(&format!("/containers/{agent}"))?;
    println!("Removed: {agent}");
    Ok(())
}
