//! Detect local coding agents and report their sessions to Nasiko.
//!
//! [`local_agents`](super::local_agents) answers "what coding agents are on this machine" and
//! drives setup. [`install`] registers an agent in the control plane (or reuses the one LLM
//! routing already uses) and installs a hook that durably queues completed turns for delivery
//! to the cluster active when they were captured.

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
            println!(
                "Reporting under agent '{}' (the same agent LLM routing uses)",
                registration.agent_name
            );
            registration
        }
        None => {
            let registration = control_plane::register_agent(agent)?;
            println!(
                "{} agent '{}' in the control plane",
                if registration.created {
                    "Registered"
                } else {
                    "Found"
                },
                registration.agent_name
            );
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

    println!("Installed hook              {}", tildify(&script));
    println!("Installed integration       {}", tildify(&registration));
    println!(
        "\nStart a new {} session, then: {}",
        spec.display_name,
        crate::commands::local_agents::sessions_hint()
    );
    if agent.restart_required() {
        println!("Restart OpenCode first so it loads the new plugin.");
    }
    if agent == Agent::Codex {
        println!("In Codex, run /hooks and trust the new Nasiko command hooks.");
    }
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
    settings.save()?;

    println!("Removed the {} hook.", spec.display_name);
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

/// Discover and install every supported local coding agent after a successful
/// account connection. One broken adapter must not block the others.
pub fn auto_install_detected() -> Result<()> {
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

    println!("\nConfiguring detected coding agents for this account...");
    let mut installed = 0usize;
    for agent in agents {
        let spec = agent.spec();
        if settings.get(spec.id).is_some() {
            println!(
                "{} reporting is already installed; keeping its existing cluster binding. Rebind explicitly with: {} connect {} --no-routing --reporting",
                spec.display_name,
                crate::commands::local_agents::cli_name(),
                spec.id
            );
            installed += 1;
            continue;
        }
        // Only agents with no reporting installed get here, so there is no saved setting to
        // keep: new reporting includes prompt and response text, as `connect --reporting` does.
        match install(InstallOptions {
            agent_id: spec.id,
            no_content: false,
            agent_name: None,
        }) {
            Ok(()) => installed += 1,
            Err(error) => eprintln!(
                "warning: automatic {} setup failed: {error:#}\n  Retry with: {} connect {} --no-routing --reporting",
                spec.display_name,
                crate::commands::local_agents::cli_name(),
                spec.id
            ),
        }
    }
    println!("Automatic coding-agent setup completed for {installed} agent(s).");
    Ok(())
}

/// Run automatic setup only when the active cluster has a usable login.
pub fn auto_install_if_authenticated() {
    let Ok((cluster, entry)) = crate::config::active_cluster() else {
        return;
    };
    let Some(token) = entry.token.as_deref() else {
        println!(
            "Coding-agent setup skipped for {cluster}: not authenticated. Run `{}`.",
            crate::commands::local_agents::login_hint()
        );
        return;
    };
    if crate::config::token_expired(token) == Some(true) {
        println!(
            "Coding-agent setup skipped for {cluster}: login expired. Run `{}`.",
            crate::commands::local_agents::login_hint()
        );
        return;
    }
    if let Err(error) = auto_install_detected() {
        eprintln!(
            "warning: automatic coding-agent discovery failed: {error:#}\n  Retry with: {} status",
            crate::commands::local_agents::cli_name()
        );
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
}
