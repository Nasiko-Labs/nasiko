//! Shared reporting launcher generation and shell quoting.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::state;

pub const SCRIPT_NAME: &str = "nasiko-session-report.sh";

pub fn script_path(config_path: &Path) -> PathBuf {
    config_path.join("hooks").join(SCRIPT_NAME)
}

pub fn install(config_path: &Path, agent_id: &str, version: u32) -> Result<PathBuf> {
    let path = script_path(config_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(&path, script_body(agent_id, version)?)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(path)
}

pub fn uninstall(config_path: &Path) -> Result<()> {
    let path = script_path(config_path);
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

pub fn installed_version(config_path: &Path) -> Option<u32> {
    version_marker(&std::fs::read_to_string(script_path(config_path)).ok()?)
}

pub fn version_marker(content: &str) -> Option<u32> {
    content
        .lines()
        .find_map(|line| line.split_once("nasiko-hook-version:"))?
        .1
        .trim()
        .parse()
        .ok()
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn script_body(agent_id: &str, version: u32) -> Result<String> {
    let exe = std::env::current_exe()
        .context("cannot determine the path of the running nasiko binary")?;
    let log = state::log_path();
    Ok(format!(
        r#"#!/usr/bin/env bash
# Managed by nasiko - do not edit. nasiko-hook-version: {version}
# Captures this session's completed turns for Nasiko.
mkdir -p {log_dir}
{exe} agents report --agent {agent_id} >>{log} 2>&1 || true
exit 0
"#,
        exe = shell_quote(&exe.to_string_lossy()),
        agent_id = shell_quote(agent_id),
        log = shell_quote(&log.to_string_lossy()),
        log_dir = shell_quote(&log.parent().unwrap_or(Path::new(".")).to_string_lossy()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_spaces_and_apostrophes() {
        assert_eq!(shell_quote("/a b/o'c"), "'/a b/o'\"'\"'c'");
    }

    #[test]
    fn launcher_is_failure_isolated_and_has_a_version() {
        let body = script_body("claude", 3).unwrap();
        assert!(body.contains("nasiko-hook-version: 3"));
        assert!(body.contains("agents report --agent 'claude'"));
        assert!(body.contains("|| true"));
        assert!(body.trim_end().ends_with("exit 0"));
    }
}
