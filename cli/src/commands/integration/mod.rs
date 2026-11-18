//! Detect local coding agents and report their sessions to Nasiko.
//!
//! [`local_agents`](super::local_agents) answers "what coding agents are on this machine",
//! "what A2A agents are running on this machine" and drives setup.
//! [`install`] registers an agent in the control plane (or reuses the one LLM
//! routing already uses) and installs a hook that durably queues completed turns for delivery
//! to the cluster and account reporting was installed for — not whichever is active later.
//! `nasiko agents setup` installs every detected agent and imports the sessions they kept from
//! before (`import`), newest first, reporting its progress to the server (`onboarding`).

mod a2a;
mod agents;
mod background;
mod backlog;
mod catalog;
mod control_plane;
mod courier;
mod import;
mod launcher;
mod ledger;
mod locality;
mod model;
mod onboarding;
mod pace;
mod queue;
mod report;
mod sanitize;
mod setup;
mod state;
mod sync;
mod windows;

use anyhow::{Context, Result, bail};
use nasiko_utils::term;
use std::path::Path;

use agents::Agent;
use catalog::Support;
use onboarding::AgentReport;
use state::{AgentState, InstallationBinding, IntegrationState, decline_key};

pub use locality::{
    USAGE, chat_history_question, chat_history_warning, content_refused, content_unconfirmed,
    is_local_cluster, privacy_unknown, synced, usage_only_notice,
};
pub use setup::SetupOptions;

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
    /// Whether this control plane keeps coding agents' chat history. For the active cluster
    /// ([`Self::current`]) the server's own answer; for a recorded binding, whether it runs on
    /// this machine (the fallback, not used to decide anything). `None` when the server couldn't
    /// be asked: new reporting then gets usage only, and existing reporting keeps its setting.
    pub accepts_chat_history: Option<bool>,
}

impl ReportingDestination {
    /// Where reporting would be bound if installed now: the active cluster and its login.
    pub fn current() -> Result<Self> {
        let binding = control_plane::installation_binding()?;
        Ok(Self {
            accepts_chat_history: locality::server_accepts_chat_history(&binding.cluster_url),
            ..Self::from(&binding)
        })
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
            accepts_chat_history: Some(is_local_cluster(&binding.cluster_url)),
        }
    }
}

/// Every catalogued coding agent with its detection and reporting state.
pub fn local_agents() -> Result<Vec<LocalAgent>> {
    let settings = IntegrationState::load()?;
    // Declines count on the cluster and account signed in now; elsewhere the agent is offered.
    let here = current_decline_key();
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
                reporting_declined: settings.is_declined(spec.id, here.as_deref()),
                reporting_agent_name: state.map(|state| state.agent_name.clone()),
                reporting_active: reporting.connected,
                reporting_installed: state.is_some(),
                reporting_status: reporting.status,
                // Chat history only with the consent this CLI records (`chat_history`): a yes
                // saved by an older one, under its default, doesn't count.
                capture_content: state
                    .is_some_and(|state| state.capture_content && state.chat_history),
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
pub use a2a::env_file::{AgentFramework, EnvTarget, Provider};
pub use control_plane::{ListedA2aAgent, RegisteredA2aAgent};

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
    rows.extend(running.agents.iter().map(|agent| {
        a2a_agent_row(
            agent,
            &shown_name(agent, &running.agents, &registered),
            llm_routing_state(agent, &registered, &connections),
        )
    }));
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
    let cards: Vec<(serde_json::Value, Option<String>)> = running
        .agents
        .iter()
        .filter(|agent| agent.card.is_ok())
        .map(|agent| (agent.raw_card.clone(), agent.instance.clone()))
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

    /// As `status`'s LLM ROUTING column shows it, beside the coding agents' `on`/`off`.
    fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Waiting => "waiting",
            Self::On => "on",
        }
    }
}

// ─── status: what the last discovery registered ─────────────────────────────

/// `status`'s A2A section: the agents the last `agents discover` found running on this machine,
/// read back from the cluster, so `status` itself probes nothing. Prints nothing when not
/// logged in, when the cluster cannot be reached (its section already says so), or when no
/// agent from this machine is registered.
pub fn print_discovered_a2a_agents() {
    let Some(machine) = a2a::machine_name() else {
        return;
    };
    let Ok(Some(listed)) = control_plane::list_discovered(&machine) else {
        return;
    };
    let connections = a2a::connections::Connections::load().unwrap_or_default();
    if let Some(section) = discovered_section(&listed, &connections) {
        print!("{section}");
    }
}

#[derive(tabled::Tabled)]
struct DiscoveredRow {
    #[tabled(rename = "AGENT")]
    agent: String,
    #[tabled(rename = "LLM ROUTING")]
    routing: String,
}

/// The section's text, or `None` when the last discovery found nothing running. Agents it
/// found gone (`stopped`) are left out: `status` lists what is there.
fn discovered_section(
    listed: &[ListedA2aAgent],
    connections: &a2a::connections::Connections,
) -> Option<String> {
    let running: Vec<&ListedA2aAgent> = listed
        .iter()
        .filter(|agent| agent.status == "registered")
        .collect();
    if running.is_empty() {
        return None;
    }
    let value = crate::commands::coding_agent_router::status_value;
    let rows = running.iter().map(|agent| DiscoveredRow {
        // Copies of one agent are told apart by their catalog names.
        agent: a2a::printable(
            if running
                .iter()
                .filter(|other| other.card_name == agent.card_name)
                .count()
                > 1
            {
                &agent.name
            } else {
                &agent.card_name
            },
        ),
        routing: value(
            llm_routing_of(agent.id, agent.last_llm_call_at.is_some(), connections).label(),
        ),
    });
    let mut table = tabled::Table::new(rows);
    table
        .with(tabled::settings::Style::blank())
        .with(tabled::settings::Alignment::left());
    Some(format!("\nA2A agents:\n{table}\n"))
}

fn llm_routing_state(
    agent: &A2aAgent,
    registered: &[RegisteredA2aAgent],
    connections: &a2a::connections::Connections,
) -> LlmRouting {
    match registration_of(agent, registered) {
        Some(entry) => llm_routing_of(entry.id, entry.last_llm_call_at.is_some(), connections),
        None => LlmRouting::Off,
    }
}

/// How the control plane registered `agent`: the entry for its card name and copy.
fn registration_of<'a>(
    agent: &A2aAgent,
    registered: &'a [RegisteredA2aAgent],
) -> Option<&'a RegisteredA2aAgent> {
    registered
        .iter()
        .find(|entry| entry.card_name == agent.name && entry.instance == agent.instance)
}

/// The name an agent is listed under: its card name, or its catalog name when another agent
/// running here has the same card name (`planner`, `planner-3fa2`), so each copy can be named.
fn shown_name(agent: &A2aAgent, running: &[A2aAgent], registered: &[RegisteredA2aAgent]) -> String {
    let namesakes = running
        .iter()
        .filter(|other| other.name == agent.name)
        .count();
    match registration_of(agent, registered) {
        Some(entry) if namesakes > 1 => a2a::printable(&entry.name),
        _ => agent.name.clone(),
    }
}

/// Only `connect` points an agent at Nasiko, so only a connected agent is `waiting` or `on`;
/// after `disconnect` its past calls no longer count.
fn llm_routing_of(
    id: uuid::Uuid,
    has_routed_call: bool,
    connections: &a2a::connections::Connections,
) -> LlmRouting {
    match (connections.agents.contains_key(&id), has_routed_call) {
        (false, _) => LlmRouting::Off,
        (true, false) => LlmRouting::Waiting,
        (true, true) => LlmRouting::On,
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
fn a2a_agent_row(agent: &A2aAgent, name: &str, routing: LlmRouting) -> DiscoverRow {
    DiscoverRow {
        agent: name.to_string(),
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

/// The running A2A agent `target` names, for `connect`: its card URL, its catalog name, or its
/// card name when no other agent running here has that name. `Ok(None)` when it names none
/// (it may be a control plane). Fails when it is a card name several copies share and none of
/// them has it as catalog name, listing how to name each.
///
/// Asks the control plane only when the card name alone cannot decide: to read the catalog
/// names (a name like `planner-3fa2`), or to register copies that share a name.
pub fn select_a2a_agent(target: &str, running: &RunningA2aAgents) -> Result<Option<A2aAgent>> {
    let agents = &running.agents;
    let namesakes = agents.iter().filter(|agent| agent.name == target).count();
    let maybe_catalog_name = namesakes == 0
        && agents
            .iter()
            .any(|agent| target.starts_with(&format!("{}-", catalog_slug(&agent.name))));
    let catalog: Vec<CatalogEntry> = if namesakes > 1 {
        register_running(running)?
            .into_iter()
            .map(CatalogEntry::from)
            .collect()
    } else if maybe_catalog_name && let Some(machine) = a2a::machine_name() {
        control_plane::list_discovered(&machine)
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.status == "registered")
            .map(CatalogEntry::from)
            .collect()
    } else {
        Vec::new()
    };
    match select(target, agents, &catalog) {
        Selection::None => Ok(None),
        Selection::One(index) => {
            let agent = &agents[index];
            let others: Vec<String> = agents
                .iter()
                .enumerate()
                .filter(|(other, copy)| *other != index && copy.name == agent.name)
                .map(|(_, copy)| format!("  {}", copy_line(copy, &catalog)))
                .collect();
            if !others.is_empty() {
                term::print_info(&format!(
                    "Connecting {}\nOther agents named {} run here:\n{}",
                    copy_line(agent, &catalog),
                    agent.name,
                    others.join("\n")
                ));
            }
            Ok(Some(agent.clone()))
        }
        Selection::Ambiguous(copies) => {
            let lines: Vec<String> = copies
                .iter()
                .map(|index| format!("  {}", copy_line(&agents[*index], &catalog)))
                .collect();
            bail!(
                "{} agents are named {target}; name one by its catalog name or card URL:\n{}",
                copies.len(),
                lines.join("\n")
            )
        }
    }
}

/// How a copy is listed when several share a name: its catalog name (when registered), card
/// URL and folder.
fn copy_line(agent: &A2aAgent, catalog: &[CatalogEntry]) -> String {
    let name = catalog
        .iter()
        .find(|entry| entry.is(agent))
        .map_or(agent.name.as_str(), |entry| entry.name.as_str());
    let folder = match &agent.origin {
        a2a::EndpointOrigin::Process { pid: Some(pid), .. } => {
            a2a::process::working_folder(*pid).map(|folder| tildify(&folder))
        }
        a2a::EndpointOrigin::Docker { container } => Some(format!("container {container}")),
        a2a::EndpointOrigin::Process { pid: None, .. } => None,
    };
    format!(
        "{}  {}{}",
        a2a::printable(name),
        agent.card_url,
        folder
            .map(|f| format!("  {}", a2a::printable(&f)))
            .unwrap_or_default()
    )
}

/// The start the control plane gives catalog names from `card_name` (its slug, roughly), so a
/// target that cannot be one is never looked up.
fn catalog_slug(card_name: &str) -> String {
    card_name.to_lowercase().replace(' ', "-")
}

/// One registered agent, as `connect` names it.
struct CatalogEntry {
    card_name: String,
    instance: Option<String>,
    name: String,
}

impl CatalogEntry {
    fn is(&self, agent: &A2aAgent) -> bool {
        self.card_name == agent.name && self.instance == agent.instance
    }
}

impl From<RegisteredA2aAgent> for CatalogEntry {
    fn from(entry: RegisteredA2aAgent) -> Self {
        Self {
            card_name: entry.card_name,
            instance: entry.instance,
            name: entry.name,
        }
    }
}

impl From<ListedA2aAgent> for CatalogEntry {
    fn from(entry: ListedA2aAgent) -> Self {
        Self {
            card_name: entry.card_name,
            instance: entry.instance,
            name: entry.name,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Selection {
    None,
    One(usize),
    /// Indexes of the copies sharing the card name.
    Ambiguous(Vec<usize>),
}

/// Which of `agents` `target` names: a card URL, then a catalog name, then a card name.
fn select(target: &str, agents: &[A2aAgent], catalog: &[CatalogEntry]) -> Selection {
    if target.contains("://") {
        let wanted = target.trim_end_matches('/');
        return agents
            .iter()
            .position(|agent| agent.card_url.trim_end_matches('/') == wanted)
            .map_or(Selection::None, Selection::One);
    }
    if let Some(index) = catalog
        .iter()
        .filter(|entry| entry.name == target)
        .find_map(|entry| agents.iter().position(|agent| entry.is(agent)))
    {
        return Selection::One(index);
    }
    let namesakes: Vec<usize> = (0..agents.len())
        .filter(|index| agents[*index].name == target)
        .collect();
    match namesakes.as_slice() {
        [] => Selection::None,
        [index] => Selection::One(*index),
        _ => Selection::Ambiguous(namesakes),
    }
}

/// What `connect` does for one running A2A agent: its catalog identity, the variables that
/// route its LLM calls through Nasiko and give it the MCP gateway, and where they go.
pub struct A2aConnection {
    pub agent_id: uuid::Uuid,
    pub card_name: String,
    /// Its catalog name.
    pub name: String,
    /// Empty until [`A2aConnection::choose`] sets them.
    pub vars: Vec<(&'static str, String)>,
    /// Whether `vars` include the MCP gateway's.
    pub mcp: McpSetup,
    /// The providers its env file already routes or calls, to offer as the default choice.
    pub providers_in_use: Vec<a2a::env_file::Provider>,
    key: String,
    cluster_url: String,
    /// The framework it runs on, when known, for showing how its code opens the gateway.
    pub framework: Option<a2a::env_file::AgentFramework>,
    /// The agent's working folder, when known.
    pub folder: Option<std::path::PathBuf>,
    /// The file to write them to, or why the user must set them.
    pub target: EnvTarget,
}

/// Whether `connect` gives an A2A agent the MCP gateway, as a deployed agent has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSetup {
    /// Not chosen.
    NotRequested,
    /// The credential could not be minted (an older control plane, say); LLM routing goes on.
    Unavailable(String),
    /// Its `MCP_GATEWAY_*` variables are in `vars`.
    Added,
}

/// Register the running agents, then prepare `agent`'s connection: its LLM key and the file
/// decided from its process (see [`a2a::env_file`]). What goes in the file is chosen next,
/// with [`A2aConnection::choose`].
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
    let entry = registration_of(agent, &registered).with_context(|| {
            format!(
                "{} was not registered (is this machine's hostname readable, and did discovery finish without warnings?)",
                agent.name
            )
        })?;
    let (key, cluster_url) = control_plane::routing_key(entry.id)?;
    let mut framework = None;
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
            framework = a2a::env_file::framework_of(&facts);
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
    let providers_in_use = match &target {
        EnvTarget::File(file) => {
            a2a::env_file::providers_in(&std::fs::read_to_string(file).unwrap_or_default())
        }
        EnvTarget::Unknown(_) => Vec::new(),
    };
    Ok(A2aConnection {
        agent_id: entry.id,
        card_name: agent.name.clone(),
        name: entry.name.clone(),
        vars: Vec::new(),
        mcp: McpSetup::NotRequested,
        providers_in_use,
        key,
        cluster_url,
        framework,
        folder,
        target,
    })
}

impl A2aConnection {
    /// Set the variables for what the user chose: routing for `providers`, and the MCP
    /// gateway when `with_mcp` (its credential is minted only then).
    pub fn choose(&mut self, providers: &[a2a::env_file::Provider], with_mcp: bool) {
        self.vars = a2a::env_file::routing_vars(&self.cluster_url, &self.key, providers);
        self.mcp = if with_mcp {
            match control_plane::mcp_token(self.agent_id) {
                Ok(token) => {
                    self.vars
                        .extend(a2a::env_file::mcp_vars(&self.cluster_url, &token));
                    McpSetup::Added
                }
                Err(error) => McpSetup::Unavailable(
                    crate::commands::coding_agent_router::one_line_warning(&error),
                ),
            }
        } else {
            McpSetup::NotRequested
        };
    }
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
            mcp: connection.mcp == McpSetup::Added,
        },
    );
    connections.save()
}

/// Undo `connect` for the A2A agent named `name`: remove Nasiko's block from the file it
/// wrote, forget the connection, and revoke its LLM key and MCP credential. `Ok(None)` when
/// no A2A agent of that name is connected.
pub fn disconnect_a2a_agent(name: &str) -> Result<Option<String>> {
    let mut connections = a2a::connections::Connections::load()?;
    let found = connections.find(name).map_err(|names| {
        anyhow::anyhow!(
            "{} connected agents are named {name}; disconnect one by its catalog name: {}",
            names.len(),
            names.join(", ")
        )
    })?;
    let Some((id, connection)) = found else {
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
    let had_mcp = connection.mcp;
    connections.agents.remove(&id);
    connections.save()?;
    // The key may have been copied elsewhere, so removing it from the file is not enough.
    if let Err(error) = control_plane::revoke_llm_tokens(id) {
        term::print_warning(&format!(
            "failed to revoke its LLM key: {}",
            crate::commands::coding_agent_router::one_line_warning(&error)
        ));
    }
    if had_mcp && let Err(error) = control_plane::revoke_mcp_token(id) {
        term::print_warning(&format!(
            "failed to revoke its MCP gateway credential: {}",
            crate::commands::coding_agent_router::one_line_warning(&error)
        ));
    }
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
    let removed = settings.agents.remove(spec.id);
    // Removing reporting is a no on the cluster it reported to: it isn't offered again there
    // after the next login, and still is on any other.
    let key = removed
        .as_ref()
        .and_then(|state| state.binding.as_ref())
        .map(|binding| decline_key(&binding.cluster_url, &binding.principal_id.to_string()))
        .or_else(current_decline_key);
    if let Some(key) = key {
        settings.decline(spec.id, &key);
    }
    let agent_name = removed.map_or_else(|| spec.agent_name.to_string(), |state| state.agent_name);
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

/// Connects every detected coding agent to the active cluster and account, for `nasiko agents
/// setup`. Running setup is the user's yes, so an agent declined at a login offer is connected
/// too, and one reporting elsewhere is moved here with its content setting kept. One broken
/// adapter must not block the others.
pub(super) fn auto_install_detected() -> Result<()> {
    let settings = IntegrationState::load()?;
    let agents: Vec<_> = Agent::ALL
        .iter()
        .copied()
        .filter(|agent| agent.spec().support == Support::Instrumented)
        .filter(|agent| detect(*agent).is_present())
        .collect();
    if agents.is_empty() {
        println!("No supported local coding agents detected.");
        return Ok(());
    }

    let here = ReportingDestination::current()?;
    let steps: Vec<_> = agents
        .into_iter()
        .map(|agent| (agent, setup_step(settings.get(agent.spec().id), &here)))
        .collect();
    // Asked only when its answer is used: agents already reporting here are kept as they are.
    let chat_history = if steps.iter().any(|(_, step)| *step != SetupStep::Keep) {
        chat_history_for_setup(&here)?
    } else {
        false
    };
    println!("\nConfiguring detected coding agents for this account...");
    let mut installed = 0usize;
    for (agent, step) in steps {
        let spec = agent.spec();
        let reconnect = format!(
            "{} connect {} --no-routing --reporting",
            crate::commands::local_agents::cli_name(),
            spec.id
        );
        let capture_content = match step {
            SetupStep::Keep => {
                println!(
                    "{} already reports sessions to {}.",
                    spec.display_name, here.cluster_name
                );
                installed += 1;
                continue;
            }
            SetupStep::Install => chat_history,
            SetupStep::Move { from } => {
                println!(
                    "{} reports sessions to {from}; moving it to {}.",
                    spec.display_name, here.cluster_name
                );
                // Asked for here, like a new install: an earlier yes was for another cluster.
                chat_history
            }
        };
        match install(InstallOptions {
            agent_id: spec.id,
            no_content: !capture_content,
            agent_name: None,
        }) {
            Ok(()) => installed += 1,
            Err(error) => term::print_warning(&format!(
                "Automatic {} setup failed: {error:#}\n  Retry with: {}",
                spec.display_name,
                term::cyan_stderr(&reconnect)
            )),
        }
    }
    println!("Automatic coding-agent setup completed for {installed} agent(s).");
    Ok(())
}

/// Whether `nasiko agents setup` syncs chat history for the agents it connects or moves here. A
/// control plane that doesn't keep it never gets it (and is said so); one that does is asked
/// once, on a terminal, defaulting to no. Without a terminal (the desktop app, a script), or when
/// the control plane couldn't be asked, usage only.
fn chat_history_for_setup(here: &ReportingDestination) -> Result<bool> {
    use std::io::IsTerminal;
    match here.accepts_chat_history {
        Some(true) => {}
        Some(false) => {
            term::print_info(&usage_only_notice(&here.cluster_name));
            return Ok(false);
        }
        None => {
            term::print_warning(&privacy_unknown(&here.cluster_name));
            return Ok(false);
        }
    }
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    term::print_warning(&chat_history_warning(&here.cluster_name));
    Ok(
        dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt(chat_history_question(&here.cluster_name))
            .default(false)
            .interact()?,
    )
}

/// What `nasiko agents setup` does with one detected agent's reporting.
#[derive(Debug, PartialEq, Eq)]
enum SetupStep {
    /// Nothing installed yet: install it here.
    Install,
    /// Already reporting to this cluster and account.
    Keep,
    /// Reporting goes to `from` (another cluster or account, or none recorded): reinstall it
    /// here, with this run's chat-history answer.
    Move { from: String },
}

/// Running setup is the user's yes, so reporting that goes elsewhere is moved to `here`, the
/// cluster and account setup runs for.
fn setup_step(state: Option<&AgentState>, here: &ReportingDestination) -> SetupStep {
    let Some(state) = state else {
        return SetupStep::Install;
    };
    let from = match &state.binding {
        Some(binding) if ReportingDestination::from(binding).same_as(here) => {
            return SetupStep::Keep;
        }
        Some(binding) => binding.cluster_name.clone(),
        None => "an older install with no cluster recorded".to_string(),
    };
    SetupStep::Move { from }
}

/// `nasiko agents sync` — see [`setup::sync`].
pub fn sync(retry_rejected: bool) -> Result<()> {
    setup::sync(retry_rejected)
}

/// `nasiko agents setup` — see [`setup`].
pub fn setup(options: SetupOptions) -> Result<()> {
    setup::setup(options)
}

/// `nasiko agents import` — see [`setup`].
pub fn import() -> Result<()> {
    setup::import()
}

/// Imports the connected agents' past sessions in the background, each to where it reports now —
/// see [`setup::start_background_import`].
pub fn import_in_background() -> Result<()> {
    setup::start_background_import()
}

/// `nasiko agents status` — see [`setup::status`].
pub fn import_status() -> Result<()> {
    setup::status()
}

/// `nasiko agents import-worker`, which `nasiko agents setup` starts in the background — see
/// [`setup::import_worker`].
pub fn import_worker() -> Result<()> {
    setup::import_worker()
}

/// Every catalogued agent as a sync-report row: on this computer, and connected, as they stand now.
fn agent_rows() -> Vec<AgentReport> {
    let settings = IntegrationState::load().unwrap_or_default();
    Agent::ALL
        .iter()
        .copied()
        .map(|agent| {
            onboarding::agent_row(
                agent,
                detect(agent).is_present(),
                agent.is_instrumented() && settings.get(agent.spec().id).is_some(),
            )
        })
        .collect()
}

/// Remembers that the user said no when reporting was offered for these agents after logging
/// in, so it isn't offered again on this cluster and account. Without a login to record it
/// against, nothing is recorded (the offer is only made to a signed-in user).
pub fn record_declined(agent_ids: &[&str]) -> Result<()> {
    let Some(key) = current_decline_key() else {
        return Ok(());
    };
    let mut settings = IntegrationState::load()?;
    for id in agent_ids {
        settings.decline(id, &key);
    }
    settings.save()
}

/// The cluster and account a decline made now is kept under, if anyone is signed in.
fn current_decline_key() -> Option<String> {
    control_plane::installation_binding()
        .ok()
        .map(|binding| decline_key(&binding.cluster_url, &binding.principal_id.to_string()))
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
        // A yes saved without this CLI's recorded consent is dropped here, not carried on.
        || {
            save_agent_state(
                agent,
                &state.agent_name,
                state.capture_content && state.chat_history,
                binding,
            )
        },
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
    settings.clear_declines(spec.id);
    settings.agents.insert(
        spec.id.to_string(),
        AgentState {
            agent_name: agent_name.to_string(),
            capture_content,
            // Saved only by a run that asked (or read --content) for a cluster that keeps it.
            chat_history: capture_content,
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
            instance: None,
        }
    }

    #[test]
    fn a2a_agents_join_the_coding_agent_table_as_detected() {
        let agent = discovered("research-crew", Ok("1.0.0".into()));

        let row = a2a_agent_row(&agent, &agent.name, LlmRouting::Off);

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
        let connected = |routing| a2a_agent_row(&agent, &agent.name, routing).connected;

        assert_eq!(connected(LlmRouting::Off), "no");
        assert_eq!(connected(LlmRouting::Waiting), "waiting");
        assert_eq!(connected(LlmRouting::On), "yes");
    }

    #[test]
    fn an_invalid_card_has_no_version() {
        let agent = discovered("half-built", Err(Vec::new()));

        assert_eq!(
            a2a_agent_row(&agent, &agent.name, LlmRouting::Off).version,
            "-"
        );
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
            instance: None,
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
                    mcp: false,
                },
            );
            assert_eq!(llm_routing_state(&agent, &[entry], &connected), state);
        }
    }

    fn listed(card_name: &str, status: &str, called: bool) -> ListedA2aAgent {
        ListedA2aAgent {
            card_name: card_name.into(),
            instance: None,
            id: uuid::Uuid::new_v4(),
            name: card_name.into(),
            status: status.into(),
            last_llm_call_at: called.then(chrono::Utc::now),
        }
    }

    fn words(text: &str) -> Vec<Vec<String>> {
        text.lines()
            .map(|line| line.split_whitespace().map(str::to_string).collect())
            .filter(|words: &Vec<String>| !words.is_empty())
            .collect()
    }

    #[test]
    fn status_lists_what_the_last_discovery_found_with_its_llm_routing() {
        let planner = listed("planner", "registered", true);
        let writer = listed("writer", "registered", false);
        let gone = listed("old-crew", "stopped", false);
        let mut connections = a2a::connections::Connections::default();
        connections.agents.insert(
            planner.id,
            a2a::connections::Connection {
                card_name: "planner".into(),
                name: "planner".into(),
                file: None,
                mcp: false,
            },
        );

        let section =
            discovered_section(&[planner, writer, gone], &connections).expect("a section");

        assert_eq!(
            words(&section),
            [
                vec!["A2A", "agents:"],
                vec!["AGENT", "LLM", "ROUTING"],
                vec!["planner", "on"],
                vec!["writer", "off"],
            ]
        );
    }

    #[test]
    fn status_shows_no_a2a_section_when_nothing_was_found_running() {
        let none = a2a::connections::Connections::default();
        assert!(discovered_section(&[], &none).is_none());
        let only_gone = [listed("old-crew", "stopped", false)];
        assert!(discovered_section(&only_gone, &none).is_none());
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
            chat_history: true,
            hook_version: expected.saturating_sub(1),
            binding: None,
        };
        assert_ne!(state.hook_version, expected);
    }

    fn bound_state(hook_version: u32) -> AgentState {
        AgentState {
            agent_name: "user-opencode".into(),
            capture_content: true,
            chat_history: true,
            hook_version,
            binding: Some(InstallationBinding {
                cluster_name: "cp".into(),
                cluster_url: "https://cp.example".into(),
                principal_id: uuid::Uuid::nil(),
            }),
        }
    }

    fn signed_in_to(cluster_name: &str, cluster_url: &str) -> ReportingDestination {
        ReportingDestination {
            cluster_name: cluster_name.into(),
            cluster_url: cluster_url.into(),
            principal_id: uuid::Uuid::nil().to_string(),
            accepts_chat_history: Some(false),
        }
    }

    #[test]
    fn setup_moves_reporting_bound_to_another_cluster_here() {
        let state = bound_state(7);

        assert_eq!(
            setup_step(None, &signed_in_to("cp", "https://cp.example")),
            SetupStep::Install
        );
        assert_eq!(
            setup_step(Some(&state), &signed_in_to("cp", "https://cp.example")),
            SetupStep::Keep
        );
        assert_eq!(
            setup_step(Some(&state), &signed_in_to("cp2", "https://cp2.example")),
            SetupStep::Move { from: "cp".into() }
        );
    }

    #[test]
    fn setup_moves_an_older_install_with_no_cluster_recorded() {
        let mut state = bound_state(7);
        state.binding = None;
        state.capture_content = false;

        assert_eq!(
            setup_step(Some(&state), &signed_in_to("cp", "https://cp.example")),
            SetupStep::Move {
                from: "an older install with no cluster recorded".into(),
            }
        );
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

    fn copy(name: &str, port: u16, instance: &str) -> a2a::DiscoveredAgent {
        a2a::DiscoveredAgent {
            card_url: format!("http://127.0.0.1:{port}/assistants/{name}/card"),
            instance: Some(instance.into()),
            ..discovered(name, Ok("1.0.0".into()))
        }
    }

    fn catalog_entry(card_name: &str, instance: &str, name: &str) -> CatalogEntry {
        CatalogEntry {
            card_name: card_name.into(),
            instance: Some(instance.into()),
            name: name.into(),
        }
    }

    #[test]
    fn connect_names_a_copy_by_card_url_catalog_name_or_a_card_name_no_other_shares() {
        let agents = [
            copy("planner", 2024, "aaaa"),
            copy("planner", 2025, "bbbb"),
            copy("writer", 2024, "aaaa"),
        ];
        let catalog = [
            catalog_entry("planner", "aaaa", "planner"),
            catalog_entry("planner", "bbbb", "planner-bbbb"),
            catalog_entry("writer", "aaaa", "writer"),
        ];

        assert_eq!(select("writer", &agents, &[]), Selection::One(2));
        assert_eq!(
            select("planner", &agents, &[]),
            Selection::Ambiguous(vec![0, 1])
        );
        assert_eq!(select("planner", &agents, &catalog), Selection::One(0));
        assert_eq!(select("planner-bbbb", &agents, &catalog), Selection::One(1));
        assert_eq!(
            select(
                "http://127.0.0.1:2025/assistants/planner/card/",
                &agents,
                &[]
            ),
            Selection::One(1)
        );
        assert_eq!(
            select("http://127.0.0.1:9/card", &agents, &[]),
            Selection::None
        );
        assert_eq!(select("prod", &agents, &catalog), Selection::None);
        // Neither copy holds the plain name (both registered after a third took it).
        let suffixed = [
            catalog_entry("planner", "aaaa", "planner-aaaa"),
            catalog_entry("planner", "bbbb", "planner-bbbb"),
        ];
        assert_eq!(
            select("planner", &agents, &suffixed),
            Selection::Ambiguous(vec![0, 1])
        );
    }

    #[test]
    fn copies_sharing_a_card_name_are_listed_by_their_catalog_names() {
        let agents = [
            copy("planner", 2024, "aaaa"),
            copy("planner", 2025, "bbbb"),
            copy("writer", 2024, "aaaa"),
        ];
        let entry = |card_name: &str, instance: &str, name: &str| RegisteredA2aAgent {
            instance: Some(instance.into()),
            name: name.into(),
            ..registered(card_name, false)
        };
        let registered = [
            entry("planner", "aaaa", "planner"),
            entry("planner", "bbbb", "planner-bbbb"),
            entry("writer", "aaaa", "writer"),
        ];
        let shown: Vec<String> = agents
            .iter()
            .map(|agent| shown_name(agent, &agents, &registered))
            .collect();
        assert_eq!(shown, ["planner", "planner-bbbb", "writer"]);
        // Before registration (not logged in) each shows its card name.
        assert_eq!(shown_name(&agents[1], &agents, &[]), "planner");
    }

    #[test]
    fn status_lists_copies_sharing_a_card_name_by_their_catalog_names() {
        let mut first = listed("planner", "registered", false);
        let mut second = listed("planner", "registered", false);
        first.instance = Some("aaaa".into());
        second.instance = Some("bbbb".into());
        second.name = "planner-bbbb".into();
        let section =
            discovered_section(&[first, second], &a2a::connections::Connections::default())
                .unwrap();
        let rows = words(&section);
        assert_eq!(rows[2][0], "planner");
        assert_eq!(rows[3][0], "planner-bbbb");
    }
}
