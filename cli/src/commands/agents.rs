use anyhow::{Result, bail};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::{AgentRecord, AgentVersion, Client, DeletedAgent, UpdateQueued, UploadedAgent};

#[derive(Debug, Deserialize)]
struct LogLine {
    timestamp: Option<String>,
    level: Option<String>,
    message: String,
    source: Option<String>,
}

// ─── Lifecycle ────────────────────────────────────────────────────────────────

pub fn ps(json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    if json {
        let raw: serde_json::Value = client.get_json("/agents?limit=100")?;
        println!("{}", serde_json::to_string_pretty(&raw)?);
        return Ok(());
    }
    let agents: Vec<AgentRecord> = client.get_json("/agents?limit=100")?;
    if agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }
    let base = client.base_url().trim_end_matches('/').to_string();

    // Split "created by you" vs "shared with you" (mirrors `nasiko mcp
    // connector list`) — only when we actually know our own id; if the
    // token can't be decoded locally, fall back to one flat list rather
    // than mislabeling everything as "shared".
    match client.current_user_id() {
        Some(my_id) => {
            let (created, shared): (Vec<&AgentRecord>, Vec<&AgentRecord>) = agents
                .iter()
                .partition(|a| a.owner_id.as_deref() == Some(my_id.as_str()));
            if !created.is_empty() {
                println!("Created by you ({}):", created.len());
                print_ps_table(&created, &base);
            }
            if !shared.is_empty() {
                if !created.is_empty() {
                    println!();
                }
                println!("Shared with you ({}):", shared.len());
                print_ps_table(&shared, &base);
            }
        }
        None => print_ps_table(&agents.iter().collect::<Vec<_>>(), &base),
    }
    Ok(())
}

fn print_ps_table(agents: &[&AgentRecord], base: &str) {
    let rows: Vec<PsTableRow> = agents
        .iter()
        .map(|a| {
            let status = a.status.as_deref().unwrap_or("unknown");
            // transport_path comes from the agent's own card (persisted at deploy
            // time); the proxy route requires the agent UUID, not the name.
            let path = a.transport_path.as_deref().unwrap_or("/");
            let url = format!("{base}/api/agents/{}{path}", a.id);
            PsTableRow {
                id: a.id.clone(),
                name: a.name.clone(),
                status: status.to_string(),
                version: a.version.as_deref().unwrap_or("-").to_string(),
                url,
            }
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
}

#[derive(Tabled)]
struct PsTableRow {
    #[tabled(rename = "AGENT ID")]
    id: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "STATUS")]
    status: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "URL")]
    url: String,
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
    match client
        .get_json::<Vec<LogLine>>(&format!("/observability/agents/{agent}/logs?limit={tail}"))
    {
        Ok(lines) => {
            print_log_lines(&lines);
            Ok(())
        }
        Err(_) => {
            // Fallback: raw container logs (older /containers API). The route
            // returns a JSON array of lines, not plain text.
            let lines: Vec<String> =
                client.get_json(&format!("/containers/{agent}/logs?tail={tail}"))?;
            for line in &lines {
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// Print a slice of `LogLine`s with aligned columns (chronological order).
fn print_log_lines(lines: &[LogLine]) {
    // API returns newest-first; reverse for chronological display.
    for l in lines.iter().rev() {
        let ts = l.timestamp.as_deref().unwrap_or("").get(..23).unwrap_or("");
        let lvl = l.level.as_deref().unwrap_or("INFO");
        let src = l.source.as_deref().unwrap_or("?");
        println!("{ts} {lvl:<5} [{src}] {}", l.message);
    }
}

/// Open an SSE stream from `/api/observability/agents/{agent}/logs/stream` and print
/// each arriving log line to stdout. Blocks until interrupted or server closes.
fn stream_logs(agent: &str) -> Result<()> {
    let (_, entry) = crate::config::active_cluster()?;
    let url = format!(
        "{}/api/observability/agents/{}/logs/stream",
        entry.url, agent
    );

    let mut resp = ureq::Agent::new_with_defaults()
        .get(&url)
        .header(
            "Authorization",
            &format!("Bearer {}", entry.token.as_deref().unwrap_or("")),
        )
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
            let ts = l.timestamp.as_deref().unwrap_or("").get(..23).unwrap_or("");
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

pub fn scale(agent: &str, replicas: u32) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_json_void(
        &format!("/containers/{agent}/scale"),
        &serde_json::json!({"replicas": replicas}),
    )?;
    println!("Scaled {agent} to {replicas} replica(s)");
    Ok(())
}

pub fn rm(id: Option<&str>, name: Option<&str>, force: bool) -> Result<()> {
    let (agent_id, label) = match (id, name) {
        (Some(id), None) => {
            if uuid::Uuid::parse_str(id).is_err() {
                bail!(
                    "'{}' is not a valid UUID — use --name to delete by name",
                    id
                );
            }
            (id.to_string(), id.to_string())
        }
        (None, Some(n)) => {
            let resolved = resolve_agent_id(n)?;
            (resolved, n.to_string())
        }
        (None, None) => bail!("provide an agent UUID or use --name <name>"),
        (Some(_), Some(_)) => bail!("provide either an id or --name, not both"),
    };

    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Terminate '{label}' and deregister?"))
            .default(false)
            .interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let client = Client::from_active_cluster()?;
    // `DELETE /agents/{id}` (not `/containers/{agent}`) — it tears down every
    // container for this agent *and* deletes the catalog row. The container-only
    // route left the catalog entry behind, so the agent kept showing up in
    // `ps`/`agents ls` forever after a "successful" removal.
    let result: DeletedAgent = client.delete_json(&format!("/agents/{agent_id}"))?;
    for err in &result.runtime_errors {
        eprintln!("warning: {err}");
    }
    println!(
        "Removed: {label} ({} container(s) stopped)",
        result.containers_stopped
    );
    Ok(())
}

const PUBLIC_REGISTRY_URL: &str = "https://registry.nasiko.dev";

fn registry_url() -> String {
    crate::config::artifact_registry_url().unwrap_or_else(|| PUBLIC_REGISTRY_URL.to_string())
}

fn unwrap_agents(raw: serde_json::Value) -> Result<Vec<AgentRecord>> {
    if let Some(arr) = raw.as_array() {
        Ok(serde_json::from_value(serde_json::Value::Array(
            arr.clone(),
        ))?)
    } else if let Some(data) = raw.get("data") {
        Ok(serde_json::from_value(data.clone())?)
    } else {
        Ok(serde_json::from_value(raw)?)
    }
}

/// Resolve a chat target (URL, "orchestrator", or agent name/id) into a full
/// A2A endpoint URL.
///
/// Used by `nasiko chat -a <target>` so callers can pass a name instead of a
/// full proxy URL.
pub fn resolve_chat_target(target: &str) -> Result<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok(target.to_string());
    }

    // "orchestrator" is not a catalog agent — it's the CP's own routing
    // endpoint (same one `nasiko chat` with no target uses).
    if target.eq_ignore_ascii_case("orchestrator") {
        let base = crate::config::active_url()?;
        return Ok(format!(
            "{}/api/orchestrator/a2a",
            base.trim_end_matches('/')
        ));
    }

    let client = Client::from_active_cluster()?;
    let agents: Vec<AgentRecord> = client.get_json("/agents?limit=100")?;
    let agent = agents
        .iter()
        .find(|a| a.id == target || a.name == target)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no agent found with id or name '{target}' (see `nasiko ps`; \
                 to message the orchestrator instead: nasiko chat \"{target}\" — \
                 any argument with spaces goes to the orchestrator)"
            )
        })?;

    let base = client.base_url().trim_end_matches('/');
    let path = agent.transport_path.as_deref().unwrap_or("/");
    Ok(format!("{}/api/agents/{}{}", base, agent.id, path))
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

    // Page through results — the server clamps `limit` to 100 per request and
    // defaults to the 50 most-recently-created agents if omitted entirely, so a
    // single unpaginated call silently misses any agent past the first page.
    // `q=` pre-filters server-side (name/description ILIKE) to cut down how many
    // pages a typical name needs to page through; the exact match below is what
    // actually decides membership.
    let mut matches: Vec<AgentRecord> = Vec::new();
    let mut offset = 0i64;
    loop {
        let path = format!(
            "/registry/user/agents?q={}&limit=100&offset={offset}",
            crate::api::urlencode(name_or_id)
        );
        let raw: serde_json::Value = client.get_json(&path)?;
        let page = unwrap_agents(raw)?;
        let page_len = page.len();
        matches.extend(page.into_iter().filter(|a| a.name == name_or_id));
        if page_len < 100 {
            break;
        }
        offset += 100;
    }

    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => bail!(
            "no agent named '{name_or_id}' found on the active cluster (run `nasiko agents ls`)"
        ),
        many => bail!(
            "multiple agents named '{name_or_id}': {} — use an ID instead",
            many.iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn cmd_ls() -> Result<()> {
    let client = Client::from_active_cluster()?;
    // GET /agents returns Vec<Agent> directly — field names match AgentRecord.
    // /registry/user/agents returns {data:[{agent_id,...}]} which uses agent_id
    // instead of id, causing deserialization failures.
    let raw: serde_json::Value = client.get_json("/agents?limit=100")?;
    let agents = unwrap_agents(raw)?;

    if agents.is_empty() {
        println!("No agents found.");
        return Ok(());
    }

    println!(
        "{}",
        Table::new(&agents)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} agent(s) total.", agents.len());
    Ok(())
}

pub fn cmd_get(agent_id: Option<&str>, name: Option<&str>, format: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    // GET /agents/{id} accepts either a UUID or an agent name.
    let raw: serde_json::Value = match (agent_id, name) {
        (Some(id), _) => client.get_json(&format!("/agents/{}", id))?,
        (None, Some(n)) => client.get_json(&format!("/agents/{}", n))?,
        (None, None) => bail!("Provide at least one of --agent-id or --name"),
    };
    let agent: AgentRecord = if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        serde_json::from_value(raw)?
    };

    if format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": agent.id, "name": agent.name, "status": agent.status,
                "version": agent.version, "framework": agent.framework,
                "url": agent.url, "description": agent.description, "created_at": agent.created_at,
            }))?
        );
    } else {
        println!("ID          : {}", agent.id);
        println!("Name        : {}", agent.name);
        println!("Status      : {}", agent.status.as_deref().unwrap_or("-"));
        println!("Version     : {}", agent.version.as_deref().unwrap_or("-"));
        println!(
            "Framework   : {}",
            agent.framework.as_deref().unwrap_or("-")
        );
        println!("URL         : {}", agent.url.as_deref().unwrap_or("-"));
        println!(
            "Description : {}",
            agent.description.as_deref().unwrap_or("-")
        );
        println!(
            "Created     : {}",
            agent.created_at.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

pub fn cmd_deploy(source: &str, agent_name: Option<&str>) -> Result<()> {
    let source_path = Path::new(source);

    if !source_path.exists() {
        if source.contains('/') && !source.starts_with('/') {
            println!(
                "Detected GitHub repo '{}', cloning and deploying...",
                source
            );
            return crate::commands::github::clone(Some(source), None);
        }
        bail!(
            "'{}' does not exist and is not a GitHub owner/repo path.",
            source
        );
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
        bail!(
            "source must be a .zip file or a directory, got: '{}'",
            source
        );
    };

    let name = agent_name.unwrap_or_else(|| {
        source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent")
    });

    let client = Client::from_active_cluster()?;
    println!("Uploading '{}'...", zip_path.display());
    let result = client.upload_agent(
        &zip_path,
        name,
        "latest",
        &[8000],
        &std::collections::HashMap::new(),
    );

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let queued = result?;
    println!("Status: {}", queued.data.status);
    if let (Some(build_id), Some(agent_id)) = (&queued.data.build_id, &queued.data.agent_id) {
        println!("build_id: {} | agent_id: {}", build_id, agent_id);
        println!("Waiting for build to complete...");
        client.poll_build_status(build_id)?;
    }
    println!("\nDeployed: {}", queued.data.agent_name);
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
    if let Some(q) = query {
        url.push_str(&format!("&q={}", urlencode(q)));
    }
    if let Some(t) = artifact_type {
        url.push_str(&format!("&type={}", urlencode(t)));
    }
    if let Some(f) = framework {
        url.push_str(&format!("&framework={}", urlencode(f)));
    }
    if let Some(t) = tags {
        url.push_str(&format!("&tags={}", urlencode(t)));
    }
    if let Some(o) = owner {
        url.push_str(&format!("&owner={}", urlencode(o)));
    }

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let items = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        println!("No artifacts found.");
        return Ok(());
    }

    let rows: Vec<SearchTableRow> = items
        .iter()
        .map(|item| {
            let tags_str = item
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|t| {
                    t.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            SearchTableRow {
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                owner: item
                    .get("owner")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                artifact_type: item
                    .get("artifact_type")
                    .or_else(|| item.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                framework: item
                    .get("framework")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                version: item
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                tags: tags_str,
            }
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} artifact(s) found.", items.len());
    Ok(())
}

#[derive(Tabled)]
struct SearchTableRow {
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "OWNER")]
    owner: String,
    #[tabled(rename = "TYPE")]
    artifact_type: String,
    #[tabled(rename = "FRAMEWORK")]
    framework: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "TAGS")]
    tags: String,
}

pub fn cmd_info(name: &str, owner: &str, version: Option<&str>) -> Result<()> {
    let base = registry_url();
    let url = match version {
        Some(v) => format!("{}/v1/artifacts/{}/{}/{}", base, owner, name, v),
        None => format!("{}/v1/artifacts/{}/{}", base, owner, name),
    };

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let a = raw.get("data").cloned().unwrap_or(raw);

    println!(
        "Name        : {}",
        a.get("name").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Owner       : {}",
        a.get("owner").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Version     : {}",
        a.get("version").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Type        : {}",
        a.get("artifact_type")
            .or_else(|| a.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );
    println!(
        "Framework   : {}",
        a.get("framework").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Description : {}",
        a.get("description").and_then(|v| v.as_str()).unwrap_or("-")
    );
    let tags_str = a
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|t| {
            t.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !tags_str.is_empty() {
        println!("Tags        : {}", tags_str);
    }
    Ok(())
}

pub fn cmd_frameworks() -> Result<()> {
    let base = registry_url();
    let url = format!("{}/v1/meta/frameworks", base);

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let frameworks = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    if frameworks.is_empty() {
        println!("No frameworks found.");
        return Ok(());
    }
    println!("Available frameworks:");
    for fw in &frameworks {
        if let Some(name) = fw.as_str() {
            println!("  - {}", name);
        }
    }
    Ok(())
}

pub fn cmd_list_uploaded() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/agents/my-uploads")?;
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

    println!(
        "{}",
        Table::new(&agents)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} agent(s).", agents.len());
    Ok(())
}

// ─── Reupload / Versions / Rollback ──────────────────────────────────────────

/// Resolve the agent ID from positional UUID or --name, shared by reupload/versions/rollback.
fn resolve_id_or_name(id: Option<&str>, name: Option<&str>) -> Result<(String, String)> {
    match (id, name) {
        (Some(id), None) => {
            if uuid::Uuid::parse_str(id).is_err() {
                bail!(
                    "'{}' is not a valid UUID — use --name to look up by name",
                    id
                );
            }
            Ok((id.to_string(), id.to_string()))
        }
        (None, Some(n)) => {
            let resolved = resolve_agent_id(n)?;
            Ok((resolved, n.to_string()))
        }
        (None, None) => bail!("provide an agent UUID or use --name <name>"),
        (Some(_), Some(_)) => bail!("provide either an id or --name, not both"),
    }
}

pub fn reupload(
    id: Option<&str>,
    name: Option<&str>,
    source: &str,
    version: Option<&str>,
    changelog: Option<&str>,
) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;

    let source_path = Path::new(source);
    if !source_path.exists() {
        bail!("'{}' does not exist", source);
    }

    // Resolve version: flag → project files → None (server auto-patches)
    let resolved_version: Option<String> = version
        .map(String::from)
        .or_else(|| crate::util::detect_version_from_source(source_path));

    if let Some(ref v) = resolved_version {
        println!("Version: {v}");
    } else {
        println!("Version: (server will auto-bump patch)");
    }

    // Zip directory if needed
    let (zip_path, is_temp) = if source_path.is_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nasiko-reupload-{}.zip",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        println!("Zipping '{}'...", source_path.display());
        let status = Command::new("zip")
            .args(["-r", &tmp.to_string_lossy(), "."])
            .current_dir(source_path)
            .status()
            .map_err(|e| anyhow::anyhow!("'zip' not found: {e}. Install with: brew install zip"))?;
        if !status.success() {
            bail!("zip failed for '{}'", source);
        }
        (tmp, true)
    } else if source_path.extension().and_then(|e| e.to_str()) == Some("zip") {
        (source_path.to_path_buf(), false)
    } else {
        bail!("source must be a directory or a .zip file");
    };

    let client = Client::from_active_cluster()?;
    let result = client.update_agent(&agent_id, &zip_path, resolved_version.as_deref(), changelog);

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let queued: UpdateQueued = result?;
    println!(
        "Queued: {} → {} (build: {})",
        queued.previous_version, queued.new_version, queued.build_id
    );
    println!("Waiting for server to build and deploy...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nRedeployed: {label} @ {}", queued.new_version);
    Ok(())
}

pub fn versions(id: Option<&str>, name: Option<&str>) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;
    let client = Client::from_active_cluster()?;
    let mut versions: Vec<AgentVersion> =
        client.get_json(&format!("/agents/{agent_id}/versions"))?;
    if versions.is_empty() {
        println!("No versions found for '{label}'.");
        return Ok(());
    }
    // The active version is never a valid rollback target — `nasiko rollback`
    // only ever considers archived rows — so showing a stale `can_rollback`
    // flag (left over from before it was last activated) on the active row
    // reads as "you can roll back" when you can't. Normalize it to false here
    // rather than teach the server or the Tabled layout about this.
    for v in &mut versions {
        if v.is_active {
            v.can_rollback = false;
        }
    }
    println!(
        "{}",
        Table::new(&versions)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} version(s) for '{label}'.", versions.len());
    Ok(())
}

pub fn rollback(
    id: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;

    #[derive(serde::Serialize)]
    struct RollbackRequest<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        target_version: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'a str>,
    }

    #[derive(serde::Deserialize)]
    struct RollbackQueued {
        build_id: String,
        rolled_back_to: String,
        rolled_back_from: String,
    }

    let client = Client::from_active_cluster()?;
    let queued: RollbackQueued = client.post_json(
        &format!("/agents/{agent_id}/rollback"),
        &RollbackRequest {
            target_version: version,
            reason,
        },
    )?;

    println!(
        "Rolling back '{}': {} → {} (build: {})",
        label, queued.rolled_back_from, queued.rolled_back_to, queued.build_id
    );
    println!("Waiting for rollback to complete...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nRolled back: {label} @ {}", queued.rolled_back_to);
    Ok(())
}
