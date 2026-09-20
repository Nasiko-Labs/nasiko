//! Connect Claude Code to the Nasiko LLM router and issue on-demand credentials.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::Client;
use crate::commands::coding_agent_router::{self, AgentSpec, ConnectionBinding};

const DEFAULT_AGENT_NAME: &str = "claude-code";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SavedValue {
    present: bool,
    #[serde(default)]
    value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionState {
    #[serde(flatten)]
    binding: ConnectionBinding,
    settings_path: PathBuf,
    helper_command: String,
    original_env_present: bool,
    original_helper: SavedValue,
    original_base_url: SavedValue,
}

/// One-time setup. Claude subsequently invokes the hidden credential helper itself.
pub fn connect(agent: Option<&str>, llm_config: Option<&str>) -> Result<()> {
    which::which("claude").context("Claude Code is not installed or 'claude' is not on PATH")?;
    if state_path().exists() {
        bail!(
            "Claude Code is already connected; run `nasiko disconnect claude` before reconnecting"
        );
    }
    let executable = std::env::current_exe().context("cannot locate the nasiko executable")?;
    let prepared = coding_agent_router::prepare(
        AgentSpec {
            id: "claude",
            display_name: "Claude Code",
            default_name: DEFAULT_AGENT_NAME,
        },
        agent,
        llm_config,
        executable,
    )?;

    let settings_path = claude_settings_path();
    let mut settings = read_json_object(&settings_path)?;
    let helper_command = helper_command()?;
    let original_env_present = settings.contains_key("env");
    let original_helper = capture(settings.get("apiKeyHelper"));
    let original_base_url = capture(
        settings
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get("ANTHROPIC_BASE_URL")),
    );
    let env = ensure_env_object(&mut settings)?;
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        Value::String(prepared.entry.url.trim_end_matches('/').to_string()),
    );
    settings.insert("apiKeyHelper".into(), Value::String(helper_command.clone()));

    let state = ConnectionState {
        binding: prepared.binding.clone(),
        settings_path: settings_path.clone(),
        helper_command,
        original_env_present,
        original_helper,
        original_base_url,
    };
    let install_result = (|| -> Result<()> {
        coding_agent_router::atomic_write_json(&state_path(), &state)?;
        if let Err(error) = write_json_atomic(&settings_path, &Value::Object(settings)) {
            let _ = fs::remove_file(state_path());
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = install_result {
        return match coding_agent_router::rollback_config(&prepared) {
            Ok(()) => Err(error),
            Err(rollback) => Err(error.context(format!(
                "local install failed and the prior Nasiko LLM config could not be restored: {rollback:#}"
            ))),
        };
    }

    let provider = prepared
        .resolved_config
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("router");
    let model = prepared
        .resolved_config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("policy-selected");
    println!(
        "Connected Claude Code to Nasiko ({}, {provider}/{model}).",
        state.binding.cluster
    );
    println!("Run `claude` normally. Disconnect with: nasiko disconnect claude");
    Ok(())
}

pub fn disconnect(force: bool) -> Result<()> {
    disconnect_internal(true, force)
}

fn disconnect_internal(print: bool, force: bool) -> Result<()> {
    let Some(state) = load_state()? else {
        if print {
            println!("Claude Code is not connected to Nasiko.");
        }
        return Ok(());
    };
    coding_agent_router::disconnect_preflight("Claude Code", &["claude"], force)?;
    let mut settings = read_json_object(&state.settings_path)?;
    restore_top_level(
        &mut settings,
        "apiKeyHelper",
        &Value::String(state.helper_command.clone()),
        &state.original_helper,
    );
    restore_env(
        &mut settings,
        "ANTHROPIC_BASE_URL",
        &Value::String(state.binding.cluster_url.trim_end_matches('/').to_string()),
        &state.original_base_url,
        state.original_env_present,
    )?;
    write_json_atomic(&state.settings_path, &Value::Object(settings))?;
    fs::remove_file(state_path()).context("failed to remove Claude connection state")?;
    if print {
        println!("Disconnected Claude Code from Nasiko.");
        println!("Restart Claude Code so the restored API settings take effect.");
    }
    Ok(())
}

pub fn status() -> Result<()> {
    let Some(state) = load_state()? else {
        println!("Claude Code is not connected to Nasiko.");
        println!("Connect with: nasiko connect claude");
        return Ok(());
    };
    let settings = read_json_object(&state.settings_path)?;
    let helper_ok = settings.get("apiKeyHelper") == Some(&Value::String(state.helper_command));
    let base_ok = settings
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        == Some(&Value::String(state.binding.cluster_url.clone()));
    let auth = coding_agent_router::auth_status(&state.binding)?;
    println!("Claude Code: connected");
    println!(
        "Cluster:     {} ({})",
        state.binding.cluster, state.binding.cluster_url
    );
    println!("Agent:       {}", state.binding.agent_name);
    println!("Nasiko auth: {auth}");
    println!(
        "Settings:    {}",
        if helper_ok && base_ok {
            "active"
        } else {
            "changed since connect"
        }
    );
    Ok(())
}

/// Hidden `apiKeyHelper` entry point. Stdout must contain only the credential.
pub fn credential() -> Result<()> {
    let state = load_state()?.ok_or_else(|| {
        anyhow::anyhow!("Claude Code is not connected; run: nasiko connect claude")
    })?;
    println!("{}", coding_agent_router::credential(&state.binding)?.token);
    Ok(())
}

/// Explicit one-process mode retained for testing and temporary use.
pub fn run(agent: &str, llm_config: Option<&str>, args: &[String]) -> Result<()> {
    let claude = which::which("claude")
        .context("Claude Code is not installed or 'claude' is not on PATH")?;
    let (_, entry, principal) = coding_agent_router::require_current_login()?;
    let client = Client::from_cluster_entry(&entry);
    let (agent_id, _) =
        coding_agent_router::resolve_owned_agent(&client, agent, &principal, "claude")?;
    coding_agent_router::configure_agent(&client, &agent_id, llm_config)?;
    #[derive(Deserialize)]
    struct Envelope {
        data: coding_agent_router::RoutingCredential,
    }
    let response: Envelope =
        client.post_json(&format!("/agents/{agent_id}/llm-token"), &json!({}))?;
    let status = Command::new(claude)
        .args(args)
        .env(
            "ANTHROPIC_BASE_URL",
            client.base_url().trim_end_matches('/'),
        )
        .env("ANTHROPIC_AUTH_TOKEN", response.data.token)
        .env_remove("ANTHROPIC_API_KEY")
        .status()
        .context("failed to launch Claude Code")?;
    if !status.success() {
        bail!("Claude Code exited with {status}");
    }
    Ok(())
}

fn state_path() -> PathBuf {
    coding_agent_router::state_path("claude")
}

fn claude_settings_path() -> PathBuf {
    claude_settings_path_from(std::env::var_os("CLAUDE_CONFIG_DIR"), home_dir())
}

fn claude_settings_path_from(config_dir: Option<std::ffi::OsString>, home: PathBuf) -> PathBuf {
    config_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
        .join("settings.json")
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn helper_command() -> Result<String> {
    let executable = std::env::current_exe().context("cannot locate the nasiko executable")?;
    Ok(format!("{} __claude-token", shell_quote(&executable)))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn load_state() -> Result<Option<ConnectionState>> {
    let path = state_path();
    if !path.exists() {
        return Ok(None);
    }
    let state: ConnectionState = coding_agent_router::read_json(&path)?.expect("path exists");
    if state.binding.version != coding_agent_router::STATE_VERSION {
        bail!(
            "unsupported Claude connection state version {}",
            state.binding.version
        );
    }
    Ok(Some(state))
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    if path.is_symlink() {
        bail!(
            "refusing to replace symlinked settings file {}",
            path.display()
        );
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Map::new());
    }
    serde_json::from_str::<Value>(&content)?
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))
}

fn ensure_env_object(settings: &mut Map<String, Value>) -> Result<&mut Map<String, Value>> {
    if !settings.contains_key("env") {
        settings.insert("env".into(), Value::Object(Map::new()));
    }
    settings
        .get_mut("env")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("Claude setting 'env' must be a JSON object"))
}

fn capture(value: Option<&Value>) -> SavedValue {
    SavedValue {
        present: value.is_some(),
        value: value.cloned().unwrap_or(Value::Null),
    }
}

fn restore_top_level(
    settings: &mut Map<String, Value>,
    key: &str,
    installed: &Value,
    original: &SavedValue,
) {
    if settings.get(key) != Some(installed) {
        eprintln!("warning: Claude setting '{key}' changed since connect; leaving it unchanged");
        return;
    }
    if original.present {
        settings.insert(key.into(), original.value.clone());
    } else {
        settings.remove(key);
    }
}

fn restore_env(
    settings: &mut Map<String, Value>,
    key: &str,
    installed: &Value,
    original: &SavedValue,
    original_env_present: bool,
) -> Result<()> {
    let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) else {
        eprintln!(
            "warning: Claude setting 'env.{key}' changed since connect; leaving it unchanged"
        );
        return Ok(());
    };
    if env.get(key) != Some(installed) {
        eprintln!(
            "warning: Claude setting 'env.{key}' changed since connect; leaving it unchanged"
        );
        return Ok(());
    }
    if original.present {
        env.insert(key.into(), original.value.clone());
    } else {
        env.remove(key);
    }
    if env.is_empty() && !original_env_present {
        settings.remove("env");
    }
    Ok(())
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    coding_agent_router::atomic_write_json(path, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_version_one_connection_state_remains_compatible() {
        let state: ConnectionState = serde_json::from_value(json!({
            "version": 1,
            "cluster": "local",
            "cluster_url": "http://localhost:8080",
            "agent_id": "agent-id",
            "agent_name": "claude-code",
            "settings_path": "/tmp/settings.json",
            "helper_command": "nasiko __claude-token",
            "original_env_present": false,
            "original_helper": {"present": false},
            "original_base_url": {"present": false}
        }))
        .unwrap();
        assert_eq!(state.binding.version, 1);
        assert!(state.binding.integration_id.is_none());
        assert_eq!(state.binding.cluster, "local");
        assert!(state.binding.principal_id.is_none());
        assert!(state.binding.executable.is_none());
    }

    #[test]
    fn claude_config_dir_overrides_the_default_home_path() {
        assert_eq!(
            claude_settings_path_from(Some("/custom/claude".into()), "/home/me".into()),
            PathBuf::from("/custom/claude/settings.json")
        );
        assert_eq!(
            claude_settings_path_from(None, "/home/me".into()),
            PathBuf::from("/home/me/.claude/settings.json")
        );
    }

    #[test]
    fn restore_preserves_unrelated_settings() {
        let mut settings = json!({
            "theme": "dark",
            "apiKeyHelper": "nasiko helper",
            "env": {"OTHER": "keep", "ANTHROPIC_BASE_URL": "https://nasiko"}
        })
        .as_object()
        .unwrap()
        .clone();
        restore_top_level(
            &mut settings,
            "apiKeyHelper",
            &json!("nasiko helper"),
            &SavedValue {
                present: false,
                value: Value::Null,
            },
        );
        restore_env(
            &mut settings,
            "ANTHROPIC_BASE_URL",
            &json!("https://nasiko"),
            &SavedValue {
                present: false,
                value: Value::Null,
            },
            true,
        )
        .unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["env"]["OTHER"], "keep");
        assert!(!settings.contains_key("apiKeyHelper"));
        assert!(settings["env"].get("ANTHROPIC_BASE_URL").is_none());
    }

    #[test]
    fn restore_keeps_user_changes() {
        let mut settings = json!({"apiKeyHelper": "user replacement"})
            .as_object()
            .unwrap()
            .clone();
        restore_top_level(
            &mut settings,
            "apiKeyHelper",
            &json!("nasiko helper"),
            &SavedValue {
                present: false,
                value: Value::Null,
            },
        );
        assert_eq!(settings["apiKeyHelper"], "user replacement");
    }

    #[test]
    fn shell_quotes_apostrophes() {
        assert_eq!(shell_quote(Path::new("/tmp/a'b")), "'/tmp/a'\\''b'");
    }
}
