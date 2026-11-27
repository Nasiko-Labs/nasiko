//! Connect Claude Code to the Nasiko LLM router and issue on-demand credentials.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::Client;
use crate::commands::coding_agent_router::{
    self, AgentSpec, ConnectionBinding, RoutingOptions, RoutingSummary,
};

const DEFAULT_AGENT_NAME: &str = "claude-code";
/// Claude Code's own default API endpoint — what `disconnect` writes when there was no
/// `ANTHROPIC_BASE_URL` before `connect`.
const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

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
    /// Whether the Nasiko MCP gateway was registered with Claude Code's own `claude mcp` store
    /// (settings.json carries no `mcpServers`, so this is the only record of it). Older state
    /// files omit this field; absent means not installed, matching a control plane with no
    /// public gateway URL to register.
    #[serde(default)]
    mcp_installed: bool,
}

/// One-time setup. Claude subsequently invokes the hidden credential helper itself.
fn agent_spec() -> AgentSpec<'static> {
    AgentSpec {
        id: "claude",
        display_name: "Claude Code",
        default_name: DEFAULT_AGENT_NAME,
    }
}

fn claude_binary() -> Result<PathBuf> {
    which::which("claude").context("Claude Code is not installed or 'claude' is not on PATH")
}

/// What `connect` needs on this machine before it asks the control plane for anything: the
/// `claude` binary, and `settings.json` with an `env` object it can write into.
fn local_requirements() -> Result<(PathBuf, Map<String, Value>, String)> {
    let claude = claude_binary()?;
    let settings = read_json_object(&claude_settings_path())?;
    let helper_command = helper_command()?;
    // Checked on a copy: `connect` records whether `env` existed before it adds one.
    ensure_env_object(&mut settings.clone())?;
    Ok((claude, settings, helper_command))
}

/// Checks everything `connect` would — what it needs on this machine, then the routing agent and
/// the LLM config with the control plane. A reconnect runs it before removing the working
/// connection, so anything that would fail leaves the connection as it is.
/// Changes nothing on this machine. With the control plane it is not entirely side-effect free:
/// a new LLM config is attached to the routing agent for the length of the check and then put
/// back (with its pinned model), and asking for the default agent creates it if it doesn't exist.
pub fn check(options: &RoutingOptions<'_>) -> Result<()> {
    local_requirements()?;
    let executable = std::env::current_exe().context("cannot locate the nasiko executable")?;
    let prepared =
        coding_agent_router::prepare(agent_spec(), options.agent, options.llm_config, executable)?;
    coding_agent_router::put_back_config(&prepared)
}

/// Connects Claude Code, returning the LLM config change it made to its routing agent.
pub fn connect(options: &RoutingOptions<'_>) -> Result<coding_agent_router::ConfigChange> {
    if state_path().exists() {
        bail!(
            "Claude Code is already connected; run `{cli} disconnect claude` before reconnecting",
            cli = crate::commands::local_agents::cli_name()
        );
    }
    // Everything local is read and checked before the control plane is changed, so nothing
    // between `prepare` and the install below can fail without rolling it back.
    let (_, mut settings, helper_command) = local_requirements()?;
    let settings_path = claude_settings_path();
    let executable = std::env::current_exe().context("cannot locate the nasiko executable")?;
    let prepared =
        coding_agent_router::prepare(agent_spec(), options.agent, options.llm_config, executable)?;

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
        mcp_installed: false,
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
        return coding_agent_router::fail_after_prepare(&prepared, "local install failed", error);
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
    term::print_success(&format!(
        "Connected Claude Code to Nasiko ({}, {provider}/{model}).",
        state.binding.cluster
    ));
    println!(
        "Run `claude` normally. Disconnect with: {}",
        term::cyan(&format!(
            "{} disconnect claude",
            crate::commands::local_agents::cli_name()
        ))
    );
    Ok(prepared.config_change())
}

fn teardown_mcp_gateway(binding: &ConnectionBinding) {
    crate::commands::coding_agent_mcp::unregister_claude_best_effort();
    coding_agent_router::revoke_bound_mcp_credential_best_effort(binding);
}

pub fn disconnect(force: bool) -> Result<()> {
    disconnect_internal(true, force)
}

fn disconnect_internal(print: bool, force: bool) -> Result<()> {
    let Some(state) = load_state()? else {
        if print {
            term::print_info("Claude Code is not connected to Nasiko.");
        }
        return Ok(());
    };
    coding_agent_router::disconnect_preflight("Claude Code", &["claude"], force)?;
    if state.mcp_installed {
        teardown_mcp_gateway(&state.binding);
    }
    restore_settings(&state)?;
    coding_agent_router::revoke_bound_routing_tokens_best_effort(&state.binding, "Claude Code");
    fs::remove_file(state_path()).context("failed to remove Claude connection state")?;
    if print {
        term::print_success("Disconnected Claude Code from Nasiko.");
        println!(
            "Open Claude Code sessions switch back with their next message; run /login in any that asks."
        );
    }
    Ok(())
}

/// Restores `settings.json`'s `apiKeyHelper`/`env.ANTHROPIC_BASE_URL` to what `connect` captured
/// before it touched them, warning instead of clobbering if either changed since connect. Used by
/// `disconnect`.
fn restore_settings(state: &ConnectionState) -> Result<()> {
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
    )?;
    write_json_atomic(&state.settings_path, &Value::Object(settings))
}

/// The current connection, if any, for `nasiko status`'s overview.
pub fn summary() -> Result<Option<RoutingSummary>> {
    Ok(
        load_state()?
            .map(|state| RoutingSummary::from_binding(&state.binding, state.mcp_installed)),
    )
}

pub fn legacy_mcp_binding() -> Result<Option<ConnectionBinding>> {
    Ok(load_state()?
        .filter(|state| state.mcp_installed)
        .map(|state| state.binding))
}

pub fn forget_legacy_mcp() -> Result<()> {
    let Some(mut state) = load_state()? else {
        return Ok(());
    };
    if !state.mcp_installed {
        return Ok(());
    }
    state.mcp_installed = false;
    coding_agent_router::atomic_write_json(&state_path(), &state)
}

/// Where `connect` writes Claude Code's routing settings.
pub fn settings_path() -> PathBuf {
    claude_settings_path()
}

pub fn status() -> Result<()> {
    let reporting = crate::commands::integration::reporting_status_for("claude")?;
    let value = coding_agent_router::status_value;
    let Some(state) = load_state()? else {
        println!("Claude Code routing:           {}", value("not connected"));
        println!("Claude Code session reporting: {}", value(&reporting));
        println!(
            "Connect with: {}",
            term::cyan(&format!(
                "{} connect claude",
                crate::commands::local_agents::cli_name()
            ))
        );
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
    println!("Claude Code routing:           {}", value("connected"));
    println!(
        "Cluster:                       {} ({})",
        state.binding.cluster, state.binding.cluster_url
    );
    println!(
        "Agent:                         {}",
        state.binding.agent_name
    );
    println!("Nasiko auth:                   {}", value(auth));
    println!(
        "Settings:                      {}",
        value(if helper_ok && base_ok {
            "active"
        } else {
            "changed since connect"
        })
    );
    println!("Claude Code session reporting: {}", value(&reporting));
    Ok(())
}

/// Hidden `apiKeyHelper` entry point. Stdout must contain only the credential.
pub fn credential() -> Result<()> {
    let state = load_state()?.ok_or_else(|| {
        anyhow::anyhow!(
            "Claude Code is not connected; run: {} connect claude",
            crate::commands::local_agents::cli_name()
        )
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
        term::print_warning(&format!(
            "Claude setting '{key}' changed since connect; leaving it unchanged"
        ));
        return;
    }
    if original.present {
        settings.insert(key.into(), original.value.clone());
    } else {
        settings.remove(key);
    }
}

/// Restores `env.{key}` to its pre-connect value. When there was none, it writes
/// `ANTHROPIC_DEFAULT_BASE_URL` instead of removing the key: a running Claude Code session
/// applies an `env` key it sees set but keeps one that was deleted, so removing it would leave
/// every open session still sending requests to the cluster after the disconnect.
fn restore_env(
    settings: &mut Map<String, Value>,
    key: &str,
    installed: &Value,
    original: &SavedValue,
) -> Result<()> {
    let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) else {
        term::print_warning(&format!(
            "Claude setting 'env.{key}' changed since connect; leaving it unchanged"
        ));
        return Ok(());
    };
    if env.get(key) != Some(installed) {
        term::print_warning(&format!(
            "Claude setting 'env.{key}' changed since connect; leaving it unchanged"
        ));
        return Ok(());
    }
    let restored = if original.present {
        original.value.clone()
    } else {
        Value::String(ANTHROPIC_DEFAULT_BASE_URL.into())
    };
    env.insert(key.into(), restored);
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
        assert!(!state.mcp_installed);
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
        )
        .unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["env"]["OTHER"], "keep");
        assert!(!settings.contains_key("apiKeyHelper"));
        assert_eq!(
            settings["env"]["ANTHROPIC_BASE_URL"],
            ANTHROPIC_DEFAULT_BASE_URL
        );
    }

    #[test]
    fn restore_writes_back_a_url_the_user_had_before_connect() {
        let mut settings = json!({"env": {"ANTHROPIC_BASE_URL": "https://nasiko"}})
            .as_object()
            .unwrap()
            .clone();
        restore_env(
            &mut settings,
            "ANTHROPIC_BASE_URL",
            &json!("https://nasiko"),
            &SavedValue {
                present: true,
                value: json!("https://corp-proxy"),
            },
        )
        .unwrap();
        assert_eq!(settings["env"]["ANTHROPIC_BASE_URL"], "https://corp-proxy");
    }

    /// A running Claude Code session applies an `env` key it sees set, but keeps an `env` key
    /// that was deleted, so `disconnect` overwrites the URL instead of removing it — even when
    /// `connect` created the `env` object itself.
    #[test]
    fn restore_overwrites_the_url_even_when_connect_created_env() {
        let mut settings = json!({"env": {"ANTHROPIC_BASE_URL": "https://nasiko"}})
            .as_object()
            .unwrap()
            .clone();
        restore_env(
            &mut settings,
            "ANTHROPIC_BASE_URL",
            &json!("https://nasiko"),
            &SavedValue {
                present: false,
                value: Value::Null,
            },
        )
        .unwrap();
        assert_eq!(
            settings["env"],
            json!({"ANTHROPIC_BASE_URL": ANTHROPIC_DEFAULT_BASE_URL})
        );
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

    fn state_for(settings_path: PathBuf) -> ConnectionState {
        ConnectionState {
            binding: ConnectionBinding {
                version: coding_agent_router::STATE_VERSION,
                integration_id: Some("claude".into()),
                cluster: "local".into(),
                cluster_url: "https://nasiko".into(),
                principal_id: None,
                agent_id: "agent".into(),
                agent_name: "claude-code".into(),
                executable: None,
            },
            settings_path,
            helper_command: "nasiko helper".into(),
            original_env_present: true,
            original_helper: SavedValue {
                present: false,
                value: Value::Null,
            },
            original_base_url: SavedValue {
                present: false,
                value: Value::Null,
            },
            mcp_installed: true,
        }
    }

    /// Covers `restore_settings`, used by `disconnect`: the shell-out to `claude mcp
    /// remove`/`add-json` itself stays untested, since it would touch the real Claude Code
    /// installation.
    #[test]
    fn restore_settings_writes_back_the_captured_values() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(
            &settings_path,
            serde_json::to_vec(&json!({
                "apiKeyHelper": "nasiko helper",
                "env": {"ANTHROPIC_BASE_URL": "https://nasiko", "OTHER": "keep"}
            }))
            .unwrap(),
        )
        .unwrap();
        restore_settings(&state_for(settings_path.clone())).unwrap();
        let restored: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
        assert!(!restored.as_object().unwrap().contains_key("apiKeyHelper"));
        assert_eq!(restored["env"]["OTHER"], "keep");
        assert_eq!(
            restored["env"]["ANTHROPIC_BASE_URL"],
            ANTHROPIC_DEFAULT_BASE_URL
        );
    }
}
