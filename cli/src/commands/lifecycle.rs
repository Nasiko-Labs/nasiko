use std::io::{BufRead, BufReader};

use anyhow::Result;
use serde::Deserialize;

use crate::api::{Client, ContainerStatus};

#[derive(Debug, Deserialize)]
struct LogLine {
    timestamp: Option<String>,
    level:     Option<String>,
    message:   String,
    source:    Option<String>,
}

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

/// Fetch or stream agent logs.
///
/// `agent` can be either an agent name (e.g. `my-agent`) or a UUID.
/// The server resolves the reference so the CLI needs no pre-lookup.
///
/// Without `--follow`: queries the structured logs endpoint for the last `tail` lines.
/// With    `--follow`: opens an SSE stream and prints new lines until Ctrl-C.
pub fn logs(agent: &str, tail: u32, follow: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if follow {
        return stream_logs(agent);
    }

    // One-shot: fetch structured log lines from the observe endpoint.
    match client.get_json::<Vec<LogLine>>(&format!("/observe/agents/{agent}/logs?limit={tail}")) {
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

/// Open an SSE stream from `/api/observe/agents/{agent}/logs/stream` and print
/// each arriving log line to stdout.  Blocks until interrupted or server closes.
fn stream_logs(agent: &str) -> Result<()> {
    let (_, entry) = crate::config::active_cluster()?;
    let url = format!("{}/api/observe/agents/{}/logs/stream", entry.url, agent);

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
        if let Some(json) = raw.strip_prefix("data: ") {
            if let Ok(l) = serde_json::from_str::<LogLine>(json) {
                let ts  = l.timestamp.as_deref().unwrap_or("").get(..23).unwrap_or("");
                let lvl = l.level.as_deref().unwrap_or("INFO");
                let src = l.source.as_deref().unwrap_or("?");
                println!("{ts} {lvl:<5} [{src}] {}", l.message);
            }
        }
    }
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
