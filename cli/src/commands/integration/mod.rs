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
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

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

/// Print the A2A agents running on this machine as a second table after the coding agents, then
/// a warning per invalid card and per source that could not be read. Prints no table when none
/// are running, so `status` is unchanged on machines without A2A agents.
pub fn print_running_a2a_agents(cli: &str) {
    let discovery = a2a::discover_local(&local_control_plane_ports());
    if !discovery.agents.is_empty() {
        println!("\nRunning A2A agents:");
        println!("{}", a2a_agent_table(&discovery.agents));
    }
    for agent in &discovery.agents {
        if let Err(errors) = &agent.card {
            for error in errors {
                term::print_warning(&format!(
                    "{} serves an invalid agent card: {error}",
                    agent.name
                ));
            }
        }
    }
    for warning in &discovery.warnings {
        term::print_warning(&format!("{warning}\n  Retry with: {cli} status"));
    }
}

/// One line of the running A2A agents table.
#[derive(Tabled)]
struct A2aAgentRow {
    #[tabled(rename = "AGENT")]
    agent: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "CARD")]
    card: &'static str,
    /// The card URL as fetched from this machine — not the card's own `url`,
    /// which inside a Docker container names the unpublished port.
    #[tabled(rename = "A2A CARD")]
    card_url: String,
}

fn a2a_agent_row(agent: &a2a::DiscoveredAgent) -> A2aAgentRow {
    A2aAgentRow {
        agent: agent.name.clone(),
        version: agent.version().unwrap_or("-").to_string(),
        card: if agent.card.is_ok() {
            "valid"
        } else {
            "invalid"
        },
        card_url: agent.card_url.clone(),
    }
}

/// Laid out like the coding-agent table in `local_agents`, so the two read as one overview.
fn a2a_agent_table(agents: &[a2a::DiscoveredAgent]) -> Table {
    let mut table = Table::new(agents.iter().map(a2a_agent_row));
    table.with(Style::blank()).with(Alignment::left());
    table
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
    status: String,
}

fn reporting_details(agent: Agent, settings: &IntegrationState) -> ReportingDetails {
    let spec = agent.spec();
    if spec.support == Support::DetectOnly {
        return ReportingDetails {
            connected: false,
            status: Support::DetectOnly.label().to_string(),
        };
    }
    let expected = agent
        .install_version()
        .expect("instrumented adapter has a version");
    let Some(state) = settings.get(spec.id) else {
        return ReportingDetails {
            connected: false,
            status: "not installed".into(),
        };
    };
    match agent.installed_version() {
        Some(version) if version == expected => match state.hook_version {
            version if version == expected => ReportingDetails {
                connected: true,
                status: format!("active (v{version})"),
            },
            // Script and saved state are from different install versions.
            _ => ReportingDetails {
                connected: false,
                status: "needs reinstall".into(),
            },
        },
        Some(version) => ReportingDetails {
            connected: false,
            status: format!("stale (v{version}; current v{expected})"),
        },
        None => ReportingDetails {
            connected: false,
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
            card_url: "http://127.0.0.1:8765/.well-known/agent-card.json".into(),
            card,
        }
    }

    #[test]
    fn a_valid_card_shows_its_version_and_where_it_was_fetched() {
        let row = a2a_agent_row(&discovered("research-crew", Ok("1.0.0".into())));

        assert_eq!(row.agent, "research-crew");
        assert_eq!(row.version, "1.0.0");
        assert_eq!(row.card, "valid");
        assert_eq!(
            row.card_url,
            "http://127.0.0.1:8765/.well-known/agent-card.json"
        );
    }

    #[test]
    fn an_invalid_card_is_marked_invalid_with_no_version() {
        let issue = a2a::CardIssue {
            field: "skills[0].description".into(),
            message: "is required".into(),
        };

        let row = a2a_agent_row(&discovered("half-built", Err(vec![issue])));

        assert_eq!(row.version, "-");
        assert_eq!(row.card, "invalid");
    }

    #[test]
    fn the_a2a_table_lists_one_agent_per_line_under_its_header() {
        let table = a2a_agent_table(&[
            discovered("research-crew", Ok("1.0.0".into())),
            discovered("half-built", Err(Vec::new())),
        ])
        .to_string();
        let lines: Vec<Vec<&str>> = table
            .lines()
            .map(|line| line.split_whitespace().collect())
            .collect();

        let url = "http://127.0.0.1:8765/.well-known/agent-card.json";
        assert_eq!(
            lines,
            [
                vec!["AGENT", "VERSION", "CARD", "A2A", "CARD"],
                vec!["research-crew", "1.0.0", "valid", url],
                vec!["half-built", "-", "invalid", url],
            ]
        );
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
