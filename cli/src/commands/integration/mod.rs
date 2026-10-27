//! Detect local coding agents and report their sessions to Nasiko.
//!
//! [`local_agents`](super::local_agents) answers "what coding agents are on this machine",
//! "what A2A agents are running on this machine" and drives setup.
//! [`install`] registers an agent in the control plane (or reuses the one LLM
//! routing already uses) and installs a hook that durably queues completed turns for delivery
//! to the cluster and account reporting was installed for — not whichever is active later.

mod a2a;
mod agents;
mod catalog;
mod control_plane;
mod launcher;
mod model;
mod queue;
mod report;
mod state;
mod sync;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
use std::path::Path;

use agents::Agent;
use catalog::Support;
use state::{AgentState, InstallationBinding, IntegrationState};

/// Options accepted by `nasiko agents install`.
pub struct InstallOptions<'a> {
    pub agent_id: &'a str,
    /// Keep prompt and response text out of exported spans.
    pub no_content: bool,
    /// Report under this already-registered agent (the one LLM routing uses) instead of
    /// registering the account's default integration agent.
    pub agent_name: Option<&'a str>,
}

/// One catalogued coding agent as `nasiko connect`/`nasiko status` present it.
#[derive(Debug, Clone)]
pub struct LocalAgent {
    pub id: &'static str,
    pub display_name: &'static str,
    pub detected: bool,
    /// Whether session reporting is implemented for this agent.
    pub reporting_supported: bool,
    /// Whether the user said no when reporting was offered after logging in, or removed it.
    pub reporting_declined: bool,
    /// The control-plane agent reporting is installed under, when installed.
    pub reporting_agent_name: Option<String>,
    pub reporting_active: bool,
    pub reporting_installed: bool,
    pub reporting_status: String,
    pub capture_content: bool,
    pub hook_location: String,
    /// Where installed reporting delivers events; `None` when not installed, or for legacy
    /// state recorded before installs were bound to a cluster (which must be reinstalled).
    pub reporting_destination: Option<ReportingDestination>,
}

/// The cluster and user an installed reporting hook delivers events for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingDestination {
    pub cluster_name: String,
    pub cluster_url: String,
    pub principal_id: String,
}

impl ReportingDestination {
    /// Where reporting would be bound if installed now: the active cluster and its login.
    pub fn current() -> Result<Self> {
        let binding = control_plane::installation_binding()?;
        Ok(Self::from(&binding))
    }

    pub fn same_as(&self, other: &Self) -> bool {
        self.is(
            &other.cluster_name,
            &other.cluster_url,
            Some(&other.principal_id),
        )
    }

    /// Whether this is `cluster_name` at `cluster_url` for `principal_id`. A `None` principal —
    /// a binding recorded before the login was stored with it — matches any user there.
    pub fn is(&self, cluster_name: &str, cluster_url: &str, principal_id: Option<&str>) -> bool {
        self.cluster_name == cluster_name
            && self.cluster_url.trim_end_matches('/') == cluster_url.trim_end_matches('/')
            && principal_id.is_none_or(|principal| {
                crate::config::same_principal(&self.principal_id, principal)
            })
    }
}

impl From<&InstallationBinding> for ReportingDestination {
    fn from(binding: &InstallationBinding) -> Self {
        Self {
            cluster_name: binding.cluster_name.clone(),
            cluster_url: binding.cluster_url.clone(),
            principal_id: binding.principal_id.to_string(),
        }
    }
}

/// Every catalogued coding agent with its detection and reporting state.
pub fn local_agents() -> Result<Vec<LocalAgent>> {
    let settings = IntegrationState::load()?;
    Ok(Agent::ALL
        .iter()
        .copied()
        .map(|agent| {
            let spec = agent.spec();
            let reporting = reporting_details(agent, &settings);
            let state = settings.get(spec.id);
            LocalAgent {
                id: spec.id,
                display_name: spec.display_name,
                detected: detect(agent).is_present(),
                reporting_supported: spec.support == Support::Instrumented,
                reporting_declined: settings.declined.contains(spec.id),
                reporting_agent_name: state.map(|state| state.agent_name.clone()),
                reporting_active: reporting.connected,
                reporting_installed: state.is_some(),
                reporting_status: reporting.status,
                capture_content: state.is_none_or(|state| state.capture_content),
                hook_location: tildify(&agent.config_path()),
                reporting_destination: state
                    .and_then(|state| state.binding.as_ref())
                    .map(ReportingDestination::from),
            }
        })
        .collect())
}

/// Whether `id` names a catalogued coding agent.
pub fn is_known_agent(id: &str) -> bool {
    catalog::find(id).is_some()
}

/// Comma-separated ids of every catalogued coding agent.
pub fn known_agent_ids() -> String {
    catalog::known_ids()
}

/// Queued coding-agent events not yet delivered to any cluster, including ones waiting to be
/// retried. Read-only: unlike a sync, counting never moves or rewrites a queued file.
pub fn pending_event_count() -> Result<usize> {
    queue::queued_count()
}

// ─── Commands ────────────────────────────────────────────────────────────────

pub use a2a::DiscoveredAgent as A2aAgent;
pub use a2a::env_file::EnvTarget;
pub use control_plane::RegisteredA2aAgent;

/// The A2A agents running on this machine, and what discovery could not look at.
pub struct RunningA2aAgents {
    pub agents: Vec<A2aAgent>,
    pub warnings: Vec<String>,
}

pub fn running_a2a_agents() -> RunningA2aAgents {
    let discovery = a2a::discover_local(&local_control_plane_ports());
    RunningA2aAgents {
        agents: discovery.agents,
        warnings: discovery.warnings,
    }
}

/// `agents discover`: every catalogued coding agent and every A2A agent running on this machine
/// in one table (whether each is here, and whether it is connected to Nasiko), then a warning
/// per invalid card and per source that could not be read.
///
/// When logged in, also registers the running A2A agents with the active cluster (agents no
/// longer running are marked stopped there), which is what their CONNECTED column reports on.
///
/// Its own command, not part of `status`: it lists sockets and containers and sends HTTP
/// probes to every listening port, which takes time and shows up in those services' logs.
pub fn discover(cli: &str) -> Result<()> {
    let settings = IntegrationState::load()?;
    let running = running_a2a_agents();
    let registered = match register_running(&running) {
        Ok(registered) => registered,
        Err(error) => {
            term::print_warning(&format!(
                "A2A agents were not registered with Nasiko: {error:#}"
            ));
            Vec::new()
        }
    };
    let connections = a2a::connections::Connections::load().unwrap_or_default();

    let mut rows: Vec<DiscoverRow> = Agent::ALL
        .iter()
        .copied()
        .map(|agent| coding_agent_row(agent, &settings))
        .collect();
    rows.extend(
        running
            .agents
            .iter()
            .map(|agent| a2a_agent_row(agent, llm_routing_state(agent, &registered, &connections))),
    );
    print!("{}", format_discover_table(&rows));

    println!("\nInstall session reporting:  {cli} connect <agent> --reporting");

    for agent in &running.agents {
        if let Err(errors) = &agent.card {
            for error in errors {
                term::print_warning(&format!(
                    "{} serves an invalid agent card: {error}",
                    agent.name
                ));
            }
        }
    }
    for warning in &running.warnings {
        term::print_warning(&format!("{warning}\n  Retry with: {cli} agents discover"));
    }
    Ok(())
}

/// Register every running agent with a valid card under this machine. Skipped (empty) when
/// not logged in, when the machine has no name, or when a discovery source failed: a partial
/// list would mark agents stopped that are still running.
pub fn register_running(running: &RunningA2aAgents) -> Result<Vec<RegisteredA2aAgent>> {
    if !running.warnings.is_empty() || crate::config::active_cluster().is_err() {
        return Ok(Vec::new());
    }
    let Some(machine) = a2a::machine_name() else {
        return Ok(Vec::new());
    };
    let cards: Vec<serde_json::Value> = running
        .agents
        .iter()
        .filter(|agent| agent.card.is_ok())
        .map(|agent| agent.raw_card.clone())
        .collect();
    Ok(control_plane::register_discovered(&machine, &cards)?.unwrap_or_default())
}

/// Whether an A2A agent's LLM calls go through Nasiko.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmRouting {
    /// Not connected (or not registered, so it cannot be).
    Off,
    /// `connect` pointed it at Nasiko; no routed call has arrived yet (it needs a restart).
    Waiting,
    /// The router has seen a call with its key since `connect`.
    On,
}

impl LlmRouting {
    /// As the CONNECTED column shows it, beside the coding agents' `yes`/`no`.
    fn connected(self) -> &'static str {
        match self {
            Self::Off => "no",
            Self::Waiting => "waiting",
            Self::On => "yes",
        }
    }
}

fn llm_routing_state(
    agent: &A2aAgent,
    registered: &[RegisteredA2aAgent],
    connections: &a2a::connections::Connections,
) -> LlmRouting {
    match registered
        .iter()
        .find(|entry| entry.card_name == agent.name)
    {
        // Only `connect` points an agent at Nasiko; after `disconnect` its past calls no longer count.
        Some(entry) if connections.agents.contains_key(&entry.id) => {
            if entry.last_llm_call_at.is_some() {
                LlmRouting::On
            } else {
                LlmRouting::Waiting
            }
        }
        _ => LlmRouting::Off,
    }
}

/// One line of the `agents discover` table.
struct DiscoverRow {
    agent: String,
    detected: bool,
    connected: &'static str,
    version: String,
    config: String,
    /// Where an A2A agent serves its agent card; `None` for coding agents.
    a2a_card: Option<String>,
}

fn coding_agent_row(agent: Agent, settings: &IntegrationState) -> DiscoverRow {
    let reporting = reporting_details(agent, settings);
    DiscoverRow {
        agent: agent.spec().id.to_string(),
        detected: detect(agent).is_present(),
        connected: if reporting.connected { "yes" } else { "no" },
        version: reporting.version,
        config: tildify(&agent.config_path()),
        a2a_card: None,
    }
}

/// A running A2A agent: found by its card, so always detected; connected once its LLM calls
/// go through Nasiko. A2A CARD is the card URL as fetched from this machine — not the card's
/// own `url`, which inside a Docker container names the unpublished port.
fn a2a_agent_row(agent: &A2aAgent, routing: LlmRouting) -> DiscoverRow {
    DiscoverRow {
        agent: agent.name.clone(),
        detected: true,
        connected: routing.connected(),
        version: agent.version().unwrap_or("-").to_string(),
        config: "-".to_string(),
        a2a_card: Some(agent.card_url.clone()),
    }
}

/// With only coding agents the table is exactly the one `agents discover` always printed. A2A
/// agents widen the AGENT column for long names and add an A2A CARD column, which needs CONFIG
/// padded to a fixed width.
fn format_discover_table(rows: &[DiscoverRow]) -> String {
    let width = rows
        .iter()
        .map(|row| row.agent.len())
        .max()
        .unwrap_or(0)
        .max(10);
    let has_cards = rows.iter().any(|row| row.a2a_card.is_some());
    let config_width = rows
        .iter()
        .map(|row| row.config.len())
        .max()
        .unwrap_or(0)
        .max("CONFIG".len());

    let mut table = format!(
        "{:<width$} {:<10} {:<11} {:<20} ",
        "AGENT", "DETECTED", "CONNECTED", "VERSION"
    );
    let mut rule_len = 88 + width - 10;
    if has_cards {
        table.push_str(&format!("{:<config_width$} A2A CARD\n", "CONFIG"));
        rule_len += config_width + 1;
    } else {
        table.push_str("CONFIG\n");
    }
    table.push_str(&format!("{}\n", "-".repeat(rule_len)));
    for row in rows {
        table.push_str(&format!(
            "{:<width$} {:<10} {:<11} {:<20} ",
            row.agent,
            if row.detected { "yes" } else { "-" },
            row.connected,
            row.version,
        ));
        if has_cards {
            let card = row.a2a_card.as_deref().unwrap_or("-");
            table.push_str(&format!("{:<config_width$} {card}\n", row.config));
        } else {
            table.push_str(&format!("{}\n", row.config));
        }
    }
    table
}

// ─── Connecting an A2A agent ─────────────────────────────────────────────────

/// What `connect` does for one running A2A agent: its catalog identity, the variables that
/// route its LLM calls through Nasiko, and where they go.
pub struct A2aConnection {
    pub agent_id: uuid::Uuid,
    pub card_name: String,
    /// Its catalog name.
    pub name: String,
    pub vars: Vec<(&'static str, String)>,
    /// The agent's working folder, when known.
    pub folder: Option<std::path::PathBuf>,
    /// The file to write them to, or why the user must set them.
    pub target: EnvTarget,
}

/// Register the running agents, then prepare `agent`'s connection: its key and the file
/// decided from its process (see [`a2a::env_file`]).
pub fn prepare_a2a_connection(
    agent: &A2aAgent,
    running: &RunningA2aAgents,
) -> Result<A2aConnection> {
    if let Err(issues) = &agent.card {
        let issues: Vec<String> = issues.iter().map(ToString::to_string).collect();
        bail!(
            "{} serves an invalid agent card, so it can't be registered: {}",
            agent.name,
            issues.join("; ")
        );
    }
    let registered = register_running(running)?;
    let entry = registered
        .iter()
        .find(|entry| entry.card_name == agent.name)
        .with_context(|| {
            format!(
                "{} was not registered (is this machine's hostname readable, and did discovery finish without warnings?)",
                agent.name
            )
        })?;
    let (key, cluster_url) = control_plane::routing_key(entry.id)?;
    let (folder, target) = match &agent.origin {
        a2a::EndpointOrigin::Docker { container } => (
            None,
            EnvTarget::Unknown(format!(
                "it runs in the Docker container {container}; set them in its container settings"
            )),
        ),
        a2a::EndpointOrigin::Process { pid: None, .. } => (
            None,
            EnvTarget::Unknown("its process could not be identified".to_string()),
        ),
        a2a::EndpointOrigin::Process { pid: Some(pid), .. } => {
            let facts = a2a::process::inspect(*pid);
            let target = match a2a::env_file::locate(&facts) {
                EnvTarget::File(file) => {
                    let folder = facts.cwd.clone().unwrap_or_default();
                    match a2a::env_file::refusal(&file, &folder) {
                        Some(reason) => EnvTarget::Unknown(reason),
                        None => EnvTarget::File(file),
                    }
                }
                unknown => unknown,
            };
            (facts.cwd, target)
        }
    };
    Ok(A2aConnection {
        agent_id: entry.id,
        card_name: agent.name.clone(),
        name: entry.name.clone(),
        vars: a2a::env_file::routing_vars(&cluster_url, &key),
        folder,
        target,
    })
}

/// The text `file` will hold once connected, for showing and writing.
pub fn a2a_connected_text(connection: &A2aConnection, file: &Path, header: &str) -> Result<String> {
    let current = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    Ok(a2a::env_file::with_block(
        &current,
        header,
        &connection.vars,
    ))
}

/// Record the connection, writing `text` to `file` first when there is one.
pub fn save_a2a_connection(connection: &A2aConnection, file: Option<(&Path, &str)>) -> Result<()> {
    if let Some((file, text)) = file {
        a2a::env_file::write(file, text)
            .with_context(|| format!("cannot write {}", file.display()))?;
    }
    let mut connections = a2a::connections::Connections::load()?;
    connections.agents.insert(
        connection.agent_id,
        a2a::connections::Connection {
            card_name: connection.card_name.clone(),
            name: connection.name.clone(),
            file: file.map(|(file, _)| file.to_path_buf()),
        },
    );
    connections.save()
}

/// Undo `connect` for the A2A agent named `name`: remove Nasiko's block from the file it
/// wrote and forget the connection. `Ok(None)` when no A2A agent of that name is connected.
pub fn disconnect_a2a_agent(name: &str) -> Result<Option<String>> {
    let mut connections = a2a::connections::Connections::load()?;
    let Some((id, connection)) = connections.find(name) else {
        return Ok(None);
    };
    let message = match connection.file.clone() {
        Some(file) => match std::fs::read_to_string(&file) {
            Ok(text) if a2a::env_file::has_block(&text) => {
                a2a::env_file::write(&file, &a2a::env_file::without_block(&text))
                    .with_context(|| format!("cannot write {}", file.display()))?;
                format!("Removed the Nasiko variables from {}", file.display())
            }
            _ => format!("{} no longer holds the Nasiko variables", file.display()),
        },
        None => "Remove the Nasiko variables you set for it".to_string(),
    };
    connections.agents.remove(&id);
    connections.save()?;
    Ok(Some(message))
}

/// Port of the active cluster when it runs on this machine, so a local
/// control plane is never reported as an agent.
fn local_control_plane_ports() -> Vec<u16> {
    let Ok((_, entry)) = crate::config::active_cluster() else {
        return Vec::new();
    };
    local_port_of(&entry.url).into_iter().collect()
}

/// The port of a loopback URL (`http://localhost:8443` → 8443); `None` for
/// remote hosts or URLs without an explicit port.
fn local_port_of(url: &str) -> Option<u16> {
    let authority = url.split("://").nth(1)?.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    let is_loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0");
    is_loopback.then(|| port.parse().ok()).flatten()
}

/// Register a coding agent with the control plane and install its hook.
pub fn install(options: InstallOptions<'_>) -> Result<()> {
    let agent = resolve(options.agent_id)?;
    require_instrumentable(agent)?;
    require_present(agent)?;

    let spec = agent.spec();
    let registration = match options.agent_name {
        Some(agent_name) => {
            let registration = control_plane::Registration {
                created: false,
                agent_name: agent_name.to_string(),
                binding: control_plane::installation_binding()?,
            };
            term::print_info(&format!(
                "Reporting under agent '{}' (the same agent LLM routing uses)",
                registration.agent_name
            ));
            registration
        }
        None => {
            let registration = control_plane::register_agent(agent)?;
            term::print_success(&format!(
                "{} agent '{}' in the control plane",
                if registration.created {
                    "Registered"
                } else {
                    "Found"
                },
                registration.agent_name
            ));
            registration
        }
    };

    let artifacts = agent.install()?;
    let (script, registration) = persist_installed_artifacts(
        artifacts,
        || {
            save_agent_state(
                agent,
                &registration.agent_name,
                !options.no_content,
                &registration.binding,
            )
        },
        || agent.uninstall(),
    )?;

    term::print_success(&format!("Installed hook              {}", tildify(&script)));
    term::print_success(&format!(
        "Installed integration       {}",
        tildify(&registration)
    ));
    println!(
        "\nStart a new {} session, then: {}",
        spec.display_name,
        term::cyan(crate::commands::local_agents::sessions_hint())
    );
    print_restart_hint(agent);
    Ok(())
}

/// Remove a coding agent's hook. The registered agent and its past traces are
/// left alone — deleting them is a separate, destructive decision.
pub fn uninstall(agent_id: &str) -> Result<()> {
    let agent = resolve(agent_id)?;
    let spec = agent.spec();
    agent.uninstall()?;

    let mut settings = IntegrationState::load()?;
    // The agent reporting was installed under — the routing agent or an account-scoped name —
    // not the catalog default.
    let agent_name = settings
        .agents
        .remove(spec.id)
        .map_or_else(|| spec.agent_name.to_string(), |state| state.agent_name);
    // Removing reporting is a no: it isn't offered again after the next login.
    settings.declined.insert(spec.id.to_string());
    settings.save()?;

    term::print_success(&format!("Removed the {} hook.", spec.display_name));
    println!(
        "Agent '{agent_name}' is still registered; its past sessions remain in observability."
    );
    Ok(())
}

/// Hook entry point — see [`report`].
pub fn report(agent_id: &str) -> Result<()> {
    report::run(resolve(agent_id)?)
}

pub fn sync() -> Result<()> {
    sync::run()
}

/// Remembers that the user said no when reporting was offered for these agents after logging
/// in, so it isn't offered again.
pub fn record_declined(agent_ids: &[&str]) -> Result<()> {
    let mut settings = IntegrationState::load()?;
    settings
        .declined
        .extend(agent_ids.iter().map(|id| id.to_string()));
    settings.save()
}

/// Rewrites every detected agent's outdated reporting hook in place, so a hook fix reaches
/// existing installs without a reinstall. Agents with no reporting state are left to the
/// reporting offer. One broken adapter must not block the others.
fn refresh_outdated_hooks() -> Result<()> {
    let settings = IntegrationState::load()?;
    let agents = Agent::ALL
        .iter()
        .copied()
        .filter(|agent| agent.spec().support == Support::Instrumented)
        .filter(|agent| detect(*agent).is_present());
    for agent in agents {
        let spec = agent.spec();
        let state = settings.get(spec.id);
        let expected = agent.install_version().expect("instrumented adapter");
        let AutoSetup::Refresh(binding) =
            auto_setup_action(state, agent.installed_version(), expected)
        else {
            continue;
        };
        let state = state.expect("refresh implies installed state");
        match refresh_hook(agent, state, binding) {
            Ok(()) => {
                term::print_success(&format!(
                    "{} reporting hook updated to v{expected}; still reporting to {}.",
                    spec.display_name, binding.cluster_name
                ));
                print_restart_hint(agent);
            }
            Err(error) => term::print_warning(&format!(
                "Updating the {} hook failed ({error:#}). Retry with: {}",
                spec.display_name,
                term::cyan_stderr(&format!(
                    "{} connect {} --no-routing --reporting",
                    crate::commands::local_agents::cli_name(),
                    spec.id
                ))
            )),
        }
    }
    Ok(())
}

/// What setup after login does with one detected agent's reporting hook.
#[derive(Debug, PartialEq, Eq)]
enum AutoSetup<'a> {
    /// Nothing installed yet: left to the reporting offer, which asks first.
    Install,
    /// Installed and current: leave it, binding included.
    Keep,
    /// Installed but its hook is outdated: rewrite the hook, keep this binding.
    Refresh(&'a InstallationBinding),
}

/// An outdated hook is rewritten in place so a fix reaches existing installs,
/// but the binding is never moved — switching clusters stays an explicit
/// `nasiko connect <agent> --reporting`. Legacy state without a binding is left
/// for that explicit connect too, since there is no destination to preserve.
fn auto_setup_action(
    state: Option<&AgentState>,
    installed_version: Option<u32>,
    expected: u32,
) -> AutoSetup<'_> {
    let Some(state) = state else {
        return AutoSetup::Install;
    };
    let is_current = installed_version == Some(expected) && state.hook_version == expected;
    match &state.binding {
        Some(binding) if !is_current => AutoSetup::Refresh(binding),
        _ => AutoSetup::Keep,
    }
}

/// Rewrite an agent's hook at the current version without touching its
/// control-plane registration, content setting, or cluster binding.
fn refresh_hook(agent: Agent, state: &AgentState, binding: &InstallationBinding) -> Result<()> {
    let artifacts = agent.install()?;
    persist_installed_artifacts(
        artifacts,
        || save_agent_state(agent, &state.agent_name, state.capture_content, binding),
        || agent.uninstall(),
    )?;
    Ok(())
}

fn print_restart_hint(agent: Agent) {
    if agent.restart_required() {
        term::print_info("Restart OpenCode first so it loads the new plugin.");
    }
    if agent == Agent::Codex {
        term::print_info("In Codex, run /hooks and trust the new Nasiko command hooks.");
    }
}

/// After logging in or switching control plane: refreshes outdated reporting hooks, then offers
/// session reporting, when the active cluster has a usable login.
pub fn auto_install_if_authenticated() {
    let Ok((cluster, entry)) = crate::config::active_cluster() else {
        return;
    };
    let Some(token) = entry.token.as_deref() else {
        term::print_info(&format!(
            "Coding-agent setup skipped for {cluster}: not authenticated. Run `{}`.",
            term::cyan(crate::commands::local_agents::login_hint())
        ));
        return;
    };
    if crate::config::token_expired(token) == Some(true) {
        term::print_info(&format!(
            "Coding-agent setup skipped for {cluster}: login expired. Run `{}`.",
            term::cyan(crate::commands::local_agents::login_hint())
        ));
        return;
    }
    // Before the offer, so its notices don't call a hook stale that was just refreshed.
    if let Err(error) = refresh_outdated_hooks() {
        term::print_warning(&format!("Reporting hooks were not refreshed ({error:#})."));
    }
    if let Err(error) = crate::commands::local_agents::offer_reporting() {
        term::print_warning(&format!(
            "Session reporting was not set up ({error:#}). Turn it on with: {}",
            term::cyan_stderr(&format!(
                "{} connect <agent> --no-routing --reporting",
                crate::commands::local_agents::cli_name()
            ))
        ));
    }
}

// ─── Detection ───────────────────────────────────────────────────────────────

/// What was found on disk for one agent.
struct Detection {
    binary_on_path: bool,
    config_dir_exists: bool,
}

impl Detection {
    /// Either signal is enough: a config directory without the binary means
    /// the agent was used here before, and a binary without a config directory
    /// means it has been installed but not yet run.
    fn is_present(&self) -> bool {
        self.binary_on_path || self.config_dir_exists
    }
}

fn detect(agent: Agent) -> Detection {
    let spec = agent.spec();
    Detection {
        binary_on_path: which::which(spec.binary).is_ok(),
        config_dir_exists: agent.config_path().is_dir(),
    }
}

struct ReportingDetails {
    connected: bool,
    /// The installed hook's version, for `agents discover`'s VERSION column.
    version: String,
    status: String,
}

fn reporting_details(agent: Agent, settings: &IntegrationState) -> ReportingDetails {
    let spec = agent.spec();
    if spec.support == Support::DetectOnly {
        return ReportingDetails {
            connected: false,
            version: "-".into(),
            status: Support::DetectOnly.label().to_string(),
        };
    }
    let expected = agent
        .install_version()
        .expect("instrumented adapter has a version");
    let Some(state) = settings.get(spec.id) else {
        return ReportingDetails {
            connected: false,
            version: "-".into(),
            status: "not installed".into(),
        };
    };
    match agent.installed_version() {
        Some(version) if version == expected => match state.hook_version {
            version if version == expected => ReportingDetails {
                connected: true,
                version: format!("v{version}"),
                status: format!("active (v{version})"),
            },
            // Script and saved state are from different install versions.
            _ => ReportingDetails {
                connected: false,
                version: format!("v{version} (state v{})", state.hook_version),
                status: "needs reinstall".into(),
            },
        },
        Some(version) => ReportingDetails {
            connected: false,
            version: format!("v{version} -> v{expected}"),
            status: format!("stale (v{version}; current v{expected})"),
        },
        None => ReportingDetails {
            connected: false,
            version: "-".into(),
            status: "not installed".into(),
        },
    }
}

fn reporting_status(agent: Agent, settings: &IntegrationState) -> String {
    reporting_details(agent, settings).status
}

/// One adapter's session-reporting status for router-specific status commands.
/// LLM routing and telemetry remain separate lifecycles.
pub(crate) fn reporting_status_for(agent_id: &str) -> Result<String> {
    let agent = resolve(agent_id)?;
    Ok(reporting_status(agent, &IntegrationState::load()?))
}

// ─── Install steps ───────────────────────────────────────────────────────────

fn resolve(agent_id: &str) -> Result<Agent> {
    catalog::find(agent_id).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown agent '{agent_id}' — known agents: {}",
            catalog::known_ids()
        )
    })
}

fn require_instrumentable(agent: Agent) -> Result<()> {
    let spec = agent.spec();
    if !agent.is_instrumented() {
        bail!(
            "{} can be detected but not yet instrumented — \
             session reporting is implemented for: {}",
            spec.display_name,
            instrumentable_ids()
        );
    }
    Ok(())
}

fn require_present(agent: Agent) -> Result<()> {
    let spec = agent.spec();
    if detect(agent).is_present() {
        return Ok(());
    }
    bail!(
        "{} was not found on this machine (no '{}' on PATH, no {})",
        spec.display_name,
        spec.binary,
        tildify(&agent.config_path())
    )
}

fn save_agent_state(
    agent: Agent,
    agent_name: &str,
    capture_content: bool,
    binding: &InstallationBinding,
) -> Result<()> {
    let spec = agent.spec();
    let mut settings = IntegrationState::load()?;
    settings.declined.remove(spec.id);
    settings.agents.insert(
        spec.id.to_string(),
        AgentState {
            agent_name: agent_name.to_string(),
            capture_content,
            hook_version: agent.install_version().expect("instrumented adapter"),
            binding: Some(binding.clone()),
        },
    );
    settings.save()
}

fn persist_installed_artifacts<T>(
    artifacts: T,
    persist: impl FnOnce() -> Result<()>,
    rollback: impl FnOnce() -> Result<()>,
) -> Result<T> {
    if let Err(error) = persist() {
        rollback().context("failed to roll back integration artifacts")?;
        return Err(error.context("failed to save integration state; installation rolled back"));
    }
    Ok(artifacts)
}

fn instrumentable_ids() -> String {
    Agent::ALL
        .iter()
        .filter(|agent| agent.is_instrumented())
        .map(|agent| agent.spec().id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Shorten a path under `$HOME` to `~/...` for display.
pub fn tildify(path: &Path) -> String {
    let home = catalog::home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn claude() -> Agent {
        catalog::find("claude").unwrap()
    }

    fn discovered(name: &str, card: Result<String, Vec<a2a::CardIssue>>) -> a2a::DiscoveredAgent {
        a2a::DiscoveredAgent {
            name: name.into(),
            card_url: "http://127.0.0.1:8000/.well-known/agent-card.json".into(),
            card,
            origin: a2a::EndpointOrigin::Process {
                pid: Some(4242),
                command: "python".into(),
            },
            raw_card: serde_json::json!({"name": name}),
        }
    }

    #[test]
    fn a2a_agents_join_the_coding_agent_table_as_detected() {
        let agent = discovered("research-crew", Ok("1.0.0".into()));

        let row = a2a_agent_row(&agent, LlmRouting::Off);

        assert!(row.detected);
        assert_eq!(row.connected, "no");
        assert_eq!(row.agent, "research-crew");
        assert_eq!(row.version, "1.0.0");
        assert_eq!(row.config, "-");
        assert_eq!(
            row.a2a_card.as_deref(),
            Some("http://127.0.0.1:8000/.well-known/agent-card.json")
        );
    }

    #[test]
    fn an_a2a_agents_connected_column_is_its_llm_routing() {
        let agent = discovered("research-crew", Ok("1.0.0".into()));
        let connected = |routing| a2a_agent_row(&agent, routing).connected;

        assert_eq!(connected(LlmRouting::Off), "no");
        assert_eq!(connected(LlmRouting::Waiting), "waiting");
        assert_eq!(connected(LlmRouting::On), "yes");
    }

    #[test]
    fn an_invalid_card_has_no_version() {
        let agent = discovered("half-built", Err(Vec::new()));

        assert_eq!(a2a_agent_row(&agent, LlmRouting::Off).version, "-");
    }

    fn coding_row(agent: &str, config: &str) -> DiscoverRow {
        DiscoverRow {
            agent: agent.into(),
            detected: true,
            connected: "no",
            version: "-".into(),
            config: config.into(),
            a2a_card: None,
        }
    }

    #[test]
    fn the_table_is_unchanged_without_a2a_agents() {
        let table = format_discover_table(&[
            coding_row("claude", "~/.claude"),
            coding_row("opencode", "~/.config/opencode"),
        ]);

        assert_eq!(
            table,
            format!(
                "AGENT      DETECTED   CONNECTED   VERSION              CONFIG\n{}\n\
                 claude     yes        no          -                    ~/.claude\n\
                 opencode   yes        no          -                    ~/.config/opencode\n",
                "-".repeat(88)
            )
        );
    }

    #[test]
    fn a2a_agents_add_an_a2a_card_column_after_config() {
        let mut crew = coding_row("research-crew", "-");
        crew.connected = "waiting";
        crew.a2a_card = Some("http://127.0.0.1:8765/.well-known/agent-card.json".into());

        let table = format_discover_table(&[coding_row("claude", "~/.claude"), crew]);
        let lines: Vec<&str> = table.lines().collect();

        assert_eq!(
            lines[0],
            "AGENT         DETECTED   CONNECTED   VERSION              CONFIG    A2A CARD"
        );
        assert_eq!(
            lines[2],
            "claude        yes        no          -                    ~/.claude -"
        );
        assert_eq!(
            lines[3],
            "research-crew yes        waiting     -                    -         \
             http://127.0.0.1:8765/.well-known/agent-card.json"
        );
    }

    fn registered(card_name: &str, called: bool) -> RegisteredA2aAgent {
        RegisteredA2aAgent {
            card_name: card_name.into(),
            id: uuid::Uuid::new_v4(),
            name: card_name.into(),
            last_llm_call_at: called.then(chrono::Utc::now),
        }
    }

    #[test]
    fn llm_routing_is_waiting_after_connect_on_after_a_call_and_off_after_disconnect() {
        let agent = discovered("research-crew", Ok("1.0.0".into()));
        let none = a2a::connections::Connections::default();

        assert_eq!(llm_routing_state(&agent, &[], &none), LlmRouting::Off);
        assert_eq!(
            llm_routing_state(&agent, &[registered("research-crew", false)], &none),
            LlmRouting::Off
        );
        // Disconnected: calls it made while connected do not keep it on.
        assert_eq!(
            llm_routing_state(&agent, &[registered("research-crew", true)], &none),
            LlmRouting::Off
        );
        for (called, state) in [(false, LlmRouting::Waiting), (true, LlmRouting::On)] {
            let entry = registered("research-crew", called);
            let mut connected = a2a::connections::Connections::default();
            connected.agents.insert(
                entry.id,
                a2a::connections::Connection {
                    card_name: "research-crew".into(),
                    name: "research-crew".into(),
                    file: None,
                },
            );
            assert_eq!(llm_routing_state(&agent, &[entry], &connected), state);
        }
    }

    #[test]
    fn only_loopback_cluster_urls_exclude_a_local_port() {
        assert_eq!(local_port_of("http://localhost:8443"), Some(8443));
        assert_eq!(local_port_of("http://127.0.0.1:8080/api"), Some(8080));
        assert_eq!(local_port_of("https://cloud.nasiko.dev:443"), None);
        assert_eq!(local_port_of("http://localhost"), None);
    }

    #[test]
    fn rejects_an_unknown_agent_id_with_the_known_ones() {
        let error = resolve("emacs").unwrap_err().to_string();

        assert!(error.contains("unknown agent 'emacs'"));
        assert!(error.contains("claude"));
    }

    #[test]
    fn allows_installing_an_instrumented_agent() {
        assert!(require_instrumentable(claude()).is_ok());
        assert!(require_instrumentable(catalog::find("codex").unwrap()).is_ok());
        assert!(require_instrumentable(catalog::find("cursor").unwrap()).is_ok());
    }

    #[test]
    fn reports_uninstalled_instrumented_agents_as_not_installed() {
        let settings = IntegrationState::default();

        let status = reporting_status(catalog::find("codex").unwrap(), &settings);

        assert_eq!(status, "not installed");
    }

    #[test]
    fn treats_a_missing_binary_and_missing_config_dir_as_absent() {
        let detection = Detection {
            binary_on_path: false,
            config_dir_exists: false,
        };

        assert!(!detection.is_present());
    }

    #[test]
    fn treats_a_config_dir_alone_as_present() {
        let detection = Detection {
            binary_on_path: false,
            config_dir_exists: true,
        };

        assert!(detection.is_present());
    }

    #[test]
    fn shortens_a_home_path_for_display() {
        let path = catalog::home().join(".claude").join("settings.json");

        assert_eq!(tildify(&path), "~/.claude/settings.json");
    }

    #[test]
    fn leaves_a_path_outside_home_alone() {
        assert_eq!(tildify(Path::new("/etc/hosts")), "/etc/hosts");
    }

    #[test]
    fn failed_state_save_rolls_back_installed_artifacts() {
        let rolled_back = Cell::new(false);
        let result = persist_installed_artifacts(
            "artifacts",
            || Err(anyhow::anyhow!("disk full")),
            || {
                rolled_back.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(rolled_back.get());
    }

    #[test]
    fn active_status_requires_matching_state_version() {
        let expected = claude().install_version().unwrap();
        let state = AgentState {
            agent_name: "claude-code".into(),
            capture_content: true,
            hook_version: expected.saturating_sub(1),
            binding: None,
        };
        assert_ne!(state.hook_version, expected);
    }

    fn bound_state(hook_version: u32) -> AgentState {
        AgentState {
            agent_name: "user-opencode".into(),
            capture_content: true,
            hook_version,
            binding: Some(InstallationBinding {
                cluster_name: "cp".into(),
                cluster_url: "https://cp.example".into(),
                principal_id: uuid::Uuid::nil(),
            }),
        }
    }

    #[test]
    fn auto_setup_installs_an_agent_with_no_state() {
        assert_eq!(auto_setup_action(None, None, 7), AutoSetup::Install);
    }

    #[test]
    fn auto_setup_keeps_a_current_install_and_its_binding() {
        let state = bound_state(7);

        assert_eq!(auto_setup_action(Some(&state), Some(7), 7), AutoSetup::Keep);
    }

    #[test]
    fn auto_setup_refreshes_an_outdated_hook_on_its_existing_binding() {
        let state = bound_state(6);

        let action = auto_setup_action(Some(&state), Some(6), 7);

        assert_eq!(action, AutoSetup::Refresh(state.binding.as_ref().unwrap()));
    }

    #[test]
    fn auto_setup_refreshes_when_hook_files_are_missing_or_unrecognised() {
        let state = bound_state(7);

        let action = auto_setup_action(Some(&state), None, 7);

        assert_eq!(action, AutoSetup::Refresh(state.binding.as_ref().unwrap()));
    }

    #[test]
    fn auto_setup_leaves_legacy_unbound_state_for_an_explicit_install() {
        let state = AgentState {
            binding: None,
            ..bound_state(6)
        };

        assert_eq!(auto_setup_action(Some(&state), Some(6), 7), AutoSetup::Keep);
    }
}
