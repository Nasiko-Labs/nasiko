//! Connect Claude Code to the Nasiko LLM router and issue on-demand credentials.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::Client;
use crate::commands::coding_agent_router::{self, AgentSpec, ConnectionBinding, McpMint};

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
    /// Whether the Nasiko MCP gateway was registered with Claude Code's own `claude mcp` store
    /// (settings.json carries no `mcpServers`, so this is the only record of it). Older state
    /// files omit this field; absent means not installed, matching a control plane with no
    /// public gateway URL to register.
    #[serde(default)]
    mcp_installed: bool,
}

/// The end state of this connect's MCP gateway attempt, for the final status print — kept
/// distinct from a plain `Option<String>` so a registration failure (Claude Code rejected the
/// client-side `claude mcp add-json`) gets its own remedy, separate from a mint failure (the
/// control plane itself couldn't issue a credential).
enum McpOutcome {
    Connected,
    MintUnavailable(String),
    RegistrationFailed(String),
}

/// One-time setup. Claude subsequently invokes the hidden credential helper itself.
pub fn connect(agent: Option<&str>, llm_config: Option<&str>) -> Result<()> {
    let claude = which::which("claude")
        .context("Claude Code is not installed or 'claude' is not on PATH")?;
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

    let mcp_client = Client::from_cluster_entry_with_timeout(
        &prepared.entry,
        Some(coding_agent_router::CP_CALL_TIMEOUT),
    );
    let mcp_mint = coding_agent_router::mint_mcp_credential(
        &mcp_client,
        &prepared.binding.agent_id,
        &prepared.entry.url,
    );
    // Registered with Claude Code's own `claude mcp` store before the atomic write below, so
    // `mcp_installed` reflects the real outcome in that one write rather than needing a second
    // write once client-side registration succeeds. A registration failure (a stale "nasiko"
    // entry, an enterprise policy block, ...) is treated exactly like a mint failure: best-effort
    // revoke, record not installed, warn — LLM routing must still connect either way.
    let (mcp_installed, mcp_outcome) = match &mcp_mint {
        McpMint::Minted(credential) => match run_mcp_add(&claude, credential) {
            Ok(()) => (true, McpOutcome::Connected),
            Err(error) => {
                coding_agent_router::revoke_mcp_credential_best_effort(
                    &mcp_client,
                    &prepared.binding.agent_id,
                );
                (
                    false,
                    McpOutcome::RegistrationFailed(coding_agent_router::one_line_warning(&error)),
                )
            }
        },
        McpMint::Unavailable(message) => (false, McpOutcome::MintUnavailable(message.clone())),
    };

    let state = ConnectionState {
        binding: prepared.binding.clone(),
        settings_path: settings_path.clone(),
        helper_command,
        original_env_present,
        original_helper,
        original_base_url,
        mcp_installed,
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
        if mcp_installed {
            // The client-side registration above succeeded, but persisting the result did not;
            // undo the registration too so nothing local is left pointing at the credential the
            // revoke below is about to kill. Every step here is attempted regardless of whether
            // an earlier one failed — the caller ends up with LLM routing rolled back either way,
            // so any partial cleanup miss is worth surfacing rather than swallowing silently.
            if let Err(error) = run_mcp_remove(&claude) {
                eprintln!(
                    "warning: failed to remove the Nasiko MCP server from Claude Code: {}",
                    coding_agent_router::one_line_warning(&error)
                );
            }
            coding_agent_router::revoke_mcp_credential_best_effort(
                &mcp_client,
                &prepared.binding.agent_id,
            );
        }
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
    match mcp_outcome {
        McpOutcome::Connected => {
            println!("{}", coding_agent_router::mcp_connected_line());
            println!("{}", coding_agent_router::mcp_multi_machine_note());
        }
        McpOutcome::MintUnavailable(reason) => {
            eprintln!(
                "warning: MCP gateway unavailable ({reason}); LLM routing is connected, MCP tools are not"
            );
        }
        McpOutcome::RegistrationFailed(reason) => {
            eprintln!(
                "warning: could not register the Nasiko MCP server with Claude Code ({reason}); LLM routing is connected, MCP tools are not. To retry: claude mcp remove --scope user nasiko && nasiko disconnect claude && nasiko connect claude"
            );
        }
    }
    println!("Run `claude` normally. Disconnect with: nasiko disconnect claude");
    Ok(())
}

/// `claude mcp add-json --scope user <name> '<json>'` args: a single JSON argument carries
/// `type`/`url`/`headers` together, so there is no per-flag surface (`--transport`, `--header`,
/// ...) that can drift out of sync with what the server issues. Like any process argument, the
/// credential is visible in the process argument list for the life of this `claude` invocation
/// (e.g. via `ps`) regardless of which flags carry it.
fn mcp_add_command(credential: &coding_agent_router::McpCredential) -> Vec<String> {
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
        coding_agent_router::MCP_SERVER_NAME.to_string(),
        payload,
    ]
}

/// `claude mcp remove --scope user <name>` args — same scope `mcp_add_command` installs into.
fn mcp_remove_command() -> Vec<String> {
    vec![
        "mcp".to_string(),
        "remove".to_string(),
        "--scope".to_string(),
        "user".to_string(),
        coding_agent_router::MCP_SERVER_NAME.to_string(),
    ]
}

fn run_mcp_add(claude: &Path, credential: &coding_agent_router::McpCredential) -> Result<()> {
    let output = Command::new(claude)
        .args(mcp_add_command(credential))
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

/// Removes the Nasiko MCP server registration. Claude Code exits non-zero when the server is
/// already absent (`No MCP server named "nasiko" in user scope`); that specific case is treated
/// as success so disconnect stays idempotent.
fn run_mcp_remove(claude: &Path) -> Result<()> {
    let output = Command::new(claude)
        .args(mcp_remove_command())
        .output()
        .context("failed to run `claude mcp remove`")?;
    if output.status.success() {
        return Ok(());
    }
    if is_not_found_output(&String::from_utf8_lossy(&output.stderr))
        || is_not_found_output(&String::from_utf8_lossy(&output.stdout))
    {
        return Ok(());
    }
    bail!(
        "claude mcp remove failed: {}",
        command_error_detail(&output)
    );
}

/// A `claude mcp` subprocess's error text: stderr when it said anything, else stdout — Claude
/// Code isn't consistent about which stream carries a given failure.
fn command_error_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// True when a `claude mcp remove` failure just means the target was already absent — the
/// specific message Claude Code exits non-zero with in that case. Checked against both streams
/// since which one carries it isn't stable across Claude Code versions.
fn is_not_found_output(output: &str) -> bool {
    output.contains("No MCP server named")
}

/// Best-effort MCP gateway teardown for disconnect: removes the client-side registration, then
/// revokes the server-side credential. Neither failure blocks removing the local Nasiko config —
/// both are reported as warnings.
fn teardown_mcp_gateway(binding: &ConnectionBinding) {
    let removal = (|| -> Result<()> {
        let claude = which::which("claude")
            .context("Claude Code is not installed or 'claude' is not on PATH")?;
        run_mcp_remove(&claude)
    })();
    if let Err(error) = removal {
        eprintln!(
            "warning: failed to remove the Nasiko MCP server from Claude Code: {}",
            coding_agent_router::one_line_warning(&error)
        );
    }
    coding_agent_router::revoke_bound_mcp_credential_best_effort(binding);
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
    if state.mcp_installed {
        teardown_mcp_gateway(&state.binding);
    }
    restore_settings(&state)?;
    fs::remove_file(state_path()).context("failed to remove Claude connection state")?;
    if print {
        println!("Disconnected Claude Code from Nasiko.");
        println!("Restart Claude Code so the restored API settings take effect.");
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
        state.original_env_present,
    )?;
    write_json_atomic(&state.settings_path, &Value::Object(settings))
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
    println!(
        "MCP gateway: {}",
        if state.mcp_installed {
            "configured"
        } else {
            "not configured"
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
        assert!(restored["env"].get("ANTHROPIC_BASE_URL").is_none());
    }

    fn mcp_credential() -> coding_agent_router::McpCredential {
        coding_agent_router::McpCredential {
            token: "ngt_secret".into(),
            gateway_url: "https://cp.example/api/mcp".into(),
            connect_url: "https://cp.example/api/mcp/s/ngt_secret".into(),
        }
    }

    #[test]
    fn mcp_add_command_registers_an_http_server_with_the_bearer_header() {
        let args = mcp_add_command(&mcp_credential());
        assert_eq!(args[0], "mcp");
        assert_eq!(args[1], "add-json");
        assert_eq!(args[2], "--scope");
        assert_eq!(args[3], "user");
        assert_eq!(args[4], "nasiko");
        let payload: Value = serde_json::from_str(&args[5]).unwrap();
        assert_eq!(payload["type"], "http");
        assert_eq!(payload["url"], "https://cp.example/api/mcp");
        assert_eq!(payload["headers"]["Authorization"], "Bearer ngt_secret");
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn mcp_add_command_carries_shell_metacharacters_in_the_token_verbatim() {
        let mut credential = mcp_credential();
        credential.token = "tok'; $(rm -rf /) `echo hi`".to_string();
        let args = mcp_add_command(&credential);
        // A `Vec<String>` passed to `Command::args` reaches the child process as literal argv
        // entries — no shell ever parses this string, so metacharacters must survive untouched.
        let payload: Value = serde_json::from_str(&args[5]).unwrap();
        assert_eq!(
            payload["headers"]["Authorization"],
            "Bearer tok'; $(rm -rf /) `echo hi`"
        );
    }

    #[test]
    fn mcp_remove_command_targets_the_same_scope_and_name() {
        assert_eq!(
            mcp_remove_command(),
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
}
