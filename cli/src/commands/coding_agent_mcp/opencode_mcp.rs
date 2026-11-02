use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nasiko_utils::term;

use crate::commands::coding_agent_router::{self, MCP_SERVER_NAME, McpCredential};

const PLUGIN: &str = "nasiko-mcp.js";
const MARKER: &str = "nasiko-mcp-version:";
const VERSION: u32 = 1;
const TOKEN_CONST: &str = "const MCP_TOKEN = ";

pub(super) fn plugin_path() -> PathBuf {
    crate::commands::opencode::config_path()
        .join("plugins")
        .join(PLUGIN)
}

pub(super) fn check(path: &Path) -> Result<()> {
    which::which("opencode").context("OpenCode is not installed or 'opencode' is not on PATH")?;
    require_managed_or_absent(path)
}

fn require_managed_or_absent(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let body =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if !is_managed(&body) {
        bail!(
            "{} exists and was not written by Nasiko; move it away, then connect again",
            path.display()
        );
    }
    Ok(())
}

fn is_managed(body: &str) -> bool {
    body.lines()
        .next()
        .is_some_and(|line| line.starts_with("// Managed by nasiko") && line.contains(MARKER))
}

pub(super) fn register(credential: &McpCredential) -> Result<PathBuf> {
    let path = plugin_path();
    check(&path)?;
    coding_agent_router::atomic_write(&path, body(credential).as_bytes())?;
    Ok(path)
}

pub(super) fn unregister(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let body =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if !is_managed(&body) {
        term::print_warning(&format!(
            "{} was replaced since connect; leaving it as it is",
            path.display()
        ));
        return Ok(());
    }
    fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
}

pub(super) fn registered_token(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let body =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(token_in(&body).unwrap_or_default()))
}

fn token_in(body: &str) -> Option<String> {
    let literal = body
        .lines()
        .find_map(|line| line.strip_prefix(TOKEN_CONST))?;
    serde_json::from_str(literal.trim()).ok()
}

fn body(credential: &McpCredential) -> String {
    let url = serde_json::to_string(&credential.gateway_url).expect("serializable URL");
    let token = serde_json::to_string(&credential.token).expect("serializable token");
    format!(
        r#"// Managed by nasiko - do not edit. {MARKER} {VERSION}
const MCP_URL = {url}
{TOKEN_CONST}{token}

export const NasikoMcpGateway = async () => ({{
  config: async (config) => {{
    config.mcp ??= {{}}
    config.mcp["{MCP_SERVER_NAME}"] = {{
      type: "remote",
      url: MCP_URL,
      headers: {{ Authorization: `Bearer ${{MCP_TOKEN}}` }},
      enabled: true,
    }}
  }},
}})
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential() -> McpCredential {
        McpCredential {
            token: "ngt_secret".into(),
            gateway_url: "https://cp.example/api/mcp".into(),
            connect_url: "https://cp.example/api/mcp/s/ngt_secret".into(),
        }
    }

    #[test]
    fn the_plugin_sets_only_the_nasiko_mcp_server() {
        let body = body(&credential());
        assert!(is_managed(&body));
        assert!(body.contains(r#"const MCP_URL = "https://cp.example/api/mcp""#));
        assert!(body.contains(r#"const MCP_TOKEN = "ngt_secret""#));
        assert!(body.contains(r#"config.mcp["nasiko"] = {"#));
        assert!(body.contains(r#"type: "remote""#));
        assert!(body.contains("headers: { Authorization: `Bearer ${MCP_TOKEN}` }"));
        assert!(!body.contains("config.provider"));
        assert!(!body.contains("config.model"));
        assert_eq!(token_in(&body).as_deref(), Some("ngt_secret"));
    }

    #[test]
    fn quotes_and_newlines_in_values_cannot_break_out_of_the_string_literals() {
        let mut credential = credential();
        credential.token = "tok\"\n;process.exit(1)//".into();
        credential.gateway_url = "https://cp/\"x\"".into();
        let body = body(&credential);
        assert_eq!(body.lines().count(), 15);
        assert_eq!(token_in(&body).as_deref(), Some(credential.token.as_str()));
    }

    #[test]
    fn the_plugin_is_valid_javascript_when_node_is_available() {
        let Ok(node) = which::which("node") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.mjs");
        fs::write(&path, body(&credential())).unwrap();
        let status = std::process::Command::new(node)
            .arg("--check")
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn a_file_nasiko_did_not_write_is_never_overwritten_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGIN);
        fs::write(&path, "export const Mine = true\n").unwrap();
        assert!(require_managed_or_absent(&path).is_err());
        unregister(&path).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "export const Mine = true\n"
        );
    }

    #[test]
    fn a_managed_plugin_is_removed_and_a_missing_one_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PLUGIN);
        unregister(&path).unwrap();
        assert!(require_managed_or_absent(&path).is_ok());
        fs::write(&path, body(&credential())).unwrap();
        assert!(require_managed_or_absent(&path).is_ok());
        assert_eq!(
            registered_token(&path).unwrap().as_deref(),
            Some("ngt_secret")
        );
        unregister(&path).unwrap();
        assert!(!path.exists());
        assert!(registered_token(&path).unwrap().is_none());
    }
}
