use anyhow::{Result, bail};
use std::path::Path;
use std::process::Command;

use crate::api::{AgentRecord, Client, ContainerStatus, UploadedAgent};

// ─── Lifecycle ────────────────────────────────────────────────────────────────

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

pub fn logs(agent: &str, tail: u32) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let logs = client.get_text(&format!("/containers/{agent}/logs?tail={tail}"))?;
    print!("{logs}");
    Ok(())
}

pub fn stop(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let _: serde_json::Value = client.post_empty(&format!("/containers/{agent}/stop"))?;
    println!("Stopped: {agent}");
    Ok(())
}

pub fn restart(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let _: serde_json::Value = client.post_empty(&format!("/containers/{agent}/restart"))?;
    println!("Restarted: {agent}");
    Ok(())
}

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

const PUBLIC_REGISTRY_URL: &str = "https://registry.nasiko.dev";

fn registry_url() -> String {
    std::env::var("NASIKO_REGISTRY_URL").unwrap_or_else(|_| PUBLIC_REGISTRY_URL.to_string())
}

fn unwrap_agents(raw: serde_json::Value) -> Result<Vec<AgentRecord>> {
    if let Some(arr) = raw.as_array() {
        Ok(serde_json::from_value(serde_json::Value::Array(arr.clone()))?)
    } else if let Some(data) = raw.get("data") {
        Ok(serde_json::from_value(data.clone())?)
    } else {
        Ok(serde_json::from_value(raw)?)
    }
}

pub fn cmd_ls() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/registry/user/agents")?;
    let agents = unwrap_agents(raw)?;

    if agents.is_empty() {
        println!("No agents found.");
        return Ok(());
    }

    println!("{:<36} {:<24} {:<10} {:<10} URL", "ID", "NAME", "STATUS", "VERSION");
    println!("{}", "-".repeat(100));
    for a in &agents {
        println!(
            "{:<36} {:<24} {:<10} {:<10} {}",
            a.id,
            a.name,
            a.status.as_deref().unwrap_or("-"),
            a.version.as_deref().unwrap_or("-"),
            a.url.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} agent(s) total.", agents.len());
    Ok(())
}

pub fn cmd_get(agent_id: Option<&str>, name: Option<&str>, format: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = match (agent_id, name) {
        (Some(id), _) => client.get_json(&format!("/registry/agent/id/{}", id))?,
        (None, Some(n)) => client.get_json(&format!("/registry/agent/name/{}", n))?,
        (None, None) => bail!("Provide at least one of --agent-id or --name"),
    };
    let agent: AgentRecord = if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        serde_json::from_value(raw)?
    };

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "id": agent.id, "name": agent.name, "status": agent.status,
            "version": agent.version, "framework": agent.framework,
            "url": agent.url, "description": agent.description, "created_at": agent.created_at,
        }))?);
    } else {
        println!("ID          : {}", agent.id);
        println!("Name        : {}", agent.name);
        println!("Status      : {}", agent.status.as_deref().unwrap_or("-"));
        println!("Version     : {}", agent.version.as_deref().unwrap_or("-"));
        println!("Framework   : {}", agent.framework.as_deref().unwrap_or("-"));
        println!("URL         : {}", agent.url.as_deref().unwrap_or("-"));
        println!("Description : {}", agent.description.as_deref().unwrap_or("-"));
        println!("Created     : {}", agent.created_at.as_deref().unwrap_or("-"));
    }
    Ok(())
}

pub fn cmd_deploy(source: &str, agent_name: Option<&str>) -> Result<()> {
    let source_path = Path::new(source);

    if !source_path.exists() {
        if source.contains('/') && !source.starts_with('/') {
            println!("Detected GitHub repo '{}', cloning and deploying...", source);
            return crate::commands::github::clone(Some(source), None);
        }
        bail!("'{}' does not exist and is not a GitHub owner/repo path.", source);
    }

    let (zip_path, is_temp) = if source_path.is_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nasiko-agent-{}.zip",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        println!("Zipping directory '{}'...", source);
        let status = Command::new("zip")
            .args(["-r", &tmp.to_string_lossy(), "."])
            .current_dir(source_path)
            .status()
            .map_err(|e| anyhow::anyhow!("'zip' not found: {e}. Install with: brew install zip"))?;
        if !status.success() {
            bail!("zip failed for directory '{}'", source);
        }
        (tmp, true)
    } else if source_path.extension().and_then(|e| e.to_str()) == Some("zip") {
        (source_path.to_path_buf(), false)
    } else {
        bail!("source must be a .zip file or a directory, got: '{}'", source);
    };

    let client = Client::from_active_cluster()?;
    println!("Uploading '{}'...", zip_path.display());
    let result = client.upload_zip(&zip_path, agent_name);

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let resp = result?;
    if resp.success {
        println!("Agent '{}' deployed successfully.", resp.agent_name.as_deref().unwrap_or("unknown"));
        if resp.agentcard_generated {
            println!("AgentCard.json generated automatically.");
        }
    } else {
        bail!("Deploy failed: {}", resp.status.as_deref().unwrap_or("unknown error"));
    }
    Ok(())
}

pub fn cmd_search(
    query: Option<&str>,
    artifact_type: Option<&str>,
    framework: Option<&str>,
    tags: Option<&str>,
    owner: Option<&str>,
    limit: usize,
) -> Result<()> {
    let base = registry_url();
    let mut url = format!("{}/v1/search?limit={}", base, limit);
    if let Some(q) = query        { url.push_str(&format!("&q={}", q)); }
    if let Some(t) = artifact_type { url.push_str(&format!("&type={}", t)); }
    if let Some(f) = framework    { url.push_str(&format!("&framework={}", f)); }
    if let Some(t) = tags         { url.push_str(&format!("&tags={}", t)); }
    if let Some(o) = owner        { url.push_str(&format!("&owner={}", o)); }

    let mut resp = ureq::get(&url).call().map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let items = raw.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    if items.is_empty() {
        println!("No artifacts found.");
        return Ok(());
    }

    println!("{:<28} {:<12} {:<10} {:<16} {:<10} TAGS", "NAME", "OWNER", "TYPE", "FRAMEWORK", "VERSION");
    println!("{}", "-".repeat(100));
    for item in &items {
        let tags_str = item.get("tags")
            .and_then(|t| t.as_array())
            .map(|t| t.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        println!(
            "{:<28} {:<12} {:<10} {:<16} {:<10} {}",
            item.get("name").and_then(|v| v.as_str()).unwrap_or("-"),
            item.get("owner").and_then(|v| v.as_str()).unwrap_or("-"),
            item.get("artifact_type").or_else(|| item.get("type")).and_then(|v| v.as_str()).unwrap_or("-"),
            item.get("framework").and_then(|v| v.as_str()).unwrap_or("-"),
            item.get("version").and_then(|v| v.as_str()).unwrap_or("-"),
            tags_str,
        );
    }
    println!("\n{} artifact(s) found.", items.len());
    Ok(())
}

pub fn cmd_info(name: &str, owner: &str, version: Option<&str>) -> Result<()> {
    let base = registry_url();
    let url = match version {
        Some(v) => format!("{}/v1/artifacts/{}/{}/{}", base, owner, name, v),
        None => format!("{}/v1/artifacts/{}/{}", base, owner, name),
    };

    let mut resp = ureq::get(&url).call().map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let a = raw.get("data").cloned().unwrap_or(raw);

    println!("Name        : {}", a.get("name").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("Owner       : {}", a.get("owner").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("Version     : {}", a.get("version").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("Type        : {}", a.get("artifact_type").or_else(|| a.get("type")).and_then(|v| v.as_str()).unwrap_or("-"));
    println!("Framework   : {}", a.get("framework").and_then(|v| v.as_str()).unwrap_or("-"));
    println!("Description : {}", a.get("description").and_then(|v| v.as_str()).unwrap_or("-"));
    let tags_str = a.get("tags").and_then(|t| t.as_array())
        .map(|t| t.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    if !tags_str.is_empty() { println!("Tags        : {}", tags_str); }
    Ok(())
}

pub fn cmd_frameworks() -> Result<()> {
    let base = registry_url();
    let url = format!("{}/v1/meta/frameworks", base);

    let mut resp = ureq::get(&url).call().map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let frameworks = raw.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    if frameworks.is_empty() {
        println!("No frameworks found.");
        return Ok(());
    }
    println!("Available frameworks:");
    for fw in &frameworks {
        if let Some(name) = fw.as_str() { println!("  - {}", name); }
    }
    Ok(())
}

pub fn cmd_list_uploaded() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/user/upload-agents")?;
    let agents: Vec<UploadedAgent> = if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else if let Some(arr) = raw.as_array() {
        serde_json::from_value(serde_json::Value::Array(arr.clone()))?
    } else {
        serde_json::from_value(raw)?
    };

    if agents.is_empty() {
        println!("No uploaded agents found.");
        return Ok(());
    }

    println!("{:<36} {:<24} {:<10} {:<10} URL", "AGENT ID", "NAME", "STATUS", "TYPE");
    println!("{}", "-".repeat(100));
    for a in &agents {
        let (status, utype) = a.upload_info.as_ref()
            .map(|ui| (ui.upload_status.as_deref().unwrap_or("-"), ui.upload_type.as_deref().unwrap_or("-")))
            .unwrap_or(("-", "-"));
        println!(
            "{:<36} {:<24} {:<10} {:<10} {}",
            a.agent_id.as_deref().unwrap_or("-"),
            a.agent_name.as_deref().unwrap_or("-"),
            status, utype,
            a.url.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} agent(s).", agents.len());
    Ok(())
}

