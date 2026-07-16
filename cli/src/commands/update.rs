use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::api::Client;

use super::agents::resolve_agent_id;

/// How a `--source` argument to `nasiko update` should be handled. Pure
/// classification of a filesystem path — split out from `update()` so it's
/// unit-testable without shelling out to `zip` or touching the network.
#[derive(Debug, PartialEq)]
enum SourceKind {
    /// A directory — must be zipped before uploading.
    Directory(PathBuf),
    /// Already a .zip file — used as-is.
    Zip(PathBuf),
}

fn classify_source(src: &str) -> Result<SourceKind> {
    let path = Path::new(src);
    if !path.exists() {
        bail!("'{}' does not exist", src);
    }
    if path.is_dir() {
        Ok(SourceKind::Directory(path.to_path_buf()))
    } else if path.extension().and_then(|e| e.to_str()) == Some("zip") {
        Ok(SourceKind::Zip(path.to_path_buf()))
    } else {
        bail!("source must be a directory or a .zip file, got: '{}'", src);
    }
}

/// Update an agent to a new version: rebuild from new source (directory or
/// .zip) or, if no source is given, re-deploy from the agent's recorded
/// GitHub source with just a version/changelog bump.
pub fn update(agent: &str, source: Option<&str>, version: Option<&str>, changelog: Option<&str>) -> Result<()> {
    let id = resolve_agent_id(agent)?;

    let (zip_path, is_temp): (Option<PathBuf>, bool) = match source {
        Some(src) => match classify_source(src)? {
            SourceKind::Zip(path) => (Some(path), false),
            SourceKind::Directory(dir) => {
                let tmp = std::env::temp_dir().join(format!(
                    "nasiko-update-{}.zip",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                ));
                println!("Zipping '{}'...", dir.display());
                let status = Command::new("zip")
                    .args(["-r", &tmp.to_string_lossy(), "."])
                    .current_dir(&dir)
                    .status()
                    .map_err(|e| anyhow::anyhow!("'zip' not found: {e}. Install with: brew install zip"))?;
                if !status.success() {
                    bail!("zip failed for '{}'", src);
                }
                (Some(tmp), true)
            }
        },
        None => (None, false),
    };

    let client = Client::from_active_cluster()?;
    println!("Updating agent '{agent}'...");
    let result = client.update_agent(&id, zip_path.as_deref(), version, changelog);

    if is_temp && let Some(ref p) = zip_path {
        let _ = fs::remove_file(p);
    }

    let queued = result?;
    println!(
        "Status: {} | build_id: {} | {} → {}",
        queued.status, queued.build_id, queued.previous_version, queued.new_version
    );
    println!("Waiting for server to build and deploy... (this may take a few minutes)");
    client.poll_build_status(&queued.build_id)?;
    println!("\nUpdated: {agent} → {}", queued.new_version);
    Ok(())
}

/// Roll back an agent to a previous version.
pub fn rollback(agent: &str, version: Option<&str>, reason: Option<&str>, yes: bool) -> Result<()> {
    let id = resolve_agent_id(agent)?;

    if !yes {
        let prompt = match version {
            Some(v) => format!("Roll back '{agent}' to version {v}?"),
            None => format!("Roll back '{agent}' to its most recent rollback-eligible version?"),
        };
        let confirm = dialoguer::Confirm::new().with_prompt(prompt).default(false).interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let client = Client::from_active_cluster()?;
    let body = serde_json::json!({
        "target_version": version,
        "reason": reason,
    });
    let queued: crate::api::RollbackQueued =
        client.post_json(&format!("/agents/{id}/rollback"), &body)?;
    println!(
        "Status: {} | build_id: {} | {} → {}",
        queued.status, queued.build_id, queued.rolled_back_from, queued.rolled_back_to
    );
    println!("Waiting for rollback to complete...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nRolled back: {agent} → {}", queued.rolled_back_to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_source_missing_path_errors() {
        let err = classify_source("/nonexistent/nasiko-update-test-path").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn classify_source_directory_is_classified_as_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let kind = classify_source(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(kind, SourceKind::Directory(dir.path().to_path_buf()));
    }

    #[test]
    fn classify_source_zip_file_is_classified_as_zip() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("agent.zip");
        std::fs::write(&zip_path, b"fake zip contents").unwrap();
        let kind = classify_source(zip_path.to_str().unwrap()).unwrap();
        assert_eq!(kind, SourceKind::Zip(zip_path));
    }

    #[test]
    fn classify_source_rejects_non_zip_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let tarball = dir.path().join("agent.tar.gz");
        std::fs::write(&tarball, b"fake tarball").unwrap();
        let err = classify_source(tarball.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("directory or a .zip file"));
    }
}
