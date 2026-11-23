mod claude_mcp;
mod codex_mcp;
mod opencode_mcp;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::api::Client;
use crate::commands::coding_agent_router::{
    self, AgentSpec, ConnectionBinding, McpCredential, RoutingSummary,
};
use crate::commands::integration;

pub use claude_mcp::unregister_best_effort as unregister_claude_best_effort;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const DESKTOP_STATE_DIR_ENV: &str = "NASIKO_DESKTOP_STATE_DIR";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpClient {
    Claude,
    Codex,
    OpenCode,
}

impl McpClient {
    pub fn for_id(id: &str) -> Option<Self> {
        match id {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    fn spec(self) -> AgentSpec<'static> {
        match self {
            Self::Claude => AgentSpec {
                id: "claude",
                display_name: "Claude Code",
                default_name: "claude-code",
            },
            Self::Codex => AgentSpec {
                id: "codex",
                display_name: "Codex CLI",
                default_name: "codex",
            },
            Self::OpenCode => AgentSpec {
                id: "opencode",
                display_name: "OpenCode",
                default_name: "opencode",
            },
        }
    }

    pub fn target(self) -> String {
        match self {
            Self::Claude => format!(
                "`claude mcp add {}` (user scope)",
                coding_agent_router::MCP_SERVER_NAME
            ),
            Self::Codex => format!(
                "[mcp_servers.{}] in {}",
                coding_agent_router::MCP_SERVER_NAME,
                integration::tildify(&crate::commands::codex::config_path())
            ),
            Self::OpenCode => integration::tildify(&opencode_mcp::plugin_path()),
        }
    }

    fn state_path(self) -> PathBuf {
        coding_agent_router::state_path(&format!("{}-mcp", self.id()))
    }

    fn check_local(self) -> Result<()> {
        match self {
            Self::Claude => claude_mcp::check().map(|_| ()),
            Self::Codex => codex_mcp::check(),
            Self::OpenCode => opencode_mcp::check(&opencode_mcp::plugin_path()),
        }
    }

    fn register(self, credential: &McpCredential) -> Result<Registration> {
        match self {
            Self::Claude => claude_mcp::register(credential).map(|()| Registration::default()),
            Self::Codex => codex_mcp::register(credential).map(|record| Registration {
                codex: Some(record),
                ..Registration::default()
            }),
            Self::OpenCode => opencode_mcp::register(credential).map(|path| Registration {
                opencode_plugin: Some(path),
                ..Registration::default()
            }),
        }
    }

    fn unregister(self, state: &McpState) -> Result<()> {
        match self {
            Self::Claude => claude_mcp::unregister(&state.token_sha256),
            Self::Codex => match &state.codex {
                Some(record) => codex_mcp::unregister(record),
                None => Ok(()),
            },
            Self::OpenCode => opencode_mcp::unregister(&state.opencode_plugin()),
        }
    }

    fn registered_token(self, state: &McpState) -> Result<Option<String>> {
        match self {
            Self::Claude => claude_mcp::registered_token(),
            Self::Codex => codex_mcp::registered_token(state.codex.as_ref()),
            Self::OpenCode => opencode_mcp::registered_token(&state.opencode_plugin()),
        }
    }
}

#[derive(Debug, Default)]
struct Registration {
    codex: Option<codex_mcp::Record>,
    opencode_plugin: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpState {
    #[serde(flatten)]
    binding: ConnectionBinding,
    #[serde(default)]
    token_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex: Option<codex_mcp::Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opencode_plugin: Option<PathBuf>,
}

impl McpState {
    fn opencode_plugin(&self) -> PathBuf {
        self.opencode_plugin
            .clone()
            .unwrap_or_else(opencode_mcp::plugin_path)
    }

    fn holds(&self, token: &str) -> bool {
        self.token_sha256.is_empty() || self.token_sha256 == fingerprint(token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpSource {
    Nasiko,
    WithRouting,
    DesktopApp,
}

#[derive(Debug, Clone)]
pub struct McpSummary {
    pub source: McpSource,
    pub cluster: String,
    pub cluster_url: String,
    pub principal_id: Option<String>,
    pub agent_id: String,
    pub agent_name: String,
}

impl McpSummary {
    fn from_binding(binding: &ConnectionBinding, source: McpSource) -> Self {
        Self {
            source,
            cluster: binding.cluster.clone(),
            cluster_url: binding.cluster_url.clone(),
            principal_id: binding.principal_id.clone(),
            agent_id: binding.agent_id.clone(),
            agent_name: binding.agent_name.clone(),
        }
    }

    fn from_routing(routing: &RoutingSummary) -> Self {
        Self {
            source: McpSource::WithRouting,
            cluster: routing.cluster.clone(),
            cluster_url: routing.cluster_url.clone(),
            principal_id: routing.principal_id.clone(),
            agent_id: routing.agent_id.clone(),
            agent_name: routing.agent_name.clone(),
        }
    }

    pub fn label(&self) -> String {
        match self.source {
            McpSource::Nasiko => format!("on ({})", self.cluster),
            McpSource::WithRouting => format!("on ({}, with LLM routing)", self.cluster),
            McpSource::DesktopApp => "on (desktop app)".to_string(),
        }
    }
}

fn fingerprint(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn load_state(client: McpClient) -> Result<Option<McpState>> {
    let state: Option<McpState> = coding_agent_router::read_json(&client.state_path())?;
    if let Some(state) = &state
        && state.binding.version != coding_agent_router::STATE_VERSION
    {
        bail!(
            "unsupported {} MCP gateway state version {}",
            client.spec().display_name,
            state.binding.version
        );
    }
    Ok(state)
}

fn remove_state(client: McpClient) -> Result<()> {
    crate::commands::codex::remove_if_exists(&client.state_path())
}

fn desktop_state_dir() -> PathBuf {
    std::env::var_os(DESKTOP_STATE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".nasiko-desktop")
                .join("coding-agents")
        })
}

fn desktop_summary(client: McpClient, state_dir: &Path) -> Result<Option<McpSummary>> {
    let path = state_dir
        .join("routing")
        .join(format!("{}.json", client.id()));
    let Some(state) = coding_agent_router::read_json::<Value>(&path)
        .with_context(|| "the Nasiko desktop app's state can't be read".to_string())?
    else {
        return Ok(None);
    };
    let holds = state.get("mcp_installed").and_then(Value::as_bool) == Some(true)
        || state
            .get("installed_mcp_server")
            .is_some_and(|installed| installed.is_string());
    if !holds {
        return Ok(None);
    }
    let text = |key: &str| {
        state
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(Some(McpSummary {
        source: McpSource::DesktopApp,
        cluster: text("cluster"),
        cluster_url: text("cluster_url"),
        principal_id: state
            .get("principal_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        agent_id: text("agent_id"),
        agent_name: text("agent_name"),
    }))
}

pub fn summary(client: McpClient, routing: Option<&RoutingSummary>) -> Result<Option<McpSummary>> {
    if let Some(desktop) = desktop_summary(client, &desktop_state_dir())? {
        return Ok(Some(desktop));
    }
    if let Some(state) = load_state(client)? {
        return Ok(Some(McpSummary::from_binding(
            &state.binding,
            McpSource::Nasiko,
        )));
    }
    Ok(routing
        .filter(|routing| routing.mcp_installed)
        .map(McpSummary::from_routing))
}

pub fn managed_by_desktop(client: McpClient) -> Result<bool> {
    Ok(desktop_summary(client, &desktop_state_dir())?.is_some())
}

pub fn forget_stale_state(client: McpClient) -> Result<bool> {
    if !managed_by_desktop(client)? || load_state(client)?.is_none() {
        return Ok(false);
    }
    remove_state(client)?;
    term::print_info(&format!(
        "{}'s MCP gateway is now managed by the Nasiko desktop app; this CLI no longer manages it.",
        client.spec().display_name
    ));
    Ok(true)
}

pub fn has_legacy(client: McpClient) -> Result<bool> {
    Ok(match client {
        McpClient::Claude => crate::commands::claude::legacy_mcp_binding()?.is_some(),
        McpClient::Codex => crate::commands::codex::legacy_mcp()?.is_some(),
        McpClient::OpenCode => crate::commands::opencode::legacy_mcp_binding()?.is_some(),
    })
}

pub fn adopt_legacy(client: McpClient) -> Result<bool> {
    if load_state(client)?.is_some() {
        return Ok(false);
    }
    match client {
        McpClient::Claude => {
            let Some(binding) = crate::commands::claude::legacy_mcp_binding()? else {
                return Ok(false);
            };
            let token_sha256 = claude_mcp::registered_token()?
                .map(|token| fingerprint(&token))
                .unwrap_or_default();
            write_state(
                client,
                &McpState {
                    binding,
                    token_sha256,
                    codex: None,
                    opencode_plugin: None,
                },
            )?;
            crate::commands::claude::forget_legacy_mcp()?;
        }
        McpClient::Codex => {
            let Some(legacy) = crate::commands::codex::legacy_mcp()? else {
                return Ok(false);
            };
            let record = codex_mcp::Record::adopted(&legacy);
            let token_sha256 = codex_mcp::registered_token(Some(&record))?
                .map(|token| fingerprint(&token))
                .unwrap_or_default();
            write_state(
                client,
                &McpState {
                    binding: legacy.binding,
                    token_sha256,
                    codex: Some(record),
                    opencode_plugin: None,
                },
            )?;
            crate::commands::codex::forget_legacy_mcp()?;
        }
        McpClient::OpenCode => {
            let Some(binding) = crate::commands::opencode::legacy_mcp_binding()? else {
                return Ok(false);
            };
            opencode_mcp::check(&opencode_mcp::plugin_path())?;
            let api = coding_agent_router::client_for_binding(
                &binding,
                &format!(
                    "run: {} connect opencode",
                    crate::commands::local_agents::cli_name()
                ),
            )?;
            let credential =
                coding_agent_router::mcp_credential(&api, &binding.agent_id, &binding.cluster_url)
                    .context("could not move the MCP gateway out of OpenCode's router plugin")?;
            let path = opencode_mcp::register(&credential)?;
            write_state(
                client,
                &McpState {
                    binding,
                    token_sha256: fingerprint(&credential.token),
                    codex: None,
                    opencode_plugin: Some(path),
                },
            )?;
            crate::commands::opencode::forget_legacy_mcp()?;
        }
    }
    Ok(true)
}

fn write_state(client: McpClient, state: &McpState) -> Result<()> {
    coding_agent_router::atomic_write_json(&client.state_path(), state)
}

pub fn install(client: McpClient) -> Result<()> {
    if managed_by_desktop(client)? {
        bail!(
            "{}'s MCP gateway is managed by the Nasiko desktop app; turn it off there first",
            client.spec().display_name
        );
    }
    if load_state(client)?.is_some() {
        bail!(
            "{}'s MCP gateway is already connected; remove it first with `{} disconnect {} --mcp`",
            client.spec().display_name,
            crate::commands::local_agents::cli_name(),
            client.id()
        );
    }
    client.check_local()?;
    let (cluster, entry, principal) = coding_agent_router::require_current_login()?;
    let api =
        Client::from_cluster_entry_with_timeout(&entry, Some(coding_agent_router::CP_CALL_TIMEOUT));
    let (agent_id, agent_name) =
        coding_agent_router::ensure_local_agent(&api, &client.spec(), &principal)?;
    let credential = coding_agent_router::mcp_credential(&api, &agent_id, &entry.url)
        .context("the control plane could not issue an MCP gateway credential")?;
    let binding = ConnectionBinding {
        version: coding_agent_router::STATE_VERSION,
        integration_id: Some(client.id().to_string()),
        cluster,
        cluster_url: entry.url.clone(),
        principal_id: Some(principal),
        agent_id,
        agent_name,
        executable: None,
    };
    let registration = match client.register(&credential) {
        Ok(registration) => registration,
        Err(error) => {
            coding_agent_router::revoke_mcp_credential_best_effort(&api, &binding.agent_id);
            return Err(error);
        }
    };
    let state = McpState {
        binding,
        token_sha256: fingerprint(&credential.token),
        codex: registration.codex,
        opencode_plugin: registration.opencode_plugin,
    };
    if let Err(error) = write_state(client, &state) {
        if let Err(unregister) = client.unregister(&state) {
            term::print_warning(&format!(
                "could not undo the MCP gateway registration: {}",
                coding_agent_router::one_line_warning(&unregister)
            ));
        }
        coding_agent_router::revoke_mcp_credential_best_effort(&api, &state.binding.agent_id);
        return Err(error.context("could not record the MCP gateway connection"));
    }
    if let Err(error) = enable_platform_connectors(&api, &state.binding.agent_id) {
        term::print_warning(&format!(
            "could not turn on the platform tools ({}); turn them on in the agent's MCP tab on the website",
            coding_agent_router::one_line_warning(&error)
        ));
    }
    term::print_success(&format!(
        "{} can now use your Nasiko tools (MCP gateway, agent {}).",
        client.spec().display_name,
        state.binding.agent_name
    ));
    println!(
        "Start a new {} session to load it.",
        client.spec().display_name
    );
    Ok(())
}

pub fn uninstall(client: McpClient) -> Result<bool> {
    if managed_by_desktop(client)? {
        bail!(
            "{}'s MCP gateway is managed by the Nasiko desktop app; turn it off there",
            client.spec().display_name
        );
    }
    let Some(state) = load_state(client)? else {
        return Ok(false);
    };
    client.unregister(&state)?;
    coding_agent_router::revoke_bound_mcp_credential_best_effort(&state.binding);
    remove_state(client)?;
    term::print_success(&format!(
        "Removed the Nasiko MCP gateway from {}.",
        client.spec().display_name
    ));
    println!(
        "Restart {} so it stops loading it.",
        client.spec().display_name
    );
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    Accepted,
    Rejected,
    Unreachable(String),
}

fn probe(cluster_url: &str, token: &str) -> Probe {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(PROBE_TIMEOUT))
            .http_status_as_error(false)
            .build(),
    );
    let url = format!("{}/api/mcp", cluster_url.trim_end_matches('/'));
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "nasiko-status", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    match agent
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json, text/event-stream")
        .send_json(body)
    {
        Ok(response) => match response.status().as_u16() {
            200..=299 => Probe::Accepted,
            401 | 403 => Probe::Rejected,
            status => Probe::Unreachable(format!("HTTP {status}")),
        },
        Err(error) => Probe::Unreachable(error.to_string()),
    }
}

fn disabled_platform_connectors(response: &Value) -> Result<Vec<String>> {
    let connectors = response
        .pointer("/data/connectors")
        .and_then(Value::as_array)
        .context("the control plane returned no connector list")?;
    connectors
        .iter()
        .filter(|connector| {
            connector.get("provider_type").and_then(Value::as_str) == Some("system")
        })
        .filter(|connector| connector.get("enabled").and_then(Value::as_bool) != Some(true))
        .map(|connector| {
            connector
                .get("connector_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .context("the control plane returned a connector without an id")
        })
        .collect()
}

fn enable_platform_connectors(api: &Client, agent_id: &str) -> Result<()> {
    let response: Value = api.get_json(&format!("/mcp/agents/{agent_id}/connectors"))?;
    for connector_id in disabled_platform_connectors(&response)? {
        let _: Value = api.put_json(
            &format!("/mcp/agents/{agent_id}/connectors/{connector_id}"),
            &json!({ "enabled": true }),
        )?;
    }
    Ok(())
}

pub fn print_status(client: McpClient, summary: Option<&McpSummary>) -> Result<()> {
    let value = coding_agent_router::status_value;
    let name = client.spec().display_name;
    let Some(summary) = summary else {
        println!("{name} MCP gateway:   {}", value("off"));
        println!(
            "Connect with: {}",
            term::cyan(&format!(
                "{} connect {} --no-routing --mcp",
                crate::commands::local_agents::cli_name(),
                client.id()
            ))
        );
        return Ok(());
    };
    println!("{name} MCP gateway:   {}", value(&summary.label()));
    if summary.source == McpSource::DesktopApp {
        println!("Managed by:        the Nasiko desktop app");
        return Ok(());
    }
    println!(
        "Cluster:           {} ({})",
        summary.cluster, summary.cluster_url
    );
    println!("Agent:             {}", summary.agent_name);
    let state = load_state(client)?;
    let token = match &state {
        Some(state) => client.registered_token(state)?,
        None => None,
    };
    let registration = match (&state, &token) {
        (None, _) => "configured",
        (Some(_), None) => "missing",
        (Some(state), Some(token)) if state.holds(token) => "configured",
        (Some(_), Some(_)) => "changed since connect",
    };
    println!("Registration:      {}", value(registration));
    if let Some(token) = token.as_deref().filter(|_| registration == "configured") {
        let credential = match probe(&summary.cluster_url, token) {
            Probe::Accepted => value("accepted"),
            Probe::Rejected => format!(
                "{} — reconnect with: {}",
                value("rejected"),
                term::cyan(&format!(
                    "{cli} disconnect {id} --mcp && {cli} connect {id} --no-routing --mcp",
                    cli = crate::commands::local_agents::cli_name(),
                    id = client.id()
                ))
            ),
            Probe::Unreachable(reason) => format!("{} ({reason})", value("unreachable")),
        };
        println!("Credential:        {credential}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ConnectionBinding {
        ConnectionBinding {
            version: coding_agent_router::STATE_VERSION,
            integration_id: Some("claude".into()),
            cluster: "local".into(),
            cluster_url: "https://cp.example".into(),
            principal_id: Some("owner".into()),
            agent_id: "agent".into(),
            agent_name: "owner-claude-code".into(),
            executable: None,
        }
    }

    #[test]
    fn every_supported_coding_agent_maps_to_one_client_and_back() {
        for client in [McpClient::Claude, McpClient::Codex, McpClient::OpenCode] {
            assert_eq!(McpClient::for_id(client.id()), Some(client));
        }
        assert_eq!(McpClient::for_id("cursor"), None);
        assert_eq!(McpClient::for_id(""), None);
    }

    #[test]
    fn the_rows_match_the_routing_integrations() {
        assert_eq!(McpClient::Claude.spec().default_name, "claude-code");
        assert_eq!(McpClient::Codex.spec().default_name, "codex");
        assert_eq!(McpClient::OpenCode.spec().default_name, "opencode");
    }

    #[test]
    fn state_paths_never_collide_with_routing_or_reporting_state() {
        for (client, file) in [
            (McpClient::Claude, "claude-mcp.json"),
            (McpClient::Codex, "codex-mcp.json"),
            (McpClient::OpenCode, "opencode-mcp.json"),
        ] {
            assert!(client.state_path().ends_with(file));
        }
    }

    #[test]
    fn state_round_trips_and_keeps_only_a_fingerprint_of_the_token() {
        let state = McpState {
            binding: binding(),
            token_sha256: fingerprint("ngt_secret"),
            codex: None,
            opencode_plugin: None,
        };
        let text = serde_json::to_string(&state).unwrap();
        assert!(!text.contains("ngt_secret"));
        assert!(!text.contains("codex"));
        assert!(!text.contains("opencode_plugin"));
        let read: McpState = serde_json::from_str(&text).unwrap();
        assert_eq!(read.binding.agent_id, "agent");
        assert!(read.holds("ngt_secret"));
        assert!(!read.holds("ngt_other"));
    }

    #[test]
    fn a_state_without_a_fingerprint_holds_any_registration() {
        let state: McpState = serde_json::from_value(json!({
            "version": coding_agent_router::STATE_VERSION,
            "cluster": "local",
            "cluster_url": "https://cp.example",
            "agent_id": "agent",
            "agent_name": "owner-claude-code"
        }))
        .unwrap();
        assert!(state.holds("ngt_anything"));
    }

    #[test]
    fn fingerprint_is_the_hex_sha256_of_the_token() {
        assert_eq!(
            fingerprint("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn routing(mcp_installed: bool) -> RoutingSummary {
        RoutingSummary {
            cluster: "local".into(),
            cluster_url: "https://cp.example".into(),
            principal_id: Some("owner".into()),
            agent_id: "agent".into(),
            agent_name: "owner-claude-code".into(),
            mcp_installed,
        }
    }

    #[test]
    fn mcp_set_up_with_routing_is_labelled_as_such() {
        let summary = McpSummary::from_routing(&routing(true));
        assert_eq!(summary.source, McpSource::WithRouting);
        assert_eq!(summary.label(), "on (local, with LLM routing)");
        assert_eq!(
            McpSummary::from_binding(&binding(), McpSource::Nasiko).label(),
            "on (local)"
        );
    }

    fn desktop_dir_with(id: &str, state: Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let routing = dir.path().join("routing");
        std::fs::create_dir_all(&routing).unwrap();
        std::fs::write(
            routing.join(format!("{id}.json")),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_desktop_app_holds_mcp_only_when_its_state_says_so() {
        let dir = desktop_dir_with(
            "claude",
            json!({"cluster": "cp", "cluster_url": "https://cp", "agent_id": "a", "agent_name": "n", "mcp_installed": true}),
        );
        let summary = desktop_summary(McpClient::Claude, dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(summary.source, McpSource::DesktopApp);
        assert_eq!(summary.label(), "on (desktop app)");
        assert!(
            desktop_summary(McpClient::Codex, dir.path())
                .unwrap()
                .is_none()
        );

        let without = desktop_dir_with("claude", json!({"mcp_installed": false}));
        assert!(
            desktop_summary(McpClient::Claude, without.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_desktop_app_holds_codex_mcp_through_its_installed_item() {
        let held = desktop_dir_with(
            "codex",
            json!({"installed_mcp_server": "saved = { url = \"x\" }"}),
        );
        assert!(
            desktop_summary(McpClient::Codex, held.path())
                .unwrap()
                .is_some()
        );
        let not_held = desktop_dir_with("codex", json!({"installed_mcp_server": null}));
        assert!(
            desktop_summary(McpClient::Codex, not_held.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_unreadable_desktop_state_is_an_error_not_a_silent_no() {
        let dir = tempfile::tempdir().unwrap();
        let routing = dir.path().join("routing");
        std::fs::create_dir_all(&routing).unwrap();
        std::fs::write(routing.join("claude.json"), b"{not json").unwrap();
        assert!(desktop_summary(McpClient::Claude, dir.path()).is_err());
    }

    #[test]
    fn probe_reads_the_gateway_answer() {
        let mut server = mockito::Server::new();
        let accepted = server
            .mock("POST", "/api/mcp")
            .match_header("authorization", "Bearer ngt_good")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
            .create();
        assert_eq!(probe(&server.url(), "ngt_good"), Probe::Accepted);
        accepted.assert();
        server
            .mock("POST", "/api/mcp")
            .match_header("authorization", "Bearer ngt_bad")
            .with_status(401)
            .create();
        assert_eq!(
            probe(&format!("{}/", server.url()), "ngt_bad"),
            Probe::Rejected
        );
        server
            .mock("POST", "/api/mcp")
            .match_header("authorization", "Bearer ngt_down")
            .with_status(502)
            .create();
        assert_eq!(
            probe(&server.url(), "ngt_down"),
            Probe::Unreachable("HTTP 502".into())
        );
        assert!(matches!(
            probe("http://127.0.0.1:1", "ngt_any"),
            Probe::Unreachable(_)
        ));
    }

    fn connector_view(body: &str) -> (mockito::ServerGuard, Client) {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/api/mcp/agents/agent/connectors")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();
        let api = Client::for_test(&server.url(), Some("jwt"));
        (server, api)
    }

    #[test]
    fn only_disabled_platform_connectors_are_picked() {
        let view = json!({"data": {"connectors": [
            {"connector_id": "on", "provider_type": "system", "name": "workspace", "enabled": true},
            {"connector_id": "off", "provider_type": "system", "name": "platform-control", "enabled": false},
            {"connector_id": "unset", "provider_type": "system", "name": "other"},
            {"connector_id": "spoof", "provider_type": "mcp_server", "name": "platform-control", "enabled": false},
            {"connector_id": "app", "provider_type": "composio", "name": "github", "enabled": false},
            {"connector_id": "untyped", "name": "platform-control", "enabled": false}
        ]}});
        assert_eq!(
            disabled_platform_connectors(&view).unwrap(),
            vec!["off".to_string(), "unset".to_string()]
        );
    }

    #[test]
    fn a_malformed_connector_view_is_an_error() {
        assert!(disabled_platform_connectors(&json!({"data": {}})).is_err());
        assert!(disabled_platform_connectors(&json!({"data": {"connectors": {}}})).is_err());
        assert!(
            disabled_platform_connectors(&json!({"data": {"connectors": [
                {"provider_type": "system", "enabled": false}
            ]}}))
            .is_err()
        );
        assert!(
            disabled_platform_connectors(&json!({"data": {"connectors": []}}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn connecting_turns_on_every_disabled_platform_connector() {
        let (mut server, api) = connector_view(
            r#"{"data":{"connectors":[
                {"connector_id":"w","provider_type":"system","name":"workspace","enabled":true},
                {"connector_id":"p","provider_type":"system","name":"platform-control","enabled":false},
                {"connector_id":"s","provider_type":"mcp_server","name":"platform-control","enabled":false}
            ]}}"#,
        );
        let enabled = server
            .mock("PUT", "/api/mcp/agents/agent/connectors/p")
            .match_body(mockito::Matcher::Json(json!({"enabled": true})))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"connector_id":"p","enabled":true}}"#)
            .expect(1)
            .create();
        let untouched = server
            .mock("PUT", mockito::Matcher::Regex("/connectors/(w|s)$".into()))
            .expect(0)
            .create();
        enable_platform_connectors(&api, "agent").unwrap();
        enabled.assert();
        untouched.assert();
    }

    #[test]
    fn nothing_is_written_when_the_platform_tools_are_already_on_or_absent() {
        for body in [
            r#"{"data":{"connectors":[{"connector_id":"p","provider_type":"system","enabled":true}]}}"#,
            r#"{"data":{"connectors":[]}}"#,
        ] {
            let (mut server, api) = connector_view(body);
            let writes = server.mock("PUT", mockito::Matcher::Any).expect(0).create();
            enable_platform_connectors(&api, "agent").unwrap();
            writes.assert();
        }
    }

    #[test]
    fn a_refused_enable_or_an_unreadable_view_is_reported() {
        let (mut server, api) = connector_view(
            r#"{"data":{"connectors":[{"connector_id":"p","provider_type":"system","enabled":false}]}}"#,
        );
        server
            .mock("PUT", "/api/mcp/agents/agent/connectors/p")
            .with_status(403)
            .create();
        assert!(enable_platform_connectors(&api, "agent").is_err());

        let mut failing = mockito::Server::new();
        failing
            .mock("GET", "/api/mcp/agents/agent/connectors")
            .with_status(500)
            .create();
        let api = Client::for_test(&failing.url(), Some("jwt"));
        assert!(enable_platform_connectors(&api, "agent").is_err());
    }
}
