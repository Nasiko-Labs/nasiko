use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
use serde_json::{Value, json};

use super::fingerprint;
use crate::commands::coding_agent_router::{self, MCP_SERVER_NAME, McpCredential};

pub(super) fn binary() -> Result<PathBuf> {
    which::which("claude").context("Claude Code is not installed or 'claude' is not on PATH")
}

pub(super) fn check() -> Result<PathBuf> {
    let claude = binary()?;
    if registered_token()?.is_some() {
        bail!(
            "Claude Code already has a user-scope MCP server named \"{MCP_SERVER_NAME}\" that this CLI didn't set up; remove it with `claude mcp remove --scope user {MCP_SERVER_NAME}`, then connect again"
        );
    }
    Ok(claude)
}

pub(super) fn register(credential: &McpCredential) -> Result<()> {
    let claude = check()?;
    let output = Command::new(&claude)
        .args(add_command(credential))
        .output()
        .context("failed to run `claude mcp add-json`")?;
    if !output.status.success() {
        bail!(
            "claude mcp add-json failed: {}",
            command_error_detail(&output)
        );
    }
    Ok(())
}

pub(super) fn unregister(token_sha256: &str) -> Result<()> {
    match registered_token()? {
        None => Ok(()),
        Some(token) if token_sha256.is_empty() || fingerprint(&token) == token_sha256 => {
            remove(&binary()?)
        }
        Some(_) => {
            term::print_warning(&format!(
                "Claude Code's \"{MCP_SERVER_NAME}\" MCP server changed since connect; leaving it as it is"
            ));
            Ok(())
        }
    }
}

pub fn unregister_best_effort() {
    if let Err(error) = binary().and_then(|claude| remove(&claude)) {
        term::print_warning(&format!(
            "failed to remove the Nasiko MCP server from Claude Code: {}",
            coding_agent_router::one_line_warning(&error)
        ));
    }
}

pub(super) fn registered_token() -> Result<Option<String>> {
    registered_token_in(&config_path())
}

fn registered_token_in(path: &Path) -> Result<Option<String>> {
    let Some(config) = coding_agent_router::read_json::<Value>(path)? else {
        return Ok(None);
    };
    let Some(server) = config
        .get("mcpServers")
        .and_then(|servers| servers.get(MCP_SERVER_NAME))
    else {
        return Ok(None);
    };
    let token = server
        .get("headers")
        .and_then(|headers| headers.get("Authorization"))
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    Ok(Some(token.to_string()))
}

fn config_path() -> PathBuf {
    config_path_from(
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

fn config_path_from(config_dir: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
    config_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home)
        .join(".claude.json")
}

fn add_command(credential: &McpCredential) -> Vec<String> {
    let payload = json!({
        "type": "http",
        "url": credential.gateway_url,
        "headers": {"Authorization": format!("Bearer {}", credential.token)},
    })
    .to_string();
    vec![
        "mcp".to_string(),
        "add-json".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        MCP_SERVER_NAME.to_string(),
        payload,
    ]
}

fn remove_command() -> Vec<String> {
    vec![
        "mcp".to_string(),
        "remove".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        MCP_SERVER_NAME.to_string(),
    ]
}

fn remove(claude: &Path) -> Result<()> {
    let output = Command::new(claude)
        .args(remove_command())
        .output()
        .context("failed to run `claude mcp remove`")?;
    if output.status.success()
        || is_not_found_output(&String::from_utf8_lossy(&output.stderr))
        || is_not_found_output(&String::from_utf8_lossy(&output.stdout))
    {
        return Ok(());
    }
    bail!(
        "claude mcp remove failed: {}",
        command_error_detail(&output)
    );
}

fn command_error_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn is_not_found_output(output: &str) -> bool {
    output.contains("No MCP server named")
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
    fn add_command_registers_an_http_server_with_the_bearer_header() {
        let args = add_command(&credential());
        assert_eq!(args[..5], ["mcp", "add-json", "--scope", "user", "nasiko"]);
        let payload: Value = serde_json::from_str(&args[5]).unwrap();
        assert_eq!(payload["type"], "http");
        assert_eq!(payload["url"], "https://cp.example/api/mcp");
        assert_eq!(payload["headers"]["Authorization"], "Bearer ngt_secret");
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn add_command_carries_shell_metacharacters_in_the_token_verbatim() {
        let mut credential = credential();
        credential.token = "tok'; $(rm -rf /) `echo hi`".to_string();
        let payload: Value = serde_json::from_str(&add_command(&credential)[5]).unwrap();
        assert_eq!(
            payload["headers"]["Authorization"],
            "Bearer tok'; $(rm -rf /) `echo hi`"
        );
    }

    #[test]
    fn remove_command_targets_the_same_scope_and_name() {
        assert_eq!(
            remove_command(),
            vec!["mcp", "remove", "--scope", "user", "nasiko"]
        );
    }

    #[test]
    fn is_not_found_output_recognizes_the_idempotent_case() {
        assert!(is_not_found_output(
            "Error: No MCP server named \"nasiko\" in user scope\n"
        ));
        assert!(!is_not_found_output(
            "Error: Cannot remove MCP server: not allowed by enterprise policy\n"
        ));
        assert!(!is_not_found_output(""));
    }

    #[test]
    fn the_user_scope_config_lives_beside_the_config_dir_or_in_home() {
        assert_eq!(
            config_path_from(Some("/custom".into()), "/home/me".into()),
            PathBuf::from("/custom/.claude.json")
        );
        assert_eq!(
            config_path_from(Some("".into()), "/home/me".into()),
            PathBuf::from("/home/me/.claude.json")
        );
        assert_eq!(
            config_path_from(None, "/home/me".into()),
            PathBuf::from("/home/me/.claude.json")
        );
    }

    fn config_with(value: Value) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        (dir, path)
    }

    #[test]
    fn the_registered_token_is_read_from_the_user_scope_server() {
        let (_dir, path) = config_with(json!({
            "projects": {"/repo": {"mcpServers": {"nasiko": {"headers": {"Authorization": "Bearer ngt_project"}}}}},
            "mcpServers": {
                "other": {"type": "stdio"},
                "nasiko": {"type": "http", "url": "https://cp/api/mcp", "headers": {"Authorization": "Bearer ngt_user"}}
            }
        }));
        assert_eq!(
            registered_token_in(&path).unwrap().as_deref(),
            Some("ngt_user")
        );
    }

    #[test]
    fn a_server_without_a_bearer_header_still_counts_as_registered() {
        let (_dir, path) = config_with(json!({"mcpServers": {"nasiko": {"type": "stdio"}}}));
        assert_eq!(registered_token_in(&path).unwrap().as_deref(), Some(""));
    }

    #[test]
    fn no_config_or_no_server_means_nothing_is_registered() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            registered_token_in(&dir.path().join("absent.json"))
                .unwrap()
                .is_none()
        );
        let (_dir, path) = config_with(json!({"mcpServers": {"other": {}}}));
        assert!(registered_token_in(&path).unwrap().is_none());
        let (_dir, path) = config_with(json!({"numStartups": 3}));
        assert!(registered_token_in(&path).unwrap().is_none());
    }

    #[test]
    fn an_unreadable_config_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(registered_token_in(&path).is_err());
    }
}
