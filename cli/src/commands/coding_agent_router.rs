//! Shared control-plane and state primitives for local coding-agent LLM routing.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::Client;
use crate::commands::llm_config::fetch_config_by_ref;
use crate::config::{self, ClusterEntry, Config};

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionBinding {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<String>,
    pub cluster: String,
    pub cluster_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    pub agent_id: String,
    pub agent_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AgentSpec<'a> {
    pub id: &'a str,
    pub display_name: &'a str,
    pub default_name: &'a str,
}

#[derive(Debug, Clone)]
pub struct PreparedConnection {
    pub binding: ConnectionBinding,
    pub resolved_config: Value,
    pub entry: ClusterEntry,
    previous_llm_config_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingCredential {
    pub token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub fn disconnect_preflight(display_name: &str, process_names: &[&str], force: bool) -> Result<()> {
    let running = running_processes(process_names);
    enforce_disconnect_preflight(display_name, &running, force)
}

fn enforce_disconnect_preflight(display_name: &str, running: &[String], force: bool) -> Result<()> {
    if running.is_empty() {
        return Ok(());
    }
    let names = running.join(", ");
    if force {
        eprintln!(
            "warning: disconnecting while {display_name} is running ({names}); open sessions may fail API requests until restarted"
        );
        return Ok(());
    }
    bail!(
        "{display_name} is still running ({names}). Close all {display_name} sessions, then rerun this command. To disconnect immediately anyway, use `--force`; open sessions may fail API requests."
    )
}

#[cfg(unix)]
fn running_processes(process_names: &[&str]) -> Vec<String> {
    process_names
        .iter()
        .filter(|name| {
            Command::new("pgrep")
                .args(["-x", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .map(|name| (*name).to_string())
        .collect()
}

#[cfg(windows)]
fn running_processes(process_names: &[&str]) -> Vec<String> {
    process_names
        .iter()
        .filter(|name| {
            let image = format!("{name}.exe");
            Command::new("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"])
                .output()
                .is_ok_and(|output| {
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout)
                            .to_ascii_lowercase()
                            .contains(&image.to_ascii_lowercase())
                })
        })
        .map(|name| (*name).to_string())
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn running_processes(_process_names: &[&str]) -> Vec<String> {
    Vec::new()
}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

pub fn prepare(
    spec: AgentSpec<'_>,
    agent_reference: Option<&str>,
    llm_config: Option<&str>,
    executable: PathBuf,
) -> Result<PreparedConnection> {
    let (cluster, entry, principal_id) = require_current_login()?;
    let client = Client::from_cluster_entry(&entry);
    let (agent_id, agent_name) = match agent_reference {
        Some(reference) => resolve_owned_agent(&client, reference, &principal_id, spec.id)?,
        None => ensure_local_agent(&client, &spec, &principal_id)?,
    };
    let (resolved_config, previous_llm_config_id) =
        configure_agent_for_install(&client, &agent_id, llm_config)?;
    Ok(PreparedConnection {
        binding: ConnectionBinding {
            version: STATE_VERSION,
            integration_id: Some(spec.id.to_string()),
            cluster,
            cluster_url: entry.url.clone(),
            principal_id: Some(principal_id),
            agent_id,
            agent_name,
            executable: Some(executable),
        },
        resolved_config,
        entry,
        previous_llm_config_id,
    })
}

pub fn require_current_login() -> Result<(String, ClusterEntry, String)> {
    let (cluster, entry) = config::active_cluster()?;
    let token = usable_login_token(&entry)?;
    let principal = config::token_subject(token)
        .ok_or_else(|| anyhow::anyhow!("invalid Nasiko login; run: nasiko auth login"))?;
    Ok((cluster, entry, principal))
}

pub fn account_scoped_agent_name(
    client: &Client,
    entry: &ClusterEntry,
    base_name: &str,
) -> Result<String> {
    let username = authenticated_account_username(client, entry)?;
    account_scoped_agent_name_for_username(&username, base_name)
}

pub fn authenticated_account_username(client: &Client, entry: &ClusterEntry) -> Result<String> {
    // Access-key based connections may store the access identifier in the
    // config's `username` field. The authenticated server profile is the
    // authority for account-scoped agent names; config is only a compatibility
    // fallback for older control planes without `/users/me`.
    let profile: Option<Value> = client.get_json("/users/me").ok();
    authoritative_account_username(profile.as_ref(), entry.username.as_deref())
        .context("Nasiko account profile is missing a username")
}

/// The account's real email, for human-facing agent labels (`register_agent`'s
/// `display_name`) — distinct from `authenticated_account_username`, whose
/// slug feeds `name` and must stay filesystem/DNS-safe. Falls back to the
/// configured username (not an email) only when the profile can't be reached,
/// same compatibility path `authenticated_account_username` takes.
pub fn authenticated_account_email(client: &Client, entry: &ClusterEntry) -> Result<String> {
    let profile: Option<Value> = client.get_json("/users/me").ok();
    authoritative_account_field(profile.as_ref(), "email", entry.username.as_deref())
        .context("Nasiko account profile is missing an email")
}

pub fn account_scoped_agent_name_for_username(username: &str, base_name: &str) -> Result<String> {
    let username = normalize_agent_name_part(username);
    if username.is_empty() {
        bail!("Nasiko account username cannot be used in an agent name");
    }
    Ok(format!("{username}-{base_name}"))
}

fn authoritative_account_username(
    profile: Option<&Value>,
    configured_username: Option<&str>,
) -> Option<String> {
    authoritative_account_field(profile, "username", configured_username)
}

/// Reads `field` off the authenticated `/users/me` profile (flat or
/// `{"data": {...}}`-enveloped), falling back to `configured_fallback` when the
/// profile is unavailable or the field is blank.
fn authoritative_account_field(
    profile: Option<&Value>,
    field: &str,
    configured_fallback: Option<&str>,
) -> Option<String> {
    profile
        .and_then(|profile| {
            profile
                .get(field)
                .or_else(|| profile.get("data").and_then(|data| data.get(field)))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            configured_fallback
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_string)
}

fn normalize_agent_name_part(value: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            separator = false;
        } else if !separator && !normalized.is_empty() {
            normalized.push('-');
            separator = true;
        }
    }
    normalized.trim_end_matches('-').to_string()
}

fn usable_login_token(entry: &ClusterEntry) -> Result<&str> {
    let token = entry
        .token
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("not logged in to Nasiko; run: nasiko auth login"))?;
    if config::token_expired(token) == Some(true) {
        bail!("Nasiko session expired; run: nasiko auth login");
    }
    Ok(token)
}

pub fn resolve_owned_agent(
    client: &Client,
    reference: &str,
    principal_id: &str,
    integration_id: &str,
) -> Result<(String, String)> {
    let agent = client
        .get_agent(reference)?
        .ok_or_else(|| anyhow::anyhow!("agent '{reference}' not found"))?;
    if agent
        .get("coding_agent_integration_id")
        .and_then(Value::as_str)
        != Some(integration_id)
    {
        bail!("agent '{reference}' is not the {integration_id} coding-agent integration");
    }
    owned_agent_fields(&agent, principal_id, reference)
}

fn owned_agent_fields(
    agent: &Value,
    principal_id: &str,
    fallback_name: &str,
) -> Result<(String, String)> {
    let owner = agent.get("owner_id").and_then(Value::as_str);
    if owner != Some(principal_id) {
        bail!("agent '{fallback_name}' is not owned by the current Nasiko user");
    }
    let id = agent
        .get("id")
        .and_then(Value::as_str)
        .context("routing agent is missing an id")?;
    let name = agent
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    Ok((id.to_string(), name.to_string()))
}

pub fn ensure_local_agent(
    client: &Client,
    spec: &AgentSpec<'_>,
    principal_id: &str,
) -> Result<(String, String)> {
    let agent: Value = client.post_json(
        "/agents/coding-integrations",
        &json!({"integration_id": spec.id}),
    )?;
    owned_agent_fields(&agent, principal_id, spec.default_name)
}

pub fn configure_agent(client: &Client, agent_id: &str, llm_config: Option<&str>) -> Result<Value> {
    configure_agent_for_install(client, agent_id, llm_config).map(|(config, _)| config)
}

fn configure_agent_for_install(
    client: &Client,
    agent_id: &str,
    llm_config: Option<&str>,
) -> Result<(Value, Option<Option<String>>)> {
    let path = format!("/agents/{agent_id}/llm-config");
    let current: Value = client.get_json(&path)?;
    let current_id = current
        .get("data")
        .and_then(|data| data.get("llm_config_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (response, previous) = if let Some(reference) = llm_config {
        let config = fetch_config_by_ref(client, reference)?;
        let config_id = config
            .get("id")
            .and_then(Value::as_str)
            .context("LLM config is missing an id")?;
        if current_id.as_deref() == Some(config_id) {
            (current, None)
        } else {
            (
                client.patch_json(&path, &json!({"llm_config_id": config_id}))?,
                Some(current_id),
            )
        }
    } else {
        (current, None)
    };
    let resolved = response
        .get("data")
        .and_then(|data| data.get("llm_config"))
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no Nasiko LLM config is available; create a default config or pass --config <name>"
            )
        })?;
    Ok((resolved, previous))
}

pub fn rollback_config(prepared: &PreparedConnection) -> Result<()> {
    let Some(previous) = &prepared.previous_llm_config_id else {
        return Ok(());
    };
    let client = Client::from_cluster_entry_with_timeout(
        &prepared.entry,
        Some(std::time::Duration::from_secs(10)),
    );
    let path = format!("/agents/{}/llm-config", prepared.binding.agent_id);
    let _: Value = client.patch_json(&path, &json!({"llm_config_id": previous}))?;
    Ok(())
}

pub fn credential(binding: &ConnectionBinding) -> Result<RoutingCredential> {
    credential_from_config(binding, &config::load()?)
}

fn credential_from_config(binding: &ConnectionBinding, cfg: &Config) -> Result<RoutingCredential> {
    let entry = cfg
        .clusters
        .get(&binding.cluster)
        .ok_or_else(|| anyhow::anyhow!("Nasiko cluster '{}' no longer exists", binding.cluster))?;
    if normalize_url(&entry.url) != normalize_url(&binding.cluster_url) {
        bail!(
            "connected Nasiko cluster URL changed; run: nasiko connect {}",
            binding.integration_id.as_deref().unwrap_or("claude")
        );
    }
    let token = usable_login_token(entry)?;
    let principal = config::token_subject(token)
        .ok_or_else(|| anyhow::anyhow!("invalid Nasiko login; run: nasiko auth login"))?;
    if binding
        .principal_id
        .as_deref()
        .is_some_and(|expected| expected != principal)
    {
        bail!("Nasiko login changed since connect; reconnect this coding agent");
    }
    let client =
        Client::from_cluster_entry_with_timeout(entry, Some(std::time::Duration::from_secs(10)));
    let response: Envelope<RoutingCredential> = client.post_json_quiet(
        &format!("/agents/{}/llm-token", binding.agent_id),
        &json!({}),
    )?;
    Ok(response.data)
}

pub fn auth_status(binding: &ConnectionBinding) -> Result<&'static str> {
    let cfg = config::load()?;
    let Some(entry) = cfg.clusters.get(&binding.cluster) else {
        return Ok("cluster missing");
    };
    if normalize_url(&entry.url) != normalize_url(&binding.cluster_url) {
        return Ok("cluster URL changed");
    }
    let Some(token) = entry.token.as_deref() else {
        return Ok("not logged in");
    };
    if config::token_expired(token) == Some(true) {
        return Ok("expired");
    }
    let Some(principal) = config::token_subject(token) else {
        return Ok("unknown");
    };
    if binding
        .principal_id
        .as_deref()
        .is_some_and(|expected| expected != principal)
    {
        return Ok("different user");
    }
    Ok("authenticated")
}

pub fn state_path(agent_id: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nasiko")
        .join("integrations")
        .join(format!("{agent_id}.json"))
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))
        .map(Some)
}

pub fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut content = serde_json::to_vec_pretty(value)?;
    content.push(b'\n');
    atomic_write(path, &content)
}

pub fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    if path.is_symlink() {
        bail!("refusing to replace symlinked file {}", path.display());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to secure {}", parent.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
}

fn normalize_url(url: &str) -> &str {
    url.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_preflight_preserves_state_when_agent_is_running() {
        let error = enforce_disconnect_preflight("Claude Code", &["claude".to_string()], false)
            .unwrap_err();

        assert!(error.to_string().contains("Close all Claude Code sessions"));
        assert!(error.to_string().contains("--force"));
    }

    #[test]
    fn forced_disconnect_allows_active_agent_with_warning_path() {
        assert!(enforce_disconnect_preflight("OpenCode", &["opencode".to_string()], true).is_ok());
    }

    #[test]
    fn disconnect_preflight_allows_stopped_agent() {
        assert!(enforce_disconnect_preflight("Codex", &[], false).is_ok());
    }

    #[test]
    fn account_names_are_safe_and_stable_for_agent_registration() {
        assert_eq!(
            normalize_agent_name_part("ankitkumarnath"),
            "ankitkumarnath"
        );
        assert_eq!(
            normalize_agent_name_part("Ankit Kumar_Nath"),
            "ankit-kumar-nath"
        );
        assert_eq!(normalize_agent_name_part("--Alice--"), "alice");
    }

    #[test]
    fn authenticated_profile_username_wins_over_access_identifier() {
        let profile = json!({"username": "ankit"});
        assert_eq!(
            authoritative_account_username(Some(&profile), Some("NASK_access_identifier")),
            Some("ankit".into())
        );

        let enveloped = json!({"data": {"username": "alice"}});
        assert_eq!(
            authoritative_account_username(Some(&enveloped), Some("NASK_other")),
            Some("alice".into())
        );
    }
    use base64::Engine as _;
    use std::collections::HashMap;

    fn jwt(subject: &str, expires: i64) -> String {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"sub": subject, "exp": expires})).unwrap());
        format!("x.{body}.x")
    }

    #[test]
    fn explicit_agent_must_belong_to_current_principal() {
        let value = json!({"id":"agent-id","name":"shared","owner_id":"other"});
        let error = owned_agent_fields(&value, "me", "shared").unwrap_err();
        assert!(error.to_string().contains("not owned"));
    }

    #[test]
    fn explicit_agent_ownership_is_checked_from_the_server_record() {
        let mut server = mockito::Server::new();
        let request = server
            .mock("GET", "/api/agents/shared")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"id":"agent-id","name":"shared","owner_id":"other"}}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        assert!(resolve_owned_agent(&client, "shared", "me", "claude").is_err());
        request.assert();
    }

    #[test]
    fn caller_controlled_metadata_cannot_select_a_routing_agent() {
        let mut server = mockito::Server::new();
        let request = server
            .mock("GET", "/api/agents/spoofed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"id":"agent-id","name":"spoofed","owner_id":"me","coding_agent_integration_id":null,"metadata":{"integration_id":"claude"}}}"#,
            )
            .create();
        let client = Client::for_test(&server.url(), None);
        let error = resolve_owned_agent(&client, "spoofed", "me", "claude").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not the claude coding-agent integration")
        );
        request.assert();
    }

    #[test]
    fn credential_binding_rejects_url_and_principal_changes_before_http() {
        let binding = ConnectionBinding {
            version: 1,
            integration_id: Some("opencode".into()),
            cluster: "bound".into(),
            cluster_url: "https://bound.example".into(),
            principal_id: Some("me".into()),
            agent_id: "agent".into(),
            agent_name: "opencode".into(),
            executable: None,
        };
        let mut cfg = Config {
            active: Some("other".into()),
            clusters: HashMap::from([(
                "bound".into(),
                ClusterEntry {
                    url: "https://changed.example".into(),
                    username: None,
                    token: Some(jwt("me", chrono::Utc::now().timestamp() + 3600)),
                },
            )]),
            registry_url: None,
        };
        assert!(
            credential_from_config(&binding, &cfg)
                .unwrap_err()
                .to_string()
                .contains("URL changed")
        );
        cfg.clusters.get_mut("bound").unwrap().url = binding.cluster_url.clone();
        cfg.clusters.get_mut("bound").unwrap().token =
            Some(jwt("different", chrono::Utc::now().timestamp() + 3600));
        assert!(
            credential_from_config(&binding, &cfg)
                .unwrap_err()
                .to_string()
                .contains("login changed")
        );
    }

    #[test]
    fn credential_uses_the_bound_cluster_even_when_another_is_active() {
        let mut server = mockito::Server::new();
        let login = jwt("me", chrono::Utc::now().timestamp() + 3600);
        let request = server
            .mock("POST", "/api/agents/agent/llm-token")
            .match_header("authorization", format!("Bearer {login}").as_str())
            .match_body("{}")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"token":"routing-jwt","expires_at":"2026-08-21T12:10:00Z"}}"#)
            .create();
        let binding = ConnectionBinding {
            version: 1,
            integration_id: Some("opencode".into()),
            cluster: "bound".into(),
            cluster_url: server.url(),
            principal_id: Some("me".into()),
            agent_id: "agent".into(),
            agent_name: "opencode".into(),
            executable: None,
        };
        let cfg = Config {
            active: Some("other".into()),
            clusters: HashMap::from([
                (
                    "bound".into(),
                    ClusterEntry {
                        url: server.url(),
                        username: None,
                        token: Some(login),
                    },
                ),
                (
                    "other".into(),
                    ClusterEntry {
                        url: "http://127.0.0.1:1".into(),
                        username: None,
                        token: None,
                    },
                ),
            ]),
            registry_url: None,
        };
        let credential = credential_from_config(&binding, &cfg).unwrap();
        assert_eq!(credential.token, "routing-jwt");
        request.assert();
    }

    #[test]
    fn atomic_write_rolls_back_temp_file_on_destination_failure() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        fs::create_dir(&destination).unwrap();
        assert!(atomic_write(&destination, b"content").is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_local_install_can_restore_the_previous_remote_config() {
        let mut server = mockito::Server::new();
        let current = server
            .mock("GET", "/api/agents/agent/llm-config")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"llm_config_id":"old","llm_config":{"id":"old","provider":"openai","model":"old-model"}}}"#,
            )
            .create();
        let configs = server
            .mock("GET", "/api/llm-configs")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"id":"new","name":"new-config","provider":"openai","model":"new-model"}]}"#,
            )
            .create();
        let attach = server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"llm_config_id": "new"})))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"llm_config_id":"new","llm_config":{"id":"new","provider":"openai","model":"new-model"}}}"#,
            )
            .create();
        let restore = server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"llm_config_id": "old"})))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"llm_config_id":"old","llm_config":{"id":"old","provider":"openai","model":"old-model"}}}"#,
            )
            .create();
        let client = Client::for_test(&server.url(), None);
        let (resolved, previous_llm_config_id) =
            configure_agent_for_install(&client, "agent", Some("new-config")).unwrap();
        assert_eq!(resolved["model"], "new-model");
        let prepared = PreparedConnection {
            binding: ConnectionBinding {
                version: 1,
                integration_id: Some("opencode".into()),
                cluster: "test".into(),
                cluster_url: server.url(),
                principal_id: Some("owner".into()),
                agent_id: "agent".into(),
                agent_name: "opencode".into(),
                executable: None,
            },
            resolved_config: resolved,
            entry: ClusterEntry {
                url: server.url(),
                username: None,
                token: None,
            },
            previous_llm_config_id,
        };
        rollback_config(&prepared).unwrap();
        current.assert();
        configs.assert();
        attach.assert();
        restore.assert();
    }
}
