use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::api::Client;

/// Upload a directory or .zip file to the active cluster.
///
/// The server builds the Docker image and deploys it — no local Docker required.
///
/// Flow:
/// 1. Resolve source: directory (auto-zipped) or .zip file
/// 2. Read name/version from AgentCard.json if not provided via flags
/// 3. POST multipart to /api/agents/upload → 202 Accepted + build_id
/// 4. Stream build status via SSE until success or failure
pub fn upload(
    source: &str,
    name: Option<&str>,
    version: Option<&str>,
    port: u16,
    env_file: Option<&str>,
    env_args: &[String],
) -> Result<()> {
    let source_path = Path::new(source);

    if !source_path.exists() {
        bail!("'{}' does not exist", source);
    }

    let env = parse_env(env_file, env_args)?;

    // ── Resolve name and version ─────────────────────────────────────────────
    let (resolved_name, resolved_version) = resolve_name_version(source_path, name, version)?;

    // ── Zip directory if needed ──────────────────────────────────────────────
    let (zip_path, is_temp) = if source_path.is_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nasiko-upload-{}.zip",
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
        bail!("source must be a directory or a .zip file, got: '{}'", source);
    };

    // ── Upload ───────────────────────────────────────────────────────────────
    let client = Client::from_active_cluster()?;
    println!("Uploading '{}' as {}:{}...", zip_path.display(), resolved_name, resolved_version);

    let result = client.upload_agent(
        &zip_path,
        &resolved_name,
        &resolved_version,
        &[port],
        &env,
    );

    if is_temp {
        let _ = fs::remove_file(&zip_path);
    }

    let queued = result?;
    println!("Status: {} | build_id: {} | agent_id: {}", queued.status, queued.build_id, queued.agent_id);
    println!("Waiting for server to build and deploy... (this may take a few minutes)");

    client.poll_build_status(&queued.build_id)?;

    println!("\nDeployed: {} ({})", queued.name, queued.image_tag);
    Ok(())
}

fn resolve_name_version(
    source: &Path,
    name_flag: Option<&str>,
    version_flag: Option<&str>,
) -> Result<(String, String)> {
    // Try reading AgentCard.json from the directory
    let card = if source.is_dir() {
        let card_path = source.join("AgentCard.json");
        if card_path.exists() {
            fs::read_to_string(&card_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        } else {
            None
        }
    } else {
        None
    };

    let name = name_flag
        .map(String::from)
        .or_else(|| card.as_ref().and_then(|c| c.get("name")?.as_str().map(String::from)))
        .or_else(|| {
            // Fall back to directory/file name without extension
            source.file_stem().and_then(|n| n.to_str()).map(|n| n.replace([' ', '/'], "-"))
        })
        .unwrap_or_else(|| "agent".into());

    if name.is_empty() {
        bail!("agent name is required (use --name or add 'name' to AgentCard.json)");
    }

    let version = version_flag
        .map(String::from)
        .or_else(|| card.as_ref().and_then(|c| c.get("version")?.as_str().map(String::from)))
        .unwrap_or_else(|| "latest".into());

    Ok((name, version))
}

fn parse_env(env_file: Option<&str>, env_args: &[String]) -> Result<HashMap<String, String>> {
    let mut env = HashMap::new();

    if let Some(path) = env_file {
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read env file '{}': {e}", path))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                env.insert(key.trim().to_string(), value.trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }

    for arg in env_args {
        if let Some((key, value)) = arg.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        } else {
            bail!("invalid env format: '{}' (expected KEY=VALUE)", arg);
        }
    }

    Ok(env)
}