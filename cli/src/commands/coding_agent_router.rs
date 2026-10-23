//! Shared control-plane and state primitives for local coding-agent LLM routing.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
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

/// What a routing `connect` installs, chosen by `nasiko connect` (flags or prompts).
#[derive(Debug, Clone, Copy)]
pub struct RoutingOptions<'a> {
    /// Existing routing agent name or UUID; `None` uses the account's default integration agent.
    pub agent: Option<&'a str>,
    /// LLM config name or UUID to attach; `None` keeps the agent's current (or default) config.
    pub llm_config: Option<&'a str>,
    /// Whether to mint and register an MCP gateway credential alongside LLM routing.
    pub mcp: bool,
    /// Set when the agent's `check` accepted these same options just before, so `connect`
    /// can skip the router probe it would otherwise repeat.
    pub checked: bool,
}

/// A check or a failed connect changed the agent's LLM config on the control plane and could not
/// change it back: the agent may now route through the config being tried.
#[derive(Debug)]
pub struct ConfigNotRestored(pub String);

impl std::fmt::Display for ConfigNotRestored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the agent's previous LLM config could not be put back ({}); it may now route through the one being tried",
            self.0
        )
    }
}

impl std::error::Error for ConfigNotRestored {}

/// Puts back the agent's LLM config after a check or a failed connect changed it, failing with
/// [`ConfigNotRestored`] so callers can say so.
pub fn put_back_config(prepared: &PreparedConnection) -> Result<()> {
    prepared.config_change().undo()
}

/// What an agent's LLM settings were before `connect` changed its config: the attached config
/// and the model pinned in it. The server clears the pin whenever the config changes, so putting
/// the config back must put the pin back too.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviousConfig {
    llm_config_id: Option<String>,
    pinned_model: Option<String>,
}

/// Attaches `previous` to the agent again: the config, then (a separate request, since the server
/// ignores a pin sent with a config change) its pinned model. Fails with [`ConfigNotRestored`].
fn put_back(client: &Client, agent_id: &str, previous: &PreviousConfig) -> Result<()> {
    let path = format!("/agents/{agent_id}/llm-config");
    let put_back = || -> Result<()> {
        client.patch_json::<Value, _>(&path, &json!({"llm_config_id": previous.llm_config_id}))?;
        if let Some(model) = &previous.pinned_model {
            client.patch_json::<Value, _>(&path, &json!({"pinned_model": model}))?;
        }
        Ok(())
    };
    put_back().map_err(|error| anyhow::Error::new(ConfigNotRestored(format!("{error:#}"))))
}

/// Ends a connect that failed after [`prepare`] changed the agent's LLM config: puts the config
/// back and returns `error`, or — when that fails too — both, marked [`ConfigNotRestored`].
pub fn fail_after_prepare<T>(
    prepared: &PreparedConnection,
    what_failed: &str,
    error: anyhow::Error,
) -> Result<T> {
    match put_back_config(prepared) {
        Ok(()) => Err(error),
        Err(put_back) => Err(put_back.context(format!("{what_failed}: {error:#}"))),
    }
}

/// The LLM config change a `connect` made to its routing agent — kept so that undoing the
/// connection right after (when a later step fails) undoes the change too.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    entry: ClusterEntry,
    agent_id: String,
    /// `Some(previous)` when the config was changed; `None` when it was left as it was.
    previous: Option<PreviousConfig>,
}

impl ConfigChange {
    /// A connect that left the agent's LLM config as it was.
    #[cfg(test)]
    pub fn unchanged() -> Self {
        Self {
            entry: ClusterEntry {
                url: String::new(),
                username: None,
                token: None,
            },
            agent_id: String::new(),
            previous: None,
        }
    }

    /// Puts the previous LLM config (and its pinned model) back; a no-op when nothing was
    /// changed. Fails with [`ConfigNotRestored`].
    pub fn undo(&self) -> Result<()> {
        let Some(previous) = &self.previous else {
            return Ok(());
        };
        let client = Client::from_cluster_entry_with_timeout(&self.entry, Some(CP_CALL_TIMEOUT));
        put_back(&client, &self.agent_id, previous)
    }
}

/// A connected routing integration, as `nasiko status` summarizes it.
#[derive(Debug, Clone)]
pub struct RoutingSummary {
    pub cluster: String,
    pub cluster_url: String,
    /// The user the connection was made as; `None` for connections from older versions.
    pub principal_id: Option<String>,
    pub agent_id: String,
    pub agent_name: String,
    pub mcp_installed: bool,
}

impl RoutingSummary {
    pub fn from_binding(binding: &ConnectionBinding, mcp_installed: bool) -> Self {
        Self {
            cluster: binding.cluster.clone(),
            cluster_url: binding.cluster_url.clone(),
            principal_id: binding.principal_id.clone(),
            agent_id: binding.agent_id.clone(),
            agent_name: binding.agent_name.clone(),
            mcp_installed,
        }
    }
}

/// The MCP outcome every routing `connect` reports once its install has completed.
pub fn print_mcp_outcome(mint: Option<&McpMint>, registration_warning: Option<&str>) {
    match (mint, registration_warning) {
        (None, _) => println!("MCP gateway: not requested"),
        (Some(McpMint::Minted(_)), None) => {
            println!("{}", mcp_connected_line());
            println!("{}", mcp_multi_machine_note());
        }
        (Some(McpMint::Minted(_)), Some(reason)) => {
            term::print_warning(&format!(
                "{reason}; LLM routing is connected, MCP tools are not"
            ));
        }
        (Some(McpMint::Unavailable(message)), _) => {
            term::print_warning(&format!(
                "MCP gateway unavailable ({message}); LLM routing is connected, MCP tools are not"
            ));
        }
    }
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
    previous: Option<PreviousConfig>,
}

impl PreparedConnection {
    /// The LLM config change `prepare` made, for undoing it later.
    pub fn config_change(&self) -> ConfigChange {
        ConfigChange {
            entry: self.entry.clone(),
            agent_id: self.binding.agent_id.clone(),
            previous: self.previous.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingCredential {
    pub token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// The MCP server name each client registers the gateway under.
pub const MCP_SERVER_NAME: &str = "nasiko";

/// Shared timeout for the short-lived, one-off control-plane calls `connect`/`disconnect` make
/// outside the main `prepare()` flow (MCP mint/revoke, LLM config rollback): long enough for a
/// normal request, short enough that a hung server doesn't stall a local install indefinitely.
pub(crate) const CP_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Upper bound on [`preflight_routing`]. Longer than [`CP_CALL_TIMEOUT`] because the probe
/// is a real (if tiny) provider round trip rather than a control-plane read, and a slow
/// upstream must not be mistaken for a broken configuration.
const PREFLIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Output cap for the preflight turn. Deliberately tiny — the probe exists to exercise
/// resolution and reach the provider, not to produce text, but it is a real billed turn.
const PREFLIGHT_MAX_OUTPUT_TOKENS: u32 = 16;

/// A minted MCP gateway credential. `token` and `connect_url` are both secrets (the URL embeds
/// the token), so `Debug` redacts them rather than deriving it.
#[derive(Clone)]
pub struct McpCredential {
    pub token: String,
    pub gateway_url: String,
    pub connect_url: String,
}

impl std::fmt::Debug for McpCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCredential")
            .field("token", &"ngt_…<redacted>")
            .field("gateway_url", &self.gateway_url)
            .field("connect_url", &"<redacted>")
            .finish()
    }
}

/// The wire shape of `POST /api/agents/{id}/mcp-token`'s `data` (`McpTokenResponse`,
/// `oss/server/src/agents/llm_config.rs`) that the CLI actually uses. The response also carries
/// `gateway_url`/`connect_url`, but those are the URLs told to agent *containers*
/// (`MCP_GATEWAY_PUBLIC_URL`, e.g. `http://server:8080/api/mcp` or a Docker-internal host — see
/// `oss/config/src/lib.rs`), unreachable from the developer's own host. `mcp_credential` derives
/// host-facing URLs from the cluster URL it already used to reach this endpoint instead.
#[derive(Deserialize)]
struct RawMcpToken {
    token: String,
}

/// Mint the bound agent's MCP gateway credential, using `cluster_url` (the same control-plane URL
/// this call itself reached) to build `gateway_url`/`connect_url` rather than the response's own
/// container-facing pair — see `RawMcpToken`. Errors are the caller's to soften: an older control
/// plane without this route, or any other transport failure, must not break LLM-routing connect.
pub fn mcp_credential(client: &Client, agent_id: &str, cluster_url: &str) -> Result<McpCredential> {
    let response: Envelope<RawMcpToken> =
        client.post_json_quiet(&format!("/agents/{agent_id}/mcp-token"), &json!({}))?;
    let gateway_url = format!("{}/api/mcp", cluster_url.trim_end_matches('/'));
    let connect_url = compose_connect_url(&gateway_url, &response.data.token);
    Ok(McpCredential {
        token: response.data.token,
        gateway_url,
        connect_url,
    })
}

/// `{gateway_url}/s/{token}` — mirrors `nasiko_mcp_gateway::injector::connect_url`. Duplicated
/// as a one-line rule rather than depending on that crate, which would pull the gateway's own
/// dependency tree into this dependency-light CLI for a single format string.
fn compose_connect_url(gateway_url: &str, token: &str) -> String {
    format!("{}/s/{token}", gateway_url.trim_end_matches('/'))
}

/// The outcome of attempting to mint an MCP gateway credential for `connect`: never an error the
/// caller must propagate, since MCP is optional and LLM-routing connect must proceed either way.
pub enum McpMint {
    Minted(McpCredential),
    Unavailable(String),
}

/// Mints the bound agent's MCP gateway credential for `connect` without printing anything: each
/// caller only learns the final, reportable outcome (LLM routing succeeded, and separately
/// whether MCP client-side registration also succeeded) once its own install has actually
/// completed, so the status lines belong there — see each `connect`'s final printing.
pub fn mint_mcp_credential(client: &Client, agent_id: &str, cluster_url: &str) -> McpMint {
    match mcp_credential(client, agent_id, cluster_url) {
        Ok(credential) => McpMint::Minted(credential),
        Err(error) => McpMint::Unavailable(one_line_warning(&error)),
    }
}

/// A single line for a `warning:` print: the outermost line of `format!("{error:#}")` — anyhow's
/// alternate `Display` appends the full cause chain on one line, so a wrapped transport failure
/// (e.g. `.context("failed to run ...")`) keeps its root cause — with everything after the first
/// line dropped. In practice the only multi-line message this drops anything from is
/// `check_status`'s trailing `\nhint: ...`, worded for a direct API caller rather than this
/// best-effort warning.
pub fn one_line_warning(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message.lines().next().unwrap_or(&message).to_string()
}

/// The line to print once a minted credential has been registered client-side successfully.
pub fn mcp_connected_line() -> String {
    format!("MCP gateway: connected (server \"{MCP_SERVER_NAME}\")")
}

/// Printed right after `mcp_connected_line()`: the control plane keeps one MCP gateway credential
/// per agent row (`nasiko_mcp_gateway::agent_tokens::mint` upserts), so connecting the same agent
/// from a second machine replaces this one, and disconnecting from either machine revokes it for
/// both.
pub fn mcp_multi_machine_note() -> &'static str {
    "Note: one MCP credential per agent; connecting from another machine replaces it."
}

/// Revoke the bound agent's MCP gateway credential. The caller softens a failure into a warning.
pub fn revoke_mcp_credential(client: &Client, agent_id: &str) -> Result<()> {
    client.delete(&format!("/agents/{agent_id}/mcp-token"))
}

/// Revokes the bound agent's MCP gateway credential, turning a failure into a printed warning
/// instead of propagating it — used both when `connect` must unwind a mint it can no longer use,
/// and during `disconnect`, where the local state must be cleaned up regardless of whether the
/// control plane is reachable.
pub fn revoke_mcp_credential_best_effort(client: &Client, agent_id: &str) {
    if let Err(error) = revoke_mcp_credential(client, agent_id) {
        term::print_warning(&format!(
            "failed to revoke the MCP gateway credential: {}",
            one_line_warning(&error)
        ));
    }
}

/// Looks up `binding.cluster` in `cfg`, refusing if the alias no longer exists or has since been
/// re-pointed at a different URL — shared by `client_for_binding` and `credential_from_config`,
/// which append their own `remedy` to whichever of the two problems is found.
fn bound_cluster_entry<'a>(
    binding: &ConnectionBinding,
    cfg: &'a Config,
    remedy: &str,
) -> Result<&'a ClusterEntry> {
    let entry = cfg.clusters.get(&binding.cluster).ok_or_else(|| {
        anyhow::anyhow!(
            "Nasiko cluster '{}' no longer exists; {remedy}",
            binding.cluster
        )
    })?;
    if normalize_url(&entry.url) != normalize_url(&binding.cluster_url) {
        bail!(
            "Nasiko cluster '{}' URL changed since connect; {remedy}",
            binding.cluster
        );
    }
    Ok(entry)
}

/// Resolves a `Client` for `binding`'s cluster from `~/.nasiko/config.json`, independent of
/// whichever cluster is currently active — so a stale binding's DELETE never reaches a server
/// other than the one that issued the credential.
fn client_for_binding(binding: &ConnectionBinding) -> Result<Client> {
    let cfg = config::load()?;
    let entry = bound_cluster_entry(binding, &cfg, "skipping MCP gateway credential revoke")?;
    Ok(Client::from_cluster_entry_with_timeout(
        entry,
        Some(CP_CALL_TIMEOUT),
    ))
}

/// Revokes `binding`'s MCP gateway credential, turning any failure — an unreachable control
/// plane, a re-pointed cluster alias, ... — into a printed warning rather than propagating it.
/// Shared by every `disconnect` (Claude, Codex, OpenCode): local cleanup must proceed regardless
/// of whether the control plane is reachable.
pub fn revoke_bound_mcp_credential_best_effort(binding: &ConnectionBinding) {
    match client_for_binding(binding) {
        Ok(client) => revoke_mcp_credential_best_effort(&client, &binding.agent_id),
        Err(error) => term::print_warning(&format!(
            "failed to revoke the MCP gateway credential: {}",
            one_line_warning(&error)
        )),
    }
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
        term::print_warning(&format!(
            "disconnecting while {display_name} is running ({names}); open sessions may fail API requests until restarted"
        ));
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
    let (resolved_config, previous) = configure_agent_for_install(&client, &agent_id, llm_config)?;
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
        previous,
    })
}

pub fn require_current_login() -> Result<(String, ClusterEntry, String)> {
    let (cluster, entry) = config::active_cluster()?;
    let token = usable_login_token(&entry)?;
    let principal = config::token_subject(token).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid Nasiko login; run: {}",
            crate::commands::local_agents::login_hint()
        )
    })?;
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
    let token = entry.token.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "not logged in to Nasiko; run: {}",
            crate::commands::local_agents::login_hint()
        )
    })?;
    if config::token_expired(token) == Some(true) {
        bail!(
            "Nasiko session expired; run: {}",
            crate::commands::local_agents::login_hint()
        );
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
    if !is_integration_agent(&agent, integration_id) {
        bail!("agent '{reference}' is not the {integration_id} coding-agent integration");
    }
    owned_agent_fields(&agent, principal_id, reference)
}

/// What the control plane says about reusing an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLookup {
    /// The current user's agent for this coding agent — what [`resolve_owned_agent`] accepts.
    Usable,
    /// No such agent (404).
    Gone,
    /// It exists, but isn't the current user's agent for this coding agent.
    NotYours,
    /// Access was refused (403): someone else's, or the user's own under a permission problem —
    /// the response doesn't say which.
    Refused,
}

/// Looks `reference` up the way [`resolve_owned_agent`] would, answering instead of failing.
/// Errors only when the control plane can't be asked.
pub fn look_up_owned_agent(reference: &str, integration_id: &str) -> Result<AgentLookup> {
    let (_, entry, principal_id) = require_current_login()?;
    let client = Client::from_cluster_entry_with_timeout(&entry, Some(CP_CALL_TIMEOUT));
    Ok(match client.get_agent_or_status(reference, &[403, 404])? {
        Err(404) => AgentLookup::Gone,
        Err(_) => AgentLookup::Refused,
        Ok(agent)
            if is_integration_agent(&agent, integration_id)
                && owned_agent_fields(&agent, &principal_id, reference).is_ok() =>
        {
            AgentLookup::Usable
        }
        Ok(_) => AgentLookup::NotYours,
    })
}

fn is_integration_agent(agent: &Value, integration_id: &str) -> bool {
    agent
        .get("coding_agent_integration_id")
        .and_then(Value::as_str)
        == Some(integration_id)
}

fn owned_agent_fields(
    agent: &Value,
    principal_id: &str,
    fallback_name: &str,
) -> Result<(String, String)> {
    let owner = agent.get("owner_id").and_then(Value::as_str);
    if !owner.is_some_and(|owner| config::same_principal(owner, principal_id)) {
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
) -> Result<(Value, Option<PreviousConfig>)> {
    let path = format!("/agents/{agent_id}/llm-config");
    let current: Value = client.get_json(&path)?;
    let current_field = |field: &str| {
        current
            .get("data")
            .and_then(|data| data.get(field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let current_id = current_field("llm_config_id");
    let current_pin = current_field("pinned_model");
    let (response, previous) = if let Some(reference) = llm_config {
        let config = fetch_config_by_ref(client, reference)?;
        let config_id = config
            .get("id")
            .and_then(Value::as_str)
            .context("LLM config is missing an id")?;
        if current_id.as_deref() == Some(config_id) {
            (current, None)
        } else {
            let previous = PreviousConfig {
                llm_config_id: current_id,
                pinned_model: current_pin,
            };
            (
                client.patch_json(&path, &json!({"llm_config_id": config_id}))?,
                Some(previous),
            )
        }
    } else {
        (current, None)
    };
    let resolved = response
        .get("data")
        .and_then(|data| data.get("llm_config"))
        .filter(|value| !value.is_null())
        .cloned();
    let Some(resolved) = resolved else {
        let error = anyhow::anyhow!(
            "no Nasiko LLM config is available; create a default config or pass --config <name>"
        );
        // The config was changed above but can't be used: put the previous one back, so a
        // failed connect never leaves the agent on it — and say so when that fails too.
        if let Some(previous) = &previous
            && let Err(put_back) = put_back(client, agent_id, previous)
        {
            return Err(put_back.context(format!("{error:#}")));
        }
        return Err(error);
    };
    Ok((resolved, previous))
}

/// What [`preflight_routing`] established about an agent's LLM configuration.
pub enum RoutingPreflight {
    /// The router resolved the config and reached its provider.
    Reachable,
    /// The router refused the configuration itself. Every turn the coding agent makes
    /// would fail the same way, so `connect` must not proceed.
    Rejected(String),
    /// Nothing was established — an older control plane without the route, a transport
    /// failure, or an upstream outage. The caller warns and connects anyway.
    Inconclusive(String),
}

/// Probes LLM routing the way the connected coding agent will: one tiny non-streaming
/// `/v1/responses` turn carrying the bound agent's own routing credential.
///
/// Without it, a config naming a provider that was never registered as a custom-provider
/// row surfaces only on the developer's first turn, as a 400 from the resolver
/// (`llm-router/src/resolver/mod.rs:433`) against a config that already looks installed.
pub fn preflight_routing(entry: &ClusterEntry, agent_id: &str, model: &str) -> RoutingPreflight {
    let _spin = nasiko_utils::term::start_status("verifying LLM routing");
    let client = Client::from_cluster_entry_with_timeout(entry, Some(PREFLIGHT_TIMEOUT));
    let credential = match mint_routing_credential(&client, agent_id) {
        Ok(credential) => credential,
        Err(error) => return RoutingPreflight::Inconclusive(one_line_warning(&error)),
    };
    match client.post_raw_with_token("/v1/responses", &credential.token, &preflight_body(model)) {
        Ok((status, body)) => classify_preflight(status, &body),
        Err(error) => RoutingPreflight::Inconclusive(one_line_warning(&error)),
    }
}

fn preflight_body(model: &str) -> Value {
    json!({
        "model": model,
        "input": "ping",
        "max_output_tokens": PREFLIGHT_MAX_OUTPUT_TOKENS,
        "stream": false,
        "store": false,
    })
}

/// Only the router's own refusals are fatal. A 4xx is a verdict on this configuration; a
/// missing route (an older control plane), a throttle, a timeout, or an upstream fault
/// says nothing about it — `429` and `5xx` in particular reach the router only *after*
/// resolution has already succeeded.
fn classify_preflight(status: u16, body: &str) -> RoutingPreflight {
    if (200..300).contains(&status) {
        return RoutingPreflight::Reachable;
    }
    if status == 404 {
        return RoutingPreflight::Inconclusive(
            "this control plane has no /v1/responses route".to_string(),
        );
    }
    let reason = preflight_error_message(body).unwrap_or_else(|| format!("HTTP {status}"));
    match status {
        408 | 429 => RoutingPreflight::Inconclusive(reason),
        400..=499 => RoutingPreflight::Rejected(reason),
        _ => RoutingPreflight::Inconclusive(reason),
    }
}

/// The `error.message` of a router error body (`llm-router/src/handlers/responses.rs:934`).
fn preflight_error_message(body: &str) -> Option<String> {
    let body: Value = serde_json::from_str(body).ok()?;
    body.get("error")?
        .get("message")?
        .as_str()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

/// Mints the bound agent's routing credential — the bearer every connected coding agent
/// presents to the LLM router, whether it was obtained by the credential helper or by the
/// connect-time preflight.
fn mint_routing_credential(client: &Client, agent_id: &str) -> Result<RoutingCredential> {
    let response: Envelope<RoutingCredential> =
        client.post_json_quiet(&format!("/agents/{agent_id}/llm-token"), &json!({}))?;
    Ok(response.data)
}

pub fn credential(binding: &ConnectionBinding) -> Result<RoutingCredential> {
    credential_from_config(binding, &config::load()?)
}

fn credential_from_config(binding: &ConnectionBinding, cfg: &Config) -> Result<RoutingCredential> {
    let entry = bound_cluster_entry(
        binding,
        cfg,
        &format!(
            "run: {} connect {}",
            crate::commands::local_agents::cli_name(),
            binding.integration_id.as_deref().unwrap_or("claude")
        ),
    )?;
    let token = usable_login_token(entry)?;
    let principal = config::token_subject(token).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid Nasiko login; run: {}",
            crate::commands::local_agents::login_hint()
        )
    })?;
    if binding
        .principal_id
        .as_deref()
        .is_some_and(|expected| !config::same_principal(expected, &principal))
    {
        bail!("Nasiko login changed since connect; reconnect this coding agent");
    }
    let client = Client::from_cluster_entry_with_timeout(entry, Some(CP_CALL_TIMEOUT));
    mint_routing_credential(&client, &binding.agent_id)
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
        .is_some_and(|expected| !config::same_principal(expected, &principal))
    {
        return Ok("different user");
    }
    Ok("authenticated")
}

/// How a `nasiko status claude|codex|opencode` value reads at a glance.
#[derive(Debug, PartialEq, Eq)]
enum StatusTone {
    Good,
    Problem,
    Attention,
    Absent,
    Plain,
}

/// Classifies the status words these commands print — [`auth_status`], settings/plugin
/// state, MCP, session reporting, and the cells of the `status` agent table. Anything
/// unrecognized stays plain.
fn status_tone(value: &str) -> StatusTone {
    match value {
        "connected" | "authenticated" | "active" | "configured" | "on" | "yes" => StatusTone::Good,
        v if v.starts_with("active (") || v.starts_with("on (") => StatusTone::Good,
        "expired" | "not logged in" | "cluster missing" | "different user" | "missing"
        | "state unreadable" | "?" => StatusTone::Problem,
        "cluster URL changed" | "changed since connect" | "unknown" | "needs reinstall" => {
            StatusTone::Attention
        }
        v if v.starts_with("stale") => StatusTone::Attention,
        "not connected" | "not configured" | "not installed" | "not supported" | "off" | "no"
        | "-" | "detect-only" => StatusTone::Absent,
        _ => StatusTone::Plain,
    }
}

/// `value` colored by [`status_tone`] for a status row: green when healthy, red when broken,
/// yellow when it needs a look, dim when simply not set up. Plain text when stdout isn't a
/// terminal, so captured output is unchanged. Only for the human-facing status commands —
/// never the credential-helper path, whose stdout other programs read.
pub(crate) fn status_value(value: &str) -> String {
    match status_tone(value) {
        StatusTone::Good => term::green(value),
        StatusTone::Problem => term::red(value),
        StatusTone::Attention => term::yellow(value),
        StatusTone::Absent => term::dim(value),
        StatusTone::Plain => value.to_string(),
    }
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
    fn status_tone_classifies_every_value_the_status_commands_print() {
        use StatusTone::*;
        for (value, tone) in [
            ("connected", Good),
            ("authenticated", Good),
            ("active", Good),
            ("active (v3)", Good),
            ("configured", Good),
            ("expired", Problem),
            ("not logged in", Problem),
            ("cluster missing", Problem),
            ("different user", Problem),
            ("missing", Problem),
            ("cluster URL changed", Attention),
            ("changed since connect", Attention),
            ("unknown", Attention),
            ("needs reinstall", Attention),
            ("stale (v2; current v3)", Attention),
            ("stale or changed (v2)", Attention),
            ("not connected", Absent),
            ("not configured", Absent),
            ("not installed", Absent),
            // The `status` agent table's cells.
            ("on", Good),
            ("yes", Good),
            ("on (cp)", Good),
            ("on (no content)", Good),
            ("state unreadable", Problem),
            ("?", Problem),
            ("not supported", Absent),
            ("off", Absent),
            ("no", Absent),
            ("-", Absent),
            ("detect-only", Absent),
            ("nasiko/router", Plain),
        ] {
            assert_eq!(status_tone(value), tone, "{value}");
        }
    }

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

    /// The reason a routing preflight fails `connect`, or `None` when it doesn't.
    fn rejection(status: u16, body: &str) -> Option<String> {
        match classify_preflight(status, body) {
            RoutingPreflight::Rejected(reason) => Some(reason),
            RoutingPreflight::Reachable | RoutingPreflight::Inconclusive(_) => None,
        }
    }

    #[test]
    fn a_router_refusal_fails_connect_and_carries_its_own_reason() {
        let unregistered = serde_json::json!({
            "error": {
                "message": "provider 'bedrock-provider' is not a registered custom provider",
                "type": "invalid_request_error",
                "code": "invalid_request_error",
            }
        })
        .to_string();
        assert_eq!(
            rejection(400, &unregistered).as_deref(),
            Some("provider 'bedrock-provider' is not a registered custom provider")
        );
        // An unparseable or empty body still rejects — the status is the verdict, the
        // message only sharpens it.
        assert_eq!(rejection(403, "").as_deref(), Some("HTTP 403"));
    }

    #[test]
    fn nothing_but_a_router_refusal_blocks_connect() {
        // Reached the provider: routing resolved.
        assert!(matches!(
            classify_preflight(200, "{}"),
            RoutingPreflight::Reachable
        ));
        // A control plane predating `/v1/responses`, a throttle, a timeout, and an upstream
        // fault all leave this config's routability unknown — none of them may block a
        // local install.
        for (status, body) in [
            (404, ""),
            (408, ""),
            (429, r#"{"error":{"message":"rate limited"}}"#),
            (502, r#"{"error":{"message":"provider returned 500"}}"#),
            (503, ""),
        ] {
            assert!(
                matches!(
                    classify_preflight(status, body),
                    RoutingPreflight::Inconclusive(_)
                ),
                "status {status} must not block connect"
            );
        }
    }

    #[test]
    fn preflight_body_is_the_cheapest_turn_that_still_resolves() {
        let body = preflight_body("anthropic.claude-sonnet-4-v1:0");
        assert_eq!(body["model"], "anthropic.claude-sonnet-4-v1:0");
        assert_eq!(body["max_output_tokens"], PREFLIGHT_MAX_OUTPUT_TOKENS);
        assert_eq!(body["stream"], false);
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
    fn mcp_credential_derives_host_facing_urls_from_the_cluster_url_not_the_response() {
        // The response's own `gateway_url`/`connect_url` are container-facing (told to agent
        // *containers* via `MCP_GATEWAY_PUBLIC_URL`) and unreachable from this host, so they must
        // never leak into what the CLI installs — only `token` is trusted from the wire.
        let mut server = mockito::Server::new();
        let request = server
            .mock("POST", "/api/agents/agent/mcp-token")
            .match_body("{}")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"token":"ngt_abc","gateway_url":"http://server:8080/api/mcp","connect_url":"http://server:8080/api/mcp/s/ngt_abc"},"status_code":200,"message":"ok"}"#,
            )
            .create();
        let client = Client::for_test(&server.url(), None);
        let credential = mcp_credential(&client, "agent", "https://cp.example.com/").unwrap();
        assert_eq!(credential.token, "ngt_abc");
        assert_eq!(credential.gateway_url, "https://cp.example.com/api/mcp");
        assert_eq!(
            credential.connect_url,
            "https://cp.example.com/api/mcp/s/ngt_abc"
        );
        request.assert();
    }

    #[test]
    fn mcp_credential_trims_a_trailing_slash_from_the_cluster_url() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/agents/agent/mcp-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"token":"ngt_abc"}}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        let credential = mcp_credential(&client, "agent", &format!("{}/", server.url())).unwrap();
        assert_eq!(credential.gateway_url, format!("{}/api/mcp", server.url()));
    }

    #[test]
    fn mint_reports_the_servers_own_message_on_a_404_with_no_special_casing() {
        // A modern control plane's only 404 here is "agent not found" — there is no
        // purpose-built "upgrade" message to special-case. An older control plane without this
        // route doesn't 404 at all; it falls through into the generic A2A proxy instead (see the
        // 503 test below), so a bare 404 never needs a translated message.
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/agents/agent/mcp-token")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message":"agent not found"}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        match mint_mcp_credential(&client, "agent", &server.url()) {
            McpMint::Unavailable(message) => assert_eq!(
                message,
                format!(
                    "HTTP 404 from {}/api/agents/agent/mcp-token: agent not found",
                    server.url()
                )
            ),
            McpMint::Minted(_) => panic!("expected Unavailable"),
        }
    }

    #[test]
    fn mint_surfaces_a_503_from_an_older_control_planes_a2a_proxy_fallthrough() {
        // An older control plane without `POST /agents/{id}/mcp-token` doesn't 404: the request
        // falls into the generic `/api/agents/{id}/*` A2A proxy instead, which answers with its
        // own error — a 503 here, standing in for "no agent container to proxy this POST to".
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/agents/agent/mcp-token")
            .with_status(503)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message":"agent container unreachable"}"#)
            .create();
        let client = Client::for_test(&server.url(), None);
        match mint_mcp_credential(&client, "agent", &server.url()) {
            McpMint::Unavailable(message) => assert_eq!(
                message,
                format!(
                    "HTTP 503 from {}/api/agents/agent/mcp-token: agent container unreachable",
                    server.url()
                )
            ),
            McpMint::Minted(_) => panic!("expected Unavailable"),
        }
    }

    #[test]
    fn revoke_mcp_credential_accepts_a_204() {
        let mut server = mockito::Server::new();
        let request = server
            .mock("DELETE", "/api/agents/agent/mcp-token")
            .with_status(204)
            .create();
        let client = Client::for_test(&server.url(), None);
        revoke_mcp_credential(&client, "agent").unwrap();
        request.assert();
    }

    #[test]
    fn one_line_warning_drops_the_hint_for_any_status_code() {
        let error = anyhow::anyhow!(
            "HTTP 500 from http://cp/api/agents/a/mcp-token: boom\nhint: server error — check server logs or try again"
        );
        assert_eq!(
            one_line_warning(&error),
            "HTTP 500 from http://cp/api/agents/a/mcp-token: boom"
        );
    }

    #[test]
    fn one_line_warning_passes_through_a_plain_message() {
        let error = anyhow::anyhow!("cannot reach control plane");
        assert_eq!(one_line_warning(&error), "cannot reach control plane");
    }

    #[test]
    fn one_line_warning_keeps_the_chained_cause_for_a_wrapped_transport_failure() {
        let error =
            anyhow::anyhow!("connection refused").context("failed to run `claude mcp add-json`");
        assert_eq!(
            one_line_warning(&error),
            "failed to run `claude mcp add-json`: connection refused"
        );
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
        let (resolved, previous) =
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
            previous,
        };
        put_back_config(&prepared).unwrap();
        current.assert();
        configs.assert();
        attach.assert();
        restore.assert();
    }

    #[test]
    fn putting_a_config_back_puts_its_pinned_model_back_too() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/agents/agent/llm-config")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":{"llm_config_id":"old","pinned_model":"pinned","llm_config":{"id":"old"}}}"#,
            )
            .create();
        server
            .mock("GET", "/api/llm-configs")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"new","name":"new-config"}]}"#)
            .create();
        let ok = r#"{"data":{"llm_config_id":"new","llm_config":{"id":"new"}}}"#;
        server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"llm_config_id": "new"})))
            .with_status(200)
            .with_body(ok)
            .create();
        // The server clears the pin when the config changes, so it must come back separately.
        let config_back = server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"llm_config_id": "old"})))
            .with_status(200)
            .with_body(ok)
            .create();
        let pin_back = server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"pinned_model": "pinned"})))
            .with_status(200)
            .with_body(ok)
            .create();
        let client = Client::for_test(&server.url(), None);
        let (_, previous) =
            configure_agent_for_install(&client, "agent", Some("new-config")).unwrap();
        let previous = previous.expect("the config was changed");
        put_back(&client, "agent", &previous).unwrap();
        config_back.assert();
        pin_back.assert();
    }

    #[test]
    fn a_pin_that_cannot_be_put_back_is_reported_as_not_restored() {
        let mut server = mockito::Server::new();
        server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"llm_config_id": "old"})))
            .with_status(200)
            .with_body("{}")
            .create();
        server
            .mock("PATCH", "/api/agents/agent/llm-config")
            .match_body(mockito::Matcher::Json(json!({"pinned_model": "pinned"})))
            .with_status(500)
            .create();
        let client = Client::for_test(&server.url(), None);
        let previous = PreviousConfig {
            llm_config_id: Some("old".into()),
            pinned_model: Some("pinned".into()),
        };
        let error = put_back(&client, "agent", &previous).unwrap_err();
        assert!(error.chain().any(|cause| cause.is::<ConfigNotRestored>()));
    }
}
