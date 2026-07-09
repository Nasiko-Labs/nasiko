use anyhow::{Result, bail};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::api::{AgentRecord, Client, ContainerStatus, UploadedAgent};

#[derive(Debug, Deserialize)]
struct LogLine {
    timestamp: Option<String>,
    level:     Option<String>,
    message:   String,
    source:    Option<String>,
}

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
    println!("{:<28} {:<12} {:<4} ENDPOINT", "NAME", "STATE", "UP");
    for c in &containers {
        let ep = c.endpoint.as_deref().unwrap_or("-");
        println!("{:<28} {:<12} {:<4} {}", c.container_id, c.state, c.replicas_live, ep);
    }
    Ok(())
}

/// Fetch or stream agent logs.
///
/// Without `--follow`: queries the structured logs endpoint for the last `tail` lines.
/// With    `--follow`: opens an SSE stream and prints new lines until Ctrl-C.
pub fn logs(agent: &str, tail: u32, follow: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if follow {
        return stream_logs(agent);
    }

    // One-shot: fetch structured log lines from the observe endpoint.
    match client.get_json::<Vec<LogLine>>(&format!("/observability/agents/{agent}/logs?limit={tail}")) {
        Ok(lines) => {
            print_log_lines(&lines);
            Ok(())
        }
        Err(_) => {
            // Fallback: raw container logs (older /containers API)
            let logs = client.get_text(&format!("/containers/{agent}/logs?tail={tail}"))?;
            print!("{logs}");
            Ok(())
        }
    }
}

/// Print a slice of `LogLine`s with aligned columns (chronological order).
fn print_log_lines(lines: &[LogLine]) {
    // API returns newest-first; reverse for chronological display.
    for l in lines.iter().rev() {
        let ts  = l.timestamp.as_deref().unwrap_or("").get(..23).unwrap_or("");
        let lvl = l.level.as_deref().unwrap_or("INFO");
        let src = l.source.as_deref().unwrap_or("?");
        println!("{ts} {lvl:<5} [{src}] {}", l.message);
    }
}

/// Open an SSE stream from `/api/observability/agents/{agent}/logs/stream` and print
/// each arriving log line to stdout. Blocks until interrupted or server closes.
fn stream_logs(agent: &str) -> Result<()> {
    let (_, entry) = crate::config::active_cluster()?;
    let url = format!("{}/api/observability/agents/{}/logs/stream", entry.url, agent);

    let mut resp = ureq::Agent::new_with_defaults()
        .get(&url)
        .header("Authorization", &format!("Bearer {}", entry.token.as_deref().unwrap_or("")))
        .call()?;

    if resp.status().as_u16() >= 400 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("HTTP {}: {}", resp.status(), body);
    }

    eprintln!("Following logs for '{}' (Ctrl-C to stop)…", agent);

    let reader = BufReader::new(resp.body_mut().as_reader());
    for raw in reader.lines() {
        let raw = raw?;
        if let Some(json) = raw.strip_prefix("data: ")
            && let Ok(l) = serde_json::from_str::<LogLine>(json)
        {
            let ts  = l.timestamp.as_deref().unwrap_or("").get(..23).unwrap_or("");
            let lvl = l.level.as_deref().unwrap_or("INFO");
            let src = l.source.as_deref().unwrap_or("?");
            println!("{ts} {lvl:<5} [{src}] {}", l.message);
        }
    }
    Ok(())
}

pub fn stop(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/stop"))?;
    println!("Stopped: {agent}");
    Ok(())
}

pub fn start(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/start"))?;
    println!("Started: {agent}");
    Ok(())
}

pub fn restart(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/restart"))?;
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
    crate::config::artifact_registry_url().unwrap_or_else(|| PUBLIC_REGISTRY_URL.to_string())
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

/// Resolve a name or UUID into a deployed agent's UUID via the CP registry.
///
/// Used by `nasiko chat --agent <name>` so callers never have to look up and
/// paste an agent's proxy URL by hand.
pub fn resolve_agent_id(name_or_id: &str) -> Result<String> {
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Ok(name_or_id.to_string());
    }

    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/registry/user/agents")?;
    let agents = unwrap_agents(raw)?;

    let matches: Vec<&AgentRecord> =
        agents.iter().filter(|a| a.name.eq_ignore_ascii_case(name_or_id)).collect();

    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => bail!(
            "no agent named '{name_or_id}' found on the active cluster (run `nasiko agents ls`)"
        ),
        many => bail!(
            "multiple agents named '{name_or_id}': {} — use an ID instead",
            many.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
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

    let name = agent_name.unwrap_or_else(|| source_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("agent"));

    let client = Client::from_active_cluster()?;
    println!("Uploading '{}'...", zip_path.display());
    let result = client.upload_agent(
        &zip_path,
        name,
        "latest",
        &[5000],
        &std::collections::HashMap::new(),
    );

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let queued = result?;
    println!("Status: {} | build_id: {} | agent_id: {}", queued.status, queued.build_id, queued.agent_id);
    println!("Waiting for build to complete...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nDeployed: {} ({})", queued.name, queued.image_tag);
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
    use crate::api::urlencode;
    let mut url = format!("{}/v1/search?limit={}", base, limit);
    if let Some(q) = query        { url.push_str(&format!("&q={}", urlencode(q))); }
    if let Some(t) = artifact_type { url.push_str(&format!("&type={}", urlencode(t))); }
    if let Some(f) = framework    { url.push_str(&format!("&framework={}", urlencode(f))); }
    if let Some(t) = tags         { url.push_str(&format!("&tags={}", urlencode(t))); }
    if let Some(o) = owner        { url.push_str(&format!("&owner={}", urlencode(o))); }

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

