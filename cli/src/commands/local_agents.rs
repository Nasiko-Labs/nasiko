//! One command surface for local coding agents — Claude Code, Codex, OpenCode and Cursor:
//! `nasiko connect`, `nasiko disconnect` and `nasiko status`.
//!
//! Each agent can get up to three things from the control plane: LLM routing (its model calls go
//! through the Nasiko LLM router), the MCP gateway (its tools include the user's Nasiko
//! connectors) and session reporting (completed turns land in Nasiko observability). On a
//! terminal the commands ask about each; the same questions are answered by flags in scripts.
//! The work itself stays in the per-agent modules (`claude`, `codex`, `opencode`) and in
//! `integration`; this module only decides what to do and in which order.

use std::io::IsTerminal;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use clap::Args;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, MultiSelect, Select};
use nasiko_utils::term;
use serde_json::Value;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::{Client, unwrap_data};
use crate::commands::coding_agent_router::{
    self, AgentLookup, ConfigChange, ConfigNotRestored, RoutingOptions, RoutingSummary,
};
use crate::commands::integration::{self, InstallOptions, LocalAgent, ReportingDestination};
use crate::commands::{auth, claude, codex, opencode};

const HELP_HEADING: &str = "Coding agents";

/// Flags for `connect <agent>`; each answers one of the questions `connect` asks on a terminal.
#[derive(Args, Debug, Clone, Default)]
pub struct ConnectOptions {
    /// Existing routing agent name or UUID (one coding agent at a time)
    #[arg(long, conflicts_with = "no_routing", help_heading = HELP_HEADING)]
    pub agent: Option<String>,
    /// LLM config name or UUID to route LLM calls through
    #[arg(long, conflicts_with = "no_routing", help_heading = HELP_HEADING)]
    pub config: Option<String>,
    /// Don't route the agent's LLM calls through Nasiko
    #[arg(long, help_heading = HELP_HEADING)]
    pub no_routing: bool,
    /// Give the agent your MCP connectors through the Nasiko gateway (the default for a new
    /// connection; a reconnect keeps the agent's current setting)
    #[arg(long, conflicts_with_all = ["no_mcp", "no_routing"], help_heading = HELP_HEADING)]
    pub mcp: bool,
    /// Don't give the agent your MCP connectors through the Nasiko gateway
    #[arg(long, conflicts_with = "no_routing", help_heading = HELP_HEADING)]
    pub no_mcp: bool,
    /// Report sessions to Nasiko observability — tokens, latency and cost; add --content for
    /// prompt and response text (asked on a terminal, off otherwise)
    #[arg(long, conflicts_with = "no_reporting", help_heading = HELP_HEADING)]
    pub reporting: bool,
    /// Don't report sessions
    #[arg(long, help_heading = HELP_HEADING)]
    pub no_reporting: bool,
    /// Report sessions including prompt and response text (implies --reporting)
    #[arg(long, conflicts_with_all = ["no_reporting", "no_content"], help_heading = HELP_HEADING)]
    pub content: bool,
    /// Report tokens, latency and cost only — no prompt or response text (implies --reporting)
    #[arg(long, conflicts_with = "no_reporting", help_heading = HELP_HEADING)]
    pub no_content: bool,
    /// Reconnect even when the coding agent is still running
    #[arg(long, help_heading = HELP_HEADING)]
    pub force: bool,
    /// Don't ask anything; use the flags and defaults
    #[arg(short = 'y', long, help_heading = HELP_HEADING)]
    pub yes: bool,
}

impl ConnectOptions {
    /// Whether any coding-agent flag was given — they mean nothing for a control-plane URL.
    pub fn is_set(&self) -> bool {
        self.agent.is_some()
            || self.config.is_some()
            || self.no_routing
            || self.mcp
            || self.no_mcp
            || self.reporting
            || self.no_reporting
            || self.content
            || self.no_content
            || self.force
            || self.yes
    }

    /// Whether the flags ask for session reporting: `--reporting`, or a content setting.
    fn mentions_reporting(&self) -> bool {
        self.reporting || self.content || self.no_content
    }

    /// Whether a flag only LLM routing can honour was given.
    fn mentions_routing(&self) -> bool {
        self.agent.is_some() || self.config.is_some() || self.mcp || self.no_mcp
    }

    /// The MCP setting the flags ask for, if any.
    fn mcp_choice(&self) -> Option<bool> {
        if self.no_mcp {
            Some(false)
        } else if self.mcp {
            Some(true)
        } else {
            None
        }
    }

    /// The content setting the flags ask for, if any. `--reporting` alone has none, so reporting
    /// that is already on keeps its own.
    fn content_choice(&self) -> Option<bool> {
        if self.no_content {
            Some(false)
        } else if self.content {
            Some(true)
        } else {
            None
        }
    }
}

/// Flags for `disconnect <agent>`.
#[derive(Args, Debug, Clone, Default)]
pub struct DisconnectOptions {
    /// Remove LLM routing and the MCP gateway
    #[arg(long)]
    pub routing: bool,
    /// Remove session reporting
    #[arg(long)]
    pub reporting: bool,
    /// Disconnect even when the coding agent is still running
    #[arg(long)]
    pub force: bool,
    /// Don't ask anything; use the flags and defaults
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// Whether `target` names a coding agent (rather than a control plane or deployment).
pub fn is_coding_agent(target: &str) -> bool {
    integration::is_known_agent(target)
}

/// The command names hints print. A CLI built on this one (with its own binary name, or with
/// some of these commands in other places) sets its own at startup through [`set_cli_hints`];
/// otherwise they are this CLI's.
pub struct CliHints {
    /// The binary name, as hints should spell it.
    pub name: String,
    /// How to log in to the active control plane again.
    pub login: String,
    /// How to list reported coding-agent sessions.
    pub sessions: String,
    /// The LLM config command group (`<it> create …`, `<it> update …`).
    pub llm_config: String,
    /// Where the agent commands (build, deploy, logs, chat, ps, maf, …) live: `nasiko` here,
    /// `<name> agents` in a CLI that groups them under `agents`.
    pub agents: String,
}

static CLI_HINTS: OnceLock<CliHints> = OnceLock::new();

/// Replaces this CLI's hints; only the first call, before any hint is printed, has an effect.
pub fn set_cli_hints(hints: CliHints) {
    let _ = CLI_HINTS.set(hints);
}

fn cli_hints() -> &'static CliHints {
    CLI_HINTS.get_or_init(|| CliHints {
        name: "nasiko".to_string(),
        login: "nasiko auth login".to_string(),
        sessions: "nasiko observe sessions".to_string(),
        llm_config: "nasiko llm-config".to_string(),
        agents: "nasiko".to_string(),
    })
}

/// The CLI's binary name, for the hints it prints.
pub fn cli_name() -> &'static str {
    &cli_hints().name
}

pub fn login_hint() -> &'static str {
    &cli_hints().login
}

pub fn sessions_hint() -> &'static str {
    &cli_hints().sessions
}

pub fn llm_config_hint() -> &'static str {
    &cli_hints().llm_config
}

/// An agent command as this CLI spells it: `agent_command("logs weather")` is
/// `nasiko logs weather` here and `nasiko-ee agents logs weather` in nasiko-ee.
pub fn agent_command(rest: &str) -> String {
    format!("{} {rest}", cli_hints().agents)
}

pub fn maf_hint() -> String {
    agent_command("maf")
}

/// Points a caller of a renamed command at its replacement. The old command still runs.
pub fn note_renamed(old: &str, new: &str) {
    let cli = cli_name();
    term::print_note(&format!(
        "`{cli} {old}` is now `{cli} {new}`; the old name will be removed in a future release."
    ));
}

// ─── The agents ──────────────────────────────────────────────────────────────

/// The LLM-routing side of an agent. Cursor has none: it supports session reporting only.
#[derive(Clone, Copy)]
struct Router {
    /// Connects routing, returning the LLM config change it made to the routing agent.
    connect: fn(&RoutingOptions<'_>) -> Result<ConfigChange>,
    disconnect: fn(bool) -> Result<()>,
    status: fn() -> Result<()>,
    summary: fn() -> Result<Option<RoutingSummary>>,
    /// Process names that must be stopped before routing is removed.
    processes: &'static [&'static str],
    /// Whether `connect` can replace an existing connection itself — rewriting its files while
    /// the agent runs, rolling back to the previous setup on failure, and keeping the previous
    /// MCP credential valid through its rotation grace window. Agents without this are
    /// disconnected first, which needs them stopped (or `--force`).
    reconnects_in_place: bool,
    /// What routing writes, and where the MCP gateway is registered — shown before applying.
    routing_target: fn() -> String,
    mcp_target: &'static str,
    /// For agents disconnected before a reconnect: asks the control plane everything the
    /// connect would, changing nothing, so the working connection is only removed once the new
    /// one is known to be accepted.
    check: Option<fn(&RoutingOptions<'_>) -> Result<()>>,
}

fn router(id: &str) -> Option<Router> {
    match id {
        "claude" => Some(Router {
            connect: claude::connect,
            disconnect: claude::disconnect,
            status: claude::status,
            summary: claude::summary,
            processes: &["claude"],
            reconnects_in_place: false,
            routing_target: || {
                format!(
                    "{} (ANTHROPIC_BASE_URL, apiKeyHelper)",
                    integration::tildify(&claude::settings_path())
                )
            },
            mcp_target: "`claude mcp add nasiko` (user scope)",
            check: Some(claude::check),
        }),
        "codex" => Some(Router {
            connect: codex::connect,
            disconnect: codex::disconnect,
            status: codex::status,
            summary: codex::summary,
            processes: &["codex"],
            reconnects_in_place: false,
            routing_target: || {
                format!(
                    "{} (nasiko model provider)",
                    integration::tildify(&codex::config_path())
                )
            },
            mcp_target: "[mcp_servers.nasiko] in the same config.toml",
            check: Some(codex::check),
        }),
        "opencode" => Some(Router {
            connect: opencode::connect,
            disconnect: opencode::disconnect,
            status: opencode::status,
            summary: opencode::summary,
            processes: &["opencode"],
            reconnects_in_place: true,
            routing_target: || integration::tildify(&opencode::plugin_path()),
            mcp_target: "inside the same plugin",
            check: None,
        }),
        _ => None,
    }
}

/// One coding agent with everything the commands show about it.
#[derive(Clone)]
struct AgentView {
    local: LocalAgent,
    router: Option<Router>,
    routing: Option<RoutingSummary>,
    /// Why this agent's routing state couldn't be read; its routing is then left alone.
    routing_error: Option<String>,
}

impl AgentView {
    fn is_connected(&self) -> bool {
        self.routing.is_some() || self.routing_error.is_some() || self.local.reporting_installed
    }
}

/// Every catalogued agent with what is set up for it. One agent's unreadable routing state is
/// recorded on that agent, never failing the others.
fn load_views() -> Result<Vec<AgentView>> {
    Ok(integration::local_agents()?
        .into_iter()
        .map(|local| {
            let router = router(local.id);
            let (routing, routing_error) = match router.map(|router| (router.summary)()) {
                Some(Ok(routing)) => (routing, None),
                Some(Err(error)) => (None, Some(format!("{error:#}"))),
                None => (None, None),
            };
            AgentView {
                local,
                router,
                routing,
                routing_error,
            }
        })
        .collect())
}

fn find_view<'a>(views: &'a [AgentView], id: &str) -> Result<&'a AgentView> {
    views
        .iter()
        .find(|view| view.local.id == id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown coding agent '{id}' — known agents: {}",
                integration::known_agent_ids()
            )
        })
}

// ─── Prompts ─────────────────────────────────────────────────────────────────

/// Where answers come from: the terminal, or — for scripts and `--yes` — the defaults.
trait Prompter {
    fn interactive(&self) -> bool;
    fn confirm(&self, question: &str, default: bool) -> Result<bool>;
    fn select(&self, question: &str, items: &[String], default: usize) -> Result<usize>;
    fn multi_select(
        &self,
        question: &str,
        items: &[String],
        defaults: &[bool],
    ) -> Result<Vec<usize>>;
}

struct Terminal;

impl Prompter for Terminal {
    fn interactive(&self) -> bool {
        true
    }

    fn confirm(&self, question: &str, default: bool) -> Result<bool> {
        Ok(Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(question)
            .default(default)
            .interact()?)
    }

    fn select(&self, question: &str, items: &[String], default: usize) -> Result<usize> {
        Ok(Select::with_theme(&ColorfulTheme::default())
            .with_prompt(question)
            .items(items)
            .default(default)
            .interact()?)
    }

    fn multi_select(
        &self,
        question: &str,
        items: &[String],
        defaults: &[bool],
    ) -> Result<Vec<usize>> {
        Ok(MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt(question)
            .items(items)
            .defaults(defaults)
            .interact()?)
    }
}

struct Unattended;

impl Prompter for Unattended {
    fn interactive(&self) -> bool {
        false
    }

    fn confirm(&self, _question: &str, default: bool) -> Result<bool> {
        Ok(default)
    }

    fn select(&self, _question: &str, _items: &[String], default: usize) -> Result<usize> {
        Ok(default)
    }

    fn multi_select(
        &self,
        _question: &str,
        _items: &[String],
        defaults: &[bool],
    ) -> Result<Vec<usize>> {
        Ok((0..defaults.len()).filter(|&i| defaults[i]).collect())
    }
}

fn prompter(yes: bool) -> Box<dyn Prompter> {
    if !yes && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        Box::new(Terminal)
    } else {
        Box::new(Unattended)
    }
}

// ─── connect ─────────────────────────────────────────────────────────────────

/// What `connect` will do about one agent's LLM routing.
#[derive(Debug, Clone, PartialEq)]
enum RoutingStep {
    Unsupported,
    Skip,
    Keep,
    Connect {
        reconnect: bool,
    },
    /// Re-apply the current connection in place, keeping its agent and MCP setting — what a
    /// script running `connect` on an agent that updates in place (OpenCode) has always done.
    Refresh,
}

/// What `connect` will do about one agent's session reporting.
#[derive(Debug, Clone, PartialEq)]
enum ReportingStep {
    Skip,
    Keep,
    /// Install, or reinstall over an existing hook. `moving_from` names the cluster that
    /// existing reporting delivers to when it is being moved to the active one.
    Install {
        capture_content: bool,
        moving_from: Option<String>,
    },
}

/// Answers shared by every agent connected in one run.
#[derive(Debug, Clone, Default, PartialEq)]
struct RoutingChoices {
    /// `--agent`: the routing agent to use instead of the current or default one.
    agent: Option<String>,
    /// Config name or UUID; `None` keeps each agent's current config (or the user's default).
    llm_config: Option<String>,
    llm_config_label: Option<String>,
    /// The MCP answer for every agent (`--mcp`/`--no-mcp` or the terminal); `None` keeps each
    /// reconnected agent's current setting and gives new connections the gateway.
    mcp: Option<bool>,
}

/// What one agent's routing `connect` is asked to do in this run.
struct RoutingRequest<'a> {
    router: Router,
    reconnect: bool,
    /// Routing agent name or UUID; `None` uses the account's default integration agent.
    agent: Option<&'a str>,
    /// The current connection's agent when it is kept, for the plan's wording.
    reused_agent_name: Option<&'a str>,
    llm_config: Option<&'a str>,
    mcp: bool,
}

/// The routing request for `plan`, or `None` when its routing isn't being (re)connected.
/// Replacing a connection keeps its agent unless `--agent` names another — but only when
/// [`verify_plan`] confirmed that agent is still the active login's. A refresh also keeps the
/// connection's MCP setting rather than adding a gateway it was set up without.
fn routing_request<'a>(
    plan: &'a AgentPlan,
    choices: &'a RoutingChoices,
) -> Option<RoutingRequest<'a>> {
    let router = plan.view.router?;
    let refresh = plan.routing == RoutingStep::Refresh;
    let reconnect = match plan.routing {
        RoutingStep::Connect { reconnect } => reconnect,
        RoutingStep::Refresh => true,
        _ => return None,
    };
    let current = plan.view.routing.as_ref();
    let reused = current.filter(|_| choices.agent.is_none() && plan.current_agent_usable);
    Some(RoutingRequest {
        router,
        reconnect,
        agent: choices
            .agent
            .as_deref()
            .or(reused.map(|current| current.agent_id.as_str())),
        reused_agent_name: reused.map(|current| current.agent_name.as_str()),
        llm_config: choices.llm_config.as_deref().filter(|_| !refresh),
        mcp: choices.mcp.unwrap_or(match current {
            Some(current) if reconnect => current.mcp_installed,
            _ => true,
        }),
    })
}

/// Whether an existing routing connection belongs to `destination`'s cluster and login.
/// A connection recorded without its login (older versions) matches on the cluster alone.
fn owned_by(routing: &RoutingSummary, destination: Option<&ReportingDestination>) -> bool {
    destination.is_some_and(|now| {
        now.is(
            &routing.cluster,
            &routing.cluster_url,
            routing.principal_id.as_deref(),
        )
    })
}

struct AgentPlan {
    view: AgentView,
    routing: RoutingStep,
    reporting: ReportingStep,
    /// Whether the current routing connection is the active login's and its agent still
    /// exists — set by [`verify_plan`]. Only then is that agent reused (by a reconnect, by
    /// reporting next to kept routing) or reconnected to after a failed reconnect.
    current_agent_usable: bool,
    /// Something the plan must say about this agent, such as a reused agent that is gone.
    note: Option<String>,
    /// Why this agent is left unchanged in this run — other agents still go ahead.
    blocked: Option<String>,
    /// Whether reporting that is left alone moves with routing to a new agent (not under
    /// `--no-reporting`, which leaves reporting exactly as it is).
    reporting_follows_routing: bool,
}

impl AgentPlan {
    fn new(view: AgentView, routing: RoutingStep) -> Self {
        Self {
            view,
            routing,
            reporting: ReportingStep::Skip,
            current_agent_usable: false,
            note: None,
            blocked: None,
            reporting_follows_routing: true,
        }
    }

    /// Whether reporting goes under the routing agent rather than the account's default, so
    /// one coding agent is one control-plane agent: routing connected in this run (its agent
    /// was just resolved), or kept routing whose agent [`verify_plan`] confirmed.
    fn reports_under_routing_agent(&self) -> bool {
        match self.routing {
            RoutingStep::Connect { .. } | RoutingStep::Refresh => true,
            RoutingStep::Keep => self.current_agent_usable,
            RoutingStep::Skip | RoutingStep::Unsupported => false,
        }
    }

    fn changes_anything(&self) -> bool {
        self.blocked.is_none()
            && (matches!(
                self.routing,
                RoutingStep::Connect { .. } | RoutingStep::Refresh
            ) || matches!(self.reporting, ReportingStep::Install { .. }))
    }
}

struct Plan {
    agents: Vec<AgentPlan>,
    choices: RoutingChoices,
    /// The active cluster and login: where reporting installed now delivers, and whose agents
    /// an existing connection may be reused for.
    destination: Option<ReportingDestination>,
}

/// `nasiko connect [agent]`: set up LLM routing, the MCP gateway and session reporting.
pub fn connect(target: Option<&str>, options: &ConnectOptions) -> Result<()> {
    let prompter = prompter(options.yes);
    let views = load_views()?;
    let targets = match target {
        Some(id) => {
            let view = find_view(&views, id)?;
            if !view.local.detected {
                bail!(
                    "{} was not found on this machine. Install it, then run `{} connect {id}` again.",
                    view.local.display_name,
                    cli_name()
                );
            }
            vec![view.clone()]
        }
        None if prompter.interactive() => pick_agents(&views, prompter.as_ref())?,
        None => bail!(
            "name the coding agent to connect ({}), or run `{} connect` on a terminal to pick one",
            integration::known_agent_ids(),
            cli_name()
        ),
    };
    if targets.is_empty() {
        println!("No coding agents selected; nothing was changed.");
        return Ok(());
    }
    if options.agent.is_some() && targets.len() > 1 {
        bail!("--agent names one routing agent, so it can be used with one coding agent at a time");
    }
    refuse_routing_flags_without_routing(&targets, options)?;
    ensure_logged_in(prompter.as_ref())?;
    let destination = integration::ReportingDestination::current().ok();

    let mut plan = build_plan(
        targets,
        options,
        destination.as_ref(),
        prompter.as_ref(),
        &fetch_llm_configs,
    )?;
    verify_plan(
        &mut plan,
        &Lookups {
            config: &check_llm_config,
            look_up_agent: &coding_agent_router::look_up_owned_agent,
        },
    )?;
    if !plan.agents.iter().any(AgentPlan::changes_anything) {
        print_nothing_to_do(&plan);
        let blocked = plan
            .agents
            .iter()
            .filter(|agent| agent.blocked.is_some())
            .count();
        if blocked > 0 {
            bail!(
                "{blocked} of {} coding agents could not be connected",
                plan.agents.len()
            );
        }
        return Ok(());
    }
    print_plan(&plan);
    if prompter.interactive() && !prompter.confirm("Apply these changes?", true)? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }

    let failures = apply_plan(&plan, options, prompter.as_ref());
    println!();
    // Showing the result must never hide what failed.
    match load_views() {
        Ok(views) => print_agent_table(
            &views
                .into_iter()
                .filter(|view| plan.agents.iter().any(|p| p.view.local.id == view.local.id))
                .collect::<Vec<_>>(),
        ),
        Err(error) => term::print_warning(&format!("could not show the result: {error:#}")),
    }
    if !failures.is_empty() {
        for (name, error) in &failures {
            term::print_error(&format!("{name}: {error}"));
        }
        bail!(
            "{} of {} coding agents could not be connected",
            failures.len(),
            plan.agents.len()
        );
    }
    println!(
        "\nCheck any time with: {}",
        term::cyan(&format!("{} status", cli_name()))
    );
    Ok(())
}

/// `--agent`, `--config` and `--no-mcp` only mean something for LLM routing; given for agents
/// that have none (Cursor), they would be silently ignored.
fn refuse_routing_flags_without_routing(
    targets: &[AgentView],
    options: &ConnectOptions,
) -> Result<()> {
    if options.mentions_routing() && targets.iter().all(|view| view.router.is_none()) {
        let names: Vec<&str> = targets.iter().map(|view| view.local.display_name).collect();
        bail!(
            "{} supports session reporting only; --agent, --config and --no-mcp don't apply",
            names.join(" and ")
        );
    }
    Ok(())
}

/// Control-plane lookups [`verify_plan`] makes.
struct Lookups<'a> {
    /// Fails, saying why, when an LLM config name or UUID doesn't resolve.
    config: &'a dyn Fn(&str) -> Result<()>,
    /// What the control plane says about an agent (UUID) as a coding agent id's routing agent.
    look_up_agent: &'a dyn Fn(&str, &str) -> Result<AgentLookup>,
}

/// Why a run needs the current routing agent (see [`verify_plan`]).
#[derive(Debug, Clone, Copy, PartialEq)]
enum AgentNeed {
    /// Reconnected or refreshed without `--agent`: it will be the routing agent again.
    Reuse,
    /// A disconnect-first agent reconnected with `--agent`: only to reconnect the old setup
    /// if the new one fails. Agents that update in place roll back themselves.
    Restore,
    /// Routing kept while reporting is installed: reporting joins that agent.
    Report,
}

fn agent_need(agent: &AgentPlan, choices: &RoutingChoices) -> Option<AgentNeed> {
    let in_place = agent
        .view
        .router
        .is_some_and(|router| router.reconnects_in_place);
    match agent.routing {
        RoutingStep::Connect { reconnect: true } | RoutingStep::Refresh
            if choices.agent.is_none() =>
        {
            Some(AgentNeed::Reuse)
        }
        RoutingStep::Connect { reconnect: true } if !in_place => Some(AgentNeed::Restore),
        RoutingStep::Keep if matches!(agent.reporting, ReportingStep::Install { .. }) => {
            Some(AgentNeed::Report)
        }
        _ => None,
    }
}

/// Checks the plan against the control plane before it is shown, so nothing is removed on a
/// mistyped or stale reference. The LLM config must exist (else the run stops: every agent
/// shares it). For each agent whose current routing agent the run needs ([`AgentNeed`]), that
/// agent must still be the user's:
///
/// | lookup                           | Reuse                  | Restore         | Report                 |
/// |----------------------------------|------------------------|-----------------|------------------------|
/// | usable                           | reused                 | restorable      | reporting joins it     |
/// | gone / not yours / refused¹      | default agent + note   | no restore+note | default agent + note   |
/// | refused, connection has its user | agent left unchanged   | no restore+note | default agent + note   |
/// | can't ask                        | agent left unchanged   | agent unchanged | agent left unchanged   |
///
/// ¹ a refusal on a connection that doesn't record its user can't be told apart from "not
/// yours". "Left unchanged" is per agent: the other agents in the run still go ahead.
fn verify_plan(plan: &mut Plan, lookups: &Lookups<'_>) -> Result<()> {
    let connects = plan
        .agents
        .iter()
        .any(|agent| matches!(agent.routing, RoutingStep::Connect { .. }));
    if connects && let Some(config) = &plan.choices.llm_config {
        (lookups.config)(config)?;
    }
    for agent in &mut plan.agents {
        if agent.blocked.is_some() {
            continue;
        }
        let Some(current) = agent.view.routing.clone() else {
            continue;
        };
        let Some(need) = agent_need(agent, &plan.choices) else {
            continue;
        };
        if !owned_by(&current, plan.destination.as_ref()) {
            continue;
        }
        let lookup = match (lookups.look_up_agent)(&current.agent_id, agent.view.local.id) {
            Ok(lookup) => lookup,
            Err(error) => {
                agent.blocked = Some(format!(
                    "could not check its routing agent {} ({error:#}); nothing was changed",
                    current.agent_name
                ));
                continue;
            }
        };
        let refused_own = lookup == AgentLookup::Refused && current.principal_id.is_some();
        if refused_own && need == AgentNeed::Reuse {
            agent.blocked = Some(format!(
                "access to its routing agent {} was refused; nothing was changed. If it is still yours, check your access to it and run connect again",
                current.agent_name
            ));
            continue;
        }
        agent.current_agent_usable = lookup == AgentLookup::Usable;
        if agent.current_agent_usable {
            continue;
        }
        // Reporting reinstalled only to join the routing agent can't join one that is gone or
        // refused: it would go under the default agent it already runs under, so leave it.
        let local = &agent.view.local;
        if need == AgentNeed::Report
            && local.reporting_active
            && matches!(agent.reporting, ReportingStep::Install { moving_from: None, capture_content } if capture_content == local.capture_content)
        {
            agent.reporting = ReportingStep::Keep;
        }
        let why = if refused_own {
            format!("access to agent {} was refused", current.agent_name)
        } else {
            format!(
                "agent {} no longer exists or is no longer yours",
                current.agent_name
            )
        };
        agent.note = Some(match need {
            AgentNeed::Reuse => format!("{why}; your default agent is used instead"),
            AgentNeed::Restore => format!("{why}, so it can't be reconnected if connecting fails"),
            AgentNeed::Report => format!(
                "{why}: session reporting goes under your default agent, and LLM routing stays on {} until you reconnect it ({} connect {} --config <llm-config>)",
                current.agent_name,
                cli_name(),
                agent.view.local.id
            ),
        });
    }
    Ok(())
}

// ─── offered after logging in ────────────────────────────────────────────────

/// What happened when session reporting was offered.
enum Offer {
    /// No terminal to ask on, so nothing was turned on.
    NotAsked,
    Declined,
    /// The plan `connect --no-routing --reporting` would apply, built from the answers.
    Accepted(Plan),
}

/// The flags the offer stands for: reporting only, leaving LLM routing as it is.
fn offer_options() -> ConnectOptions {
    ConnectOptions {
        no_routing: true,
        reporting: true,
        ..ConnectOptions::default()
    }
}

/// After `auth login`, `connect <url>` or `use`: offers session reporting for the detected
/// agents that have none and weren't declined before. It is `connect --no-routing --reporting`
/// for those agents — the same plan, agent choice and content question — but only once the
/// user says yes: without a terminal it says how to turn reporting on, and a no is remembered
/// so it isn't asked again. Agents it doesn't offer are told about, one line each, when their
/// reporting doesn't reach this cluster or was declined. One agent's failure never stops the
/// others.
pub fn offer_reporting() -> Result<()> {
    let views: Vec<AgentView> = load_views()?
        .into_iter()
        .filter(|view| view.local.detected && view.local.reporting_supported)
        .collect();
    let destination = ReportingDestination::current().ok();
    for line in reporting_notices(&views, destination.as_ref()) {
        println!("{line}");
    }
    let offered: Vec<AgentView> = views
        .into_iter()
        .filter(|view| !view.local.reporting_installed && !view.local.reporting_declined)
        .collect();
    if offered.is_empty() {
        return Ok(());
    }
    let ids: Vec<&'static str> = offered.iter().map(|view| view.local.id).collect();
    let turn_on = turn_on_reporting_hint(&ids);
    let prompter = prompter(false);
    match reporting_offer(offered, destination.as_ref(), prompter.as_ref())? {
        Offer::NotAsked => term::print_info(&format!(
            "Session reporting is off. Turn it on with: {}",
            term::cyan(&turn_on)
        )),
        Offer::Declined => {
            integration::record_declined(&ids)?;
            term::print_info(&format!(
                "Session reporting left off; it won't be offered again. Turn it on any time with: {}",
                term::cyan(&turn_on)
            ));
        }
        Offer::Accepted(mut plan) => {
            verify_plan(
                &mut plan,
                &Lookups {
                    config: &check_llm_config,
                    look_up_agent: &coding_agent_router::look_up_owned_agent,
                },
            )?;
            print_plan(&plan);
            for (name, error) in apply_plan(&plan, &offer_options(), prompter.as_ref()) {
                term::print_warning(&format!("{name}: {error}"));
            }
        }
    }
    Ok(())
}

/// One line for each agent the offer skips but the user should hear about: reporting installed
/// that doesn't reach this cluster and account, or whose hook isn't working, and reporting
/// declined before. Agents reporting here as they should are not mentioned.
fn reporting_notices(
    views: &[AgentView],
    destination: Option<&ReportingDestination>,
) -> Vec<String> {
    views
        .iter()
        .filter_map(|view| {
            let local = &view.local;
            let name = local.display_name;
            let command = turn_on_reporting_hint(&[local.id]);
            if local.reporting_installed {
                if let Some((from, _)) = reporting_elsewhere(local, destination) {
                    return Some(format!(
                        "{name} still reports sessions to {from}; move it here with: {command}"
                    ));
                }
                return (!local.reporting_active).then(|| {
                    format!(
                        "{name}'s session reporting hook {}; reinstall it with: {command}",
                        stale_reason(local)
                    )
                });
            }
            local.reporting_declined.then(|| {
                format!("{name}: session reporting not offered (you said no before); turn it on with: {command}")
            })
        })
        .collect()
}

/// The command that turns reporting on, without touching LLM routing: concrete for one agent,
/// with the ids listed for several.
fn turn_on_reporting_hint(ids: &[&str]) -> String {
    match ids {
        [id] => format!("{} connect {id} --no-routing --reporting", cli_name()),
        ids => format!(
            "{} connect <agent> --no-routing --reporting (agent: {})",
            cli_name(),
            ids.join(", ")
        ),
    }
}

/// Asks whether to report sessions from `targets` — defaulting to no, since nobody asked for
/// it — and, on a yes, builds the reporting plan, which asks about prompt and response text.
fn reporting_offer(
    targets: Vec<AgentView>,
    destination: Option<&ReportingDestination>,
    prompter: &dyn Prompter,
) -> Result<Offer> {
    if !prompter.interactive() {
        return Ok(Offer::NotAsked);
    }
    let names: Vec<&str> = targets.iter().map(|view| view.local.display_name).collect();
    let question = format!(
        "Report sessions from {} to Nasiko observability?",
        readable_list(&names)
    );
    if !prompter.confirm(&question, false)? {
        return Ok(Offer::Declined);
    }
    let plan = build_plan(
        targets,
        &offer_options(),
        destination,
        prompter,
        &fetch_llm_configs,
    )?;
    Ok(Offer::Accepted(plan))
}

fn check_llm_config(reference: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    crate::commands::llm_config::fetch_config_by_ref(&client, reference)
        .map(|_| ())
        .map_err(|error| {
            anyhow::anyhow!(
                "{error:#}; nothing was changed. See your LLM configs with: {} list",
                llm_config_hint()
            )
        })
}

fn pick_agents(views: &[AgentView], prompter: &dyn Prompter) -> Result<Vec<AgentView>> {
    println!("Local coding agents:");
    print_agent_table(views);
    println!();
    let detected: Vec<&AgentView> = views.iter().filter(|view| view.local.detected).collect();
    if detected.is_empty() {
        bail!(
            "no supported coding agents were found on this machine ({})",
            integration::known_agent_ids()
        );
    }
    let labels: Vec<String> = detected
        .iter()
        .map(|view| {
            let mut label = format!("{} ({})", view.local.display_name, view.local.id);
            if view.is_connected() {
                label.push_str(" — already connected; select to change its setup");
            } else if view.router.is_none() {
                label.push_str(" — session reporting only");
            }
            label
        })
        .collect();
    let defaults: Vec<bool> = detected.iter().map(|view| !view.is_connected()).collect();
    let chosen = prompter.multi_select(
        "Which coding agents do you want to connect? (space selects, enter confirms)",
        &labels,
        &defaults,
    )?;
    Ok(chosen.into_iter().map(|i| detected[i].clone()).collect())
}

/// Requires a usable login on the active control plane, offering to log in on a terminal.
fn ensure_logged_in(prompter: &dyn Prompter) -> Result<()> {
    let (cluster, entry) = crate::config::active_cluster().map_err(|_| {
        anyhow::anyhow!(
            "no control plane is connected; first run: {} connect <control-plane-url>",
            cli_name()
        )
    })?;
    if let Err(error) = coding_agent_router::require_current_login() {
        if !prompter.interactive() {
            return Err(error);
        }
        term::print_warning(&format!("{error:#}"));
        if !prompter.confirm(&format!("Log in to {cluster} now?"), true)? {
            bail!("a Nasiko login is required to connect coding agents");
        }
        auth::login_without_setup()?;
        coding_agent_router::require_current_login()?;
    }
    let user = crate::config::active_cluster()
        .ok()
        .and_then(|(_, current)| current.username)
        .map(|user| format!(", logged in as {user}"))
        .unwrap_or_default();
    println!("Control plane: {cluster} ({}){user}", entry.url);
    Ok(())
}

fn fetch_llm_configs() -> Result<Vec<Value>> {
    unwrap_data(Client::from_active_cluster()?.get_json("/llm-configs")?)
}

/// Decides every step before anything changes, asking on a terminal and reading flags otherwise.
/// `destination` is where reporting would deliver if installed now (the active cluster).
fn build_plan(
    targets: Vec<AgentView>,
    options: &ConnectOptions,
    destination: Option<&ReportingDestination>,
    prompter: &dyn Prompter,
    load_configs: &dyn Fn() -> Result<Vec<Value>>,
) -> Result<Plan> {
    let mut agents: Vec<AgentPlan> = Vec::with_capacity(targets.len());
    for view in targets {
        // Routing state that can't be read is left alone; only a run that would change routing
        // has to stop for it.
        if let Some(error) = view.routing_error.clone() {
            let mut planned = AgentPlan::new(view, RoutingStep::Keep);
            if !options.no_routing {
                planned.blocked = Some(format!(
                    "its LLM routing state can't be read ({error}); nothing was changed"
                ));
            }
            agents.push(planned);
            continue;
        }
        let routing = existing_routing_step(&view, options, destination, prompter)?;
        let note = view
            .routing
            .as_ref()
            .filter(|current| {
                routing == RoutingStep::Keep && made_as_another_account(current, destination)
            })
            .map(|current| {
                format!(
                    "LLM routing was set up as another account on {}, so its LLM calls fail for you; reconnect it: {} connect {} --config <llm-config>",
                    current.cluster,
                    cli_name(),
                    view.local.id
                )
            });
        let mut planned = AgentPlan::new(view, routing);
        planned.note = note;
        planned.reporting_follows_routing = !options.no_reporting;
        agents.push(planned);
    }

    // New routing: one yes/no for every agent that has none yet.
    let new_routing: Vec<usize> = (0..agents.len())
        .filter(|&i| agents[i].routing == RoutingStep::Connect { reconnect: false })
        .collect();
    if !new_routing.is_empty()
        && prompter.interactive()
        && options.config.is_none()
        && options.agent.is_none()
        && !prompter.confirm(
            &format!(
                "Route LLM calls from {} through Nasiko?",
                names(&agents, &new_routing)
            ),
            true,
        )?
    {
        for &i in &new_routing {
            agents[i].routing = RoutingStep::Skip;
        }
    }

    let mut choices = RoutingChoices {
        agent: options.agent.clone(),
        mcp: options.mcp_choice(),
        ..RoutingChoices::default()
    };
    let routed: Vec<usize> = (0..agents.len())
        .filter(|&i| matches!(agents[i].routing, RoutingStep::Connect { .. }))
        .collect();
    if !routed.is_empty() {
        let reconnecting = routed
            .iter()
            .any(|&i| agents[i].routing == RoutingStep::Connect { reconnect: true });
        match choose_llm_config(options, prompter, load_configs, reconnecting)? {
            Some(chosen) => {
                choices.llm_config = chosen.reference;
                choices.llm_config_label = chosen.label;
            }
            None => {
                for &i in &routed {
                    agents[i].routing = if agents[i].view.routing.is_some() {
                        RoutingStep::Keep
                    } else {
                        RoutingStep::Skip
                    };
                }
            }
        }
    }
    let routed: Vec<usize> = (0..agents.len())
        .filter(|&i| matches!(agents[i].routing, RoutingStep::Connect { .. }))
        .collect();
    if !routed.is_empty() && choices.mcp.is_none() && prompter.interactive() {
        // The suggested answer keeps what is there: on, unless every agent being reconnected
        // was set up without the gateway.
        let suggested = routed.iter().any(|&i| match &agents[i].view.routing {
            Some(current) if agents[i].routing == (RoutingStep::Connect { reconnect: true }) => {
                current.mcp_installed
            }
            _ => true,
        });
        choices.mcp = Some(prompter.confirm(
            &format!(
                "Give {} your MCP connectors through the Nasiko gateway?",
                names(&agents, &routed)
            ),
            suggested,
        )?);
    }

    // Reporting already installed — working or not — is checked on its own, so its saved
    // settings are kept; the rest get one shared yes/no.
    let mut unreported = Vec::new();
    for (i, agent) in agents.iter_mut().enumerate() {
        if agent.view.local.reporting_installed {
            agent.reporting = existing_reporting_step(agent, options, destination, prompter)?;
        } else {
            unreported.push(i);
        }
    }
    if !unreported.is_empty() {
        let report = if options.no_reporting {
            false
        } else if options.mentions_reporting() {
            true
        } else if prompter.interactive() {
            prompter.confirm(
                &format!(
                    "Report sessions from {} to Nasiko observability?",
                    names(&agents, &unreported)
                ),
                true,
            )?
        } else {
            false
        };
        if report {
            let capture_content = match options.content_choice() {
                Some(choice) => choice,
                None if prompter.interactive() => prompter.confirm(
                    "Include prompt and response text? (No sends only tokens, latency and cost)",
                    true,
                )?,
                // A script asked for reporting, not for text: that takes --content.
                None => false,
            };
            for &i in &unreported {
                agents[i].reporting = ReportingStep::Install {
                    capture_content,
                    moving_from: None,
                };
            }
        }
    }
    Ok(Plan {
        agents,
        choices,
        destination: destination.cloned(),
    })
}

/// Installed reporting stays as it is, unless it delivers to another cluster or account than
/// the active one (moved when asked, or when the flags ask for reporting), its hook needs
/// reinstalling (likewise), it runs under another agent than the LLM routing that is kept
/// (likewise — one coding agent is one control-plane agent), or the flags ask for a different
/// content setting. Whatever is reinstalled keeps the saved content setting unless `--content`
/// or `--no-content` says otherwise, so prompt and response text is never turned on unasked.
fn existing_reporting_step(
    agent: &AgentPlan,
    options: &ConnectOptions,
    destination: Option<&ReportingDestination>,
    prompter: &dyn Prompter,
) -> Result<ReportingStep> {
    let local = &agent.view.local;
    if options.no_reporting {
        return Ok(ReportingStep::Keep);
    }
    let capture_content = options.content_choice().unwrap_or(local.capture_content);
    let asked = |question: String| -> Result<bool> {
        Ok(options.mentions_reporting()
            || (prompter.interactive() && prompter.confirm(&question, true)?))
    };
    let install = |moving_from: Option<String>| ReportingStep::Install {
        capture_content,
        moving_from,
    };
    if let Some((from, to)) = reporting_elsewhere(local, destination) {
        let question = format!(
            "{} reports sessions to {from}. Move reporting to {to}?",
            local.display_name
        );
        return Ok(if asked(question)? {
            install(Some(from))
        } else {
            ReportingStep::Keep
        });
    }
    if !local.reporting_active {
        let question = format!(
            "{}'s session reporting hook {}. Reinstall it?",
            local.display_name,
            stale_reason(local)
        );
        return Ok(if asked(question)? {
            install(None)
        } else {
            ReportingStep::Keep
        });
    }
    // Routing replaced in this run may change agent: that is handled once it has (apply_agent).
    if agent.routing == RoutingStep::Keep
        && let Some(routing_agent) = misaligned_routing_agent(agent, destination)
    {
        let question = format!(
            "{} reports sessions under agent {}, but its LLM routing uses agent {routing_agent}. Report under {routing_agent}?",
            local.display_name,
            local.reporting_agent_name.as_deref().unwrap_or("?")
        );
        if asked(question)? {
            return Ok(install(None));
        }
    }
    Ok(if capture_content != local.capture_content {
        install(None)
    } else {
        ReportingStep::Keep
    })
}

/// Where installed reporting delivers when that isn't the active cluster and account, as
/// `(from, to)` for a sentence: `("staging", "prod")`, or `("another account on prod",
/// "your account on prod")` when only the account differs.
fn reporting_elsewhere(
    local: &LocalAgent,
    destination: Option<&ReportingDestination>,
) -> Option<(String, String)> {
    let now = destination?;
    match &local.reporting_destination {
        Some(bound) if bound.same_as(now) => None,
        Some(bound) if now.is(&bound.cluster_name, &bound.cluster_url, None) => Some((
            format!("another account on {}", bound.cluster_name),
            format!("your account on {}", now.cluster_name),
        )),
        Some(bound) => Some((bound.cluster_name.clone(), now.cluster_name.clone())),
        None => Some((
            "an older install with no cluster recorded".to_string(),
            now.cluster_name.clone(),
        )),
    }
}

/// The agent kept LLM routing uses, when reporting runs under a different one — so one coding
/// agent would show up as two control-plane agents. Only routing that belongs to the active
/// login counts; reporting must also deliver there.
fn misaligned_routing_agent<'a>(
    agent: &'a AgentPlan,
    destination: Option<&ReportingDestination>,
) -> Option<&'a str> {
    let routing = agent.view.routing.as_ref()?;
    let local = &agent.view.local;
    let aligned = local.reporting_agent_name.as_deref() == Some(routing.agent_name.as_str());
    (owned_by(routing, destination)
        && reporting_elsewhere(local, destination).is_none()
        && !aligned)
        .then_some(routing.agent_name.as_str())
}

/// Whether routing was set up on the active cluster as another account — its credential helper
/// refuses every call for the user logged in now.
fn made_as_another_account(
    routing: &RoutingSummary,
    destination: Option<&ReportingDestination>,
) -> bool {
    destination.is_some_and(|now| now.is(&routing.cluster, &routing.cluster_url, None))
        && !owned_by(routing, destination)
}

/// Why an installed reporting hook isn't active, as the end of a sentence about the hook.
fn stale_reason(local: &LocalAgent) -> String {
    format!("isn't working ({})", local.reporting_status)
}

/// The routing step before the shared questions: unsupported, keep what's there, or connect.
fn existing_routing_step(
    view: &AgentView,
    options: &ConnectOptions,
    destination: Option<&ReportingDestination>,
    prompter: &dyn Prompter,
) -> Result<RoutingStep> {
    let Some(router) = view.router else {
        return Ok(RoutingStep::Unsupported);
    };
    let Some(current) = &view.routing else {
        return Ok(if options.no_routing {
            RoutingStep::Skip
        } else {
            RoutingStep::Connect { reconnect: false }
        });
    };
    if options.no_routing {
        return Ok(RoutingStep::Keep);
    }
    // A flag that changes the connection answers "keep or reconnect?" the same way for every
    // agent, on a terminal or not. An MCP flag is a change only when it differs from what is set.
    let asked_to_change = options.config.is_some()
        || options.agent.is_some()
        || options
            .mcp_choice()
            .is_some_and(|mcp| mcp != current.mcp_installed);
    if asked_to_change {
        return Ok(RoutingStep::Connect { reconnect: true });
    }
    if !prompter.interactive() {
        // Scripts only replace a connection they explicitly asked to change. One that updates
        // in place (OpenCode) is refreshed as `connect` always did — unless the flags are about
        // reporting, which leaves routing alone.
        return Ok(
            if router.reconnects_in_place && !options.mentions_reporting() {
                RoutingStep::Refresh
            } else {
                RoutingStep::Keep
            },
        );
    }
    // Routing made as another account doesn't work for this login: reconnecting is suggested.
    let broken = made_as_another_account(current, destination);
    let keep = if broken {
        format!(
            "Keep it (agent {} on {}) — set up as another account, so its LLM calls fail for you",
            current.agent_name, current.cluster
        )
    } else {
        format!(
            "Keep it (agent {} on {})",
            current.agent_name, current.cluster
        )
    };
    let items = vec![keep, "Reconnect with new settings".to_string()];
    let choice = prompter.select(
        &format!(
            "{} LLM routing is already connected.",
            view.local.display_name
        ),
        &items,
        usize::from(broken),
    )?;
    Ok(if choice == 1 {
        RoutingStep::Connect { reconnect: true }
    } else {
        RoutingStep::Keep
    })
}

/// An LLM config picked for routing. `reference` is `None` when each agent keeps its current
/// config (or the user's default).
struct ConfigChoice {
    reference: Option<String>,
    label: Option<String>,
}

/// The LLM config to route through, or `None` when the user chose not to route after all.
/// When an agent is being reconnected, the first choice keeps the routing agent's current
/// config, so accepting the suggested answer never switches the model.
fn choose_llm_config(
    options: &ConnectOptions,
    prompter: &dyn Prompter,
    load_configs: &dyn Fn() -> Result<Vec<Value>>,
    reconnecting: bool,
) -> Result<Option<ConfigChoice>> {
    let keep_current = ConfigChoice {
        reference: None,
        label: None,
    };
    if let Some(config) = &options.config {
        return Ok(Some(ConfigChoice {
            reference: Some(config.clone()),
            label: Some(config.clone()),
        }));
    }
    if !prompter.interactive() {
        return Ok(Some(keep_current));
    }
    let configs = load_configs()?;
    if configs.is_empty() && !reconnecting {
        term::print_warning(&format!(
            "You have no LLM configs yet. Create one with `{}`.",
            term::cyan_stderr(&format!(
                "{} create --name <name> --provider <provider> --model <model> --default",
                llm_config_hint()
            ))
        ));
        if prompter.confirm("Continue without LLM routing?", true)? {
            return Ok(None);
        }
        bail!("an LLM config is required for LLM routing");
    }
    let mut items = Vec::new();
    if reconnecting {
        items.push("Keep the routing agent's current LLM config".to_string());
    }
    let first_config = items.len();
    items.extend(configs.iter().map(config_label));
    items.push(if reconnecting {
        "Don't change LLM routing (connected agents stay as they are)".to_string()
    } else {
        "Don't route LLM calls through Nasiko".to_string()
    });
    let default = if reconnecting {
        0
    } else {
        configs
            .iter()
            .position(|config| config.get("is_default").and_then(Value::as_bool) == Some(true))
            .unwrap_or(0)
    };
    let choice = prompter.select("LLM config to route through:", &items, default)?;
    if reconnecting && choice == 0 {
        return Ok(Some(keep_current));
    }
    let Some(config) = choice
        .checked_sub(first_config)
        .and_then(|i| configs.get(i))
    else {
        return Ok(None);
    };
    let reference = config
        .get("id")
        .or_else(|| config.get("name"))
        .and_then(Value::as_str)
        .context("LLM config is missing an id")?;
    let name = config
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(reference);
    Ok(Some(ConfigChoice {
        reference: Some(reference.to_string()),
        label: Some(name.to_string()),
    }))
}

fn config_label(config: &Value) -> String {
    let field = |key: &str| config.get(key).and_then(Value::as_str).unwrap_or("?");
    let default = if config.get("is_default").and_then(Value::as_bool) == Some(true) {
        " — your default"
    } else {
        ""
    };
    format!(
        "{} ({}/{}){default}",
        field("name"),
        field("provider"),
        field("model")
    )
}

fn names(agents: &[AgentPlan], indexes: &[usize]) -> String {
    let names: Vec<&str> = indexes
        .iter()
        .map(|&i| agents[i].view.local.display_name)
        .collect();
    readable_list(&names)
}

/// `a`, `a and b`, `a, b and c`.
fn readable_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn print_plan(plan: &Plan) {
    println!("\nPlan:");
    for agent in &plan.agents {
        println!("  {}", agent.view.local.display_name);
        if let Some(reason) = &agent.blocked {
            println!("    • Not changed: {reason}");
            continue;
        }
        print_routing_line(agent, plan);
        if let Some(note) = &agent.note {
            println!("    • Note: {note}");
        }
        println!("    • Session reporting: {}", reporting_line(agent, plan));
    }
    println!();
}

fn print_routing_line(agent: &AgentPlan, plan: &Plan) {
    let Some(request) = routing_request(agent, &plan.choices) else {
        let what = match agent.routing {
            RoutingStep::Keep => "keep the current connection",
            RoutingStep::Skip => "not set up",
            _ => "not supported",
        };
        println!("    • LLM routing: {what}");
        return;
    };
    let router = request.router;
    let config = plan
        .choices
        .llm_config_label
        .as_deref()
        .filter(|_| request.llm_config.is_some())
        .map(|label| format!(" via LLM config {label}"))
        .unwrap_or_default();
    let verb = match (request.reconnect, router.reconnects_in_place) {
        (false, _) => "connect",
        (true, true) => "update in place",
        (true, false) => "reconnect",
    };
    let kept_agent = request
        .reused_agent_name
        .map(|name| format!(", keeping agent {name}"))
        .unwrap_or_default();
    let current = agent.view.routing.as_ref();
    // A connection made as another user or on another cluster is replaced, not kept.
    let replaced = current
        .filter(|current| !owned_by(current, plan.destination.as_ref()))
        .map(|current| {
            if made_as_another_account(current, plan.destination.as_ref()) {
                ", replacing a connection made as another user".to_string()
            } else {
                format!(", replacing its connection to {}", current.cluster)
            }
        })
        .unwrap_or_default();
    println!(
        "    • LLM routing: {verb}{config}{kept_agent}{replaced} → {}",
        (router.routing_target)()
    );
    let had_mcp = request.reconnect && current.is_some_and(|current| current.mcp_installed);
    match (request.mcp, had_mcp) {
        (true, _) => println!("    • MCP gateway: {}", router.mcp_target),
        (false, true) => println!("    • MCP gateway: removed"),
        (false, false) => println!("    • MCP gateway: not added"),
    }
}

/// What happens to `agent`'s session reporting, as the plan shows it — including where reporting
/// that is left alone actually goes.
fn reporting_line(agent: &AgentPlan, plan: &Plan) -> String {
    let local = &agent.view.local;
    let destination = plan.destination.as_ref();
    let content = |capture: bool| {
        if capture {
            "with prompt and response text"
        } else {
            "tokens, latency and cost only"
        }
    };
    let turn_on = format!(
        "{} connect {} --no-routing --reporting",
        cli_name(),
        local.id
    );
    match &agent.reporting {
        ReportingStep::Install {
            capture_content,
            moving_from: Some(from),
        } => {
            let to = reporting_elsewhere(local, destination)
                .map_or_else(|| "the active cluster".to_string(), |(_, to)| to);
            format!("move from {from} to {to} ({})", content(*capture_content))
        }
        ReportingStep::Install {
            capture_content, ..
        } if local.reporting_active && *capture_content != local.capture_content => {
            format!("switch to {}", content(*capture_content))
        }
        ReportingStep::Install {
            capture_content, ..
        } if local.reporting_active => format!(
            "move under agent {}, the one LLM routing uses ({})",
            agent
                .view
                .routing
                .as_ref()
                .map_or("?", |routing| routing.agent_name.as_str()),
            content(*capture_content)
        ),
        ReportingStep::Install {
            capture_content, ..
        } if local.reporting_installed => format!(
            "reinstall the hook in {} ({})",
            local.hook_location,
            content(*capture_content)
        ),
        ReportingStep::Install {
            capture_content, ..
        } => format!(
            "hook in {} ({})",
            local.hook_location,
            content(*capture_content)
        ),
        ReportingStep::Keep if !local.reporting_installed => "not set up".to_string(),
        ReportingStep::Keep if !local.reporting_active => format!(
            "left as it is — the hook {}; reinstall it with: {turn_on}",
            stale_reason(local)
        ),
        ReportingStep::Keep => match reporting_elsewhere(local, destination) {
            Some((from, _)) => format!("on, but reporting to {from}; move it with: {turn_on}"),
            None if agent.reporting_follows_routing
                && matches!(
                    agent.routing,
                    RoutingStep::Connect { .. } | RoutingStep::Refresh
                ) =>
            {
                "already on (it follows LLM routing to its agent)".to_string()
            }
            None => "already on".to_string(),
        },
        ReportingStep::Skip => "not set up".to_string(),
    }
}

/// When `connect` has nothing to change: why, per agent, and how to change it.
fn print_nothing_to_do(plan: &Plan) {
    term::print_info("Nothing to change.");
    let cli = cli_name();
    for agent in &plan.agents {
        let name = agent.view.local.display_name;
        let id = agent.view.local.id;
        if let Some(reason) = &agent.blocked {
            term::print_info(&format!("{name}: not changed — {reason}"));
            continue;
        }
        if let Some(note) = &agent.note {
            term::print_info(&format!("{name}: {note}"));
        } else if agent.routing == RoutingStep::Unsupported
            && agent.reporting == ReportingStep::Skip
        {
            term::print_info(&format!(
                "{name} supports session reporting only: {}",
                term::cyan(&format!("{cli} connect {id} --reporting"))
            ));
        } else if agent.routing == RoutingStep::Keep {
            term::print_info(&format!(
                "{name} is already connected. To change it: {} (or run it on a terminal).",
                term::cyan(&format!("{cli} connect {id} --config <name>"))
            ));
        }
        if agent.view.local.reporting_installed
            && (!agent.view.local.reporting_active
                || reporting_elsewhere(&agent.view.local, plan.destination.as_ref()).is_some())
        {
            println!(
                "{name}'s session reporting: {}",
                reporting_line(agent, plan)
            );
        }
    }
}

/// Applies each agent's plan in turn. One agent's failure never stops the others, and an agent
/// is never left half-connected: if its reporting can't be set up, routing this run created
/// from nothing is removed again (routing it reconnected is kept — the old one is gone).
fn apply_plan(
    plan: &Plan,
    options: &ConnectOptions,
    prompter: &dyn Prompter,
) -> Vec<(&'static str, String)> {
    let mut failures = Vec::new();
    for agent in &plan.agents {
        let name = agent.view.local.display_name;
        if let Some(reason) = &agent.blocked {
            failures.push((name, reason.clone()));
            continue;
        }
        if !agent.changes_anything() {
            continue;
        }
        println!("\n{}", term::bold(&format!("── {name} ──")));
        if let Err(error) = apply_agent(
            agent,
            &plan.choices,
            plan.destination.as_ref(),
            options,
            prompter,
        ) {
            failures.push((name, format!("{error:#}")));
        }
    }
    failures
}

fn apply_agent(
    agent: &AgentPlan,
    choices: &RoutingChoices,
    destination: Option<&ReportingDestination>,
    options: &ConnectOptions,
    prompter: &dyn Prompter,
) -> Result<()> {
    let id = agent.view.local.id;
    // A routing failure stops this agent before its reporting step, so say that step was
    // skipped and how to run it on its own.
    let routed = apply_routing(agent, choices, options, prompter).map_err(|error| {
        if matches!(agent.reporting, ReportingStep::Install { .. }) {
            error.context(format!(
                "session reporting was not changed either; set it up on its own with: {} connect {id} --no-routing --reporting",
                cli_name()
            ))
        } else {
            error
        }
    })?;
    match agent.reporting {
        ReportingStep::Install {
            capture_content, ..
        } => install_reporting(agent, capture_content, routed),
        ReportingStep::Keep if routed.is_some() && agent.reporting_follows_routing => {
            follow_routing_agent(agent, destination)
        }
        ReportingStep::Keep | ReportingStep::Skip => Ok(()),
    }
}

/// What applying an agent's routing did, for the reporting step after it.
enum Routed {
    /// Connected from nothing, with the LLM config change it made — undone with it if
    /// reporting then fails.
    New(ConfigChange),
    /// An existing connection was replaced or refreshed.
    Replaced,
}

/// Applies the routing part of `agent`'s plan; `None` when routing is left as it is.
fn apply_routing(
    agent: &AgentPlan,
    choices: &RoutingChoices,
    options: &ConnectOptions,
    prompter: &dyn Prompter,
) -> Result<Option<Routed>> {
    let name = agent.view.local.display_name;
    let Some(request) = routing_request(agent, choices) else {
        return Ok(None);
    };
    let router = request.router;
    let mut routing = RoutingOptions {
        agent: request.agent,
        llm_config: request.llm_config,
        mcp: request.mcp,
        checked: false,
    };
    // An in-place reconnect is the agent's own `connect`, which rolls itself back. Others
    // must be disconnected first — only once the new routing is known to be accepted.
    let disconnect_first = request.reconnect && !router.reconnects_in_place;
    if disconnect_first {
        // Asked first: a reconnect that won't go ahead must not touch the control plane.
        let force = allowed_while_running(name, router, options.force, "Reconnect", prompter)?;
        if let Some(check) = router.check {
            check(&routing).map_err(|error| {
                let config_not_restored =
                    error.chain().any(|cause| cause.is::<ConfigNotRestored>());
                error.context(if config_not_restored {
                    "the current connection was not removed, but checking the new LLM routing \
                     left the agent's LLM config changed"
                } else {
                    "the new LLM routing would not work; the current connection was left as it is"
                })
            })?;
            routing.checked = true;
        }
        (router.disconnect)(force).context("could not remove the current LLM routing")?;
    }
    match (router.connect)(&routing) {
        Ok(_) if request.reconnect => Ok(Some(Routed::Replaced)),
        Ok(change) => Ok(Some(Routed::New(change))),
        Err(error) if disconnect_first => {
            Err(restore_previous_routing(agent, router, &routing, error))
        }
        Err(error) => Err(error.context("LLM routing failed")),
    }
}

/// Installs or reinstalls reporting. If that fails, routing this run connected from nothing is
/// removed again with the LLM config change it made, so the agent isn't left half-connected;
/// routing it replaced is kept (the old one is gone).
fn install_reporting(
    agent: &AgentPlan,
    capture_content: bool,
    routed: Option<Routed>,
) -> Result<()> {
    let result = reporting_agent_name(agent).and_then(|agent_name| {
        integration::install(InstallOptions {
            agent_id: agent.view.local.id,
            no_content: !capture_content,
            agent_name: agent_name.as_deref(),
        })
    });
    let Err(error) = result else {
        return Ok(());
    };
    if let (Some(Routed::New(change)), Some(router)) = (&routed, agent.view.router) {
        term::print_warning(
            "Session reporting failed; removing the LLM routing set up a moment ago.",
        );
        if let Err(rollback) = (router.disconnect)(true) {
            term::print_warning(&format!("could not remove LLM routing: {rollback:#}"));
        }
        if let Err(put_back) = change.undo() {
            term::print_warning(&format!("{put_back:#}"));
        }
        return Err(error.context("session reporting failed"));
    }
    let kept = if routed.is_some() {
        "LLM routing was reconnected and kept; "
    } else {
        ""
    };
    Err(error.context(format!(
        "session reporting failed ({kept}retry with: {} connect {} --no-routing --reporting)",
        cli_name(),
        agent.view.local.id
    )))
}

/// After routing was replaced: reporting that delivers here but runs under another agent than
/// routing now uses is moved under it, keeping its content setting — one coding agent stays
/// one control-plane agent.
fn follow_routing_agent(
    agent: &AgentPlan,
    destination: Option<&ReportingDestination>,
) -> Result<()> {
    let local = &agent.view.local;
    if !local.reporting_active || reporting_elsewhere(local, destination).is_some() {
        return Ok(());
    }
    let Some(router) = agent.view.router else {
        return Ok(());
    };
    let Some(routing) = (router.summary)()? else {
        return Ok(());
    };
    if local.reporting_agent_name.as_deref() == Some(routing.agent_name.as_str()) {
        return Ok(());
    }
    term::print_info(&format!(
        "Session reporting follows LLM routing to agent {}.",
        routing.agent_name
    ));
    integration::install(InstallOptions {
        agent_id: local.id,
        no_content: !local.capture_content,
        agent_name: Some(&routing.agent_name),
    })
    .with_context(|| {
        format!(
            "LLM routing was reconnected, but session reporting still goes under agent {}; retry with: {} connect {} --no-routing --reporting",
            local.reporting_agent_name.as_deref().unwrap_or("?"),
            cli_name(),
            local.id
        )
    })
}

/// After a reconnect removed the previous routing and connecting the new one failed: connects
/// the previous setup again — its agent, that agent's LLM config (a failed connect restores it)
/// and its MCP setting — so a reconnect doesn't leave the agent without routing. Only possible
/// when that connection was the active login's and its agent still exists. The returned error
/// says how it ended.
fn restore_previous_routing(
    agent: &AgentPlan,
    router: Router,
    tried: &RoutingOptions<'_>,
    error: anyhow::Error,
) -> anyhow::Error {
    let retry = format!("run `{} connect {}` again", cli_name(), agent.view.local.id);
    let Some(previous) = agent
        .view
        .routing
        .as_ref()
        .filter(|_| agent.current_agent_usable)
    else {
        return error.context(format!(
            "LLM routing failed after the previous connection was removed; {retry}"
        ));
    };
    term::print_warning(&format!(
        "{}: the new LLM routing failed; reconnecting the previous one.",
        agent.view.local.display_name
    ));
    let previous_setup = RoutingOptions {
        agent: Some(&previous.agent_id),
        llm_config: None,
        mcp: previous.mcp_installed,
        checked: false,
    };
    let config_changed = error.chain().any(|cause| cause.is::<ConfigNotRestored>());
    let tried_previous_agent = tried.agent == Some(previous.agent_id.as_str());
    match (router.connect)(&previous_setup).map(|_| ()) {
        Ok(()) if config_changed && tried_previous_agent => error.context(
            "LLM routing failed; the previous connection was reconnected, but its agent keeps the \
             LLM config being tried, which could not be put back",
        ),
        Ok(()) if config_changed => error.context(format!(
            "LLM routing failed; the previous connection was restored, but {} keeps the LLM config \
             being tried, which could not be put back",
            tried
                .agent
                .map_or("your default agent".to_string(), |agent| format!("agent {agent}"))
        )),
        Ok(()) => error.context("LLM routing failed; the previous connection was restored"),
        Err(restore) => error.context(format!(
            "LLM routing failed, and reconnecting the previous one failed too ({restore:#}); {retry}"
        )),
    }
}

/// Whether an action that removes `name`'s routing may go ahead (`true` = force past a running
/// agent). Already forced, or the agent isn't running: go ahead. On a terminal, a running agent
/// is a question; unattended, it is refused here — before anything is changed — with the
/// preflight's own error, which names `--force`.
fn allowed_while_running(
    name: &str,
    router: Router,
    force: bool,
    action: &str,
    prompter: &dyn Prompter,
) -> Result<bool> {
    if force {
        return Ok(true);
    }
    let not_running = coding_agent_router::disconnect_preflight(name, router.processes, false);
    if not_running.is_ok() {
        return Ok(false);
    }
    if !prompter.interactive() {
        return not_running.map(|()| false);
    }
    let anyway = prompter.confirm(
        &format!(
            "{name} is still running; open sessions may fail API requests until restarted. {action} anyway?"
        ),
        false,
    )?;
    if !anyway {
        bail!("left as it was; close {name} and run the command again, or pass --force");
    }
    Ok(true)
}

/// The agent reporting is installed under: the routing agent when
/// [`AgentPlan::reports_under_routing_agent`], so one coding agent shows up as one agent;
/// otherwise `None`, the account's default. Read after routing is applied, so it sees a
/// connection made in the same run.
fn reporting_agent_name(agent: &AgentPlan) -> Result<Option<String>> {
    let Some(router) = agent
        .view
        .router
        .filter(|_| agent.reports_under_routing_agent())
    else {
        return Ok(None);
    };
    Ok((router.summary)()?.map(|summary| summary.agent_name))
}

// ─── disconnect ──────────────────────────────────────────────────────────────

struct Removal {
    view: AgentView,
    routing: bool,
    reporting: bool,
}

/// `nasiko disconnect [agent]`: remove LLM routing (with the MCP gateway) and/or reporting.
pub fn disconnect(target: Option<&str>, options: &DisconnectOptions) -> Result<()> {
    let prompter = prompter(options.yes);
    let views = load_views()?;
    let targets: Vec<AgentView> = match target {
        Some(id) => vec![find_view(&views, id)?.clone()],
        None if prompter.interactive() => {
            let connected: Vec<&AgentView> =
                views.iter().filter(|view| view.is_connected()).collect();
            if connected.is_empty() {
                term::print_info("No coding agents are connected to Nasiko.");
                return Ok(());
            }
            let labels: Vec<String> = connected
                .iter()
                .map(|view| format!("{} ({})", view.local.display_name, connected_parts(view)))
                .collect();
            let chosen = prompter.multi_select(
                "Which coding agents do you want to disconnect? (space selects, enter confirms)",
                &labels,
                &vec![true; labels.len()],
            )?;
            chosen.into_iter().map(|i| connected[i].clone()).collect()
        }
        None => bail!(
            "name the coding agent to disconnect ({})",
            integration::known_agent_ids()
        ),
    };

    let mut removals = Vec::new();
    for view in targets {
        if let Some(removal) = choose_removal(view, options, prompter.as_ref())? {
            removals.push(removal);
        }
    }
    if removals.is_empty() {
        return Ok(());
    }
    if prompter.interactive() {
        println!("\nThis will remove:");
        for removal in &removals {
            println!("  {}", removal.view.local.display_name);
            if removal.routing {
                println!("    • LLM routing and the MCP gateway");
            }
            if removal.reporting {
                println!("    • Session reporting (past sessions stay in observability)");
            }
        }
        if !prompter.confirm("Disconnect?", true)? {
            println!("Cancelled; nothing was changed.");
            return Ok(());
        }
    }

    let mut failures = Vec::new();
    for removal in &removals {
        if let Err(error) = remove(removal, options.force, prompter.as_ref()) {
            failures.push(format!("{}: {error:#}", removal.view.local.display_name));
        }
    }
    if !failures.is_empty() {
        for failure in &failures {
            term::print_error(failure);
        }
        bail!(
            "{} coding agent(s) could not be disconnected",
            failures.len()
        );
    }
    Ok(())
}

fn connected_parts(view: &AgentView) -> String {
    let mut parts = Vec::new();
    if let Some(routing) = &view.routing {
        parts.push(if routing.mcp_installed {
            "LLM routing, MCP"
        } else {
            "LLM routing"
        });
    }
    if view.local.reporting_installed {
        parts.push("reporting");
    }
    parts.join(", ")
}

/// Which parts of one agent to remove: from flags, from the terminal, or today's default
/// (routing for agents that have it, reporting for reporting-only agents).
fn choose_removal(
    view: AgentView,
    options: &DisconnectOptions,
    prompter: &dyn Prompter,
) -> Result<Option<Removal>> {
    let name = view.local.display_name;
    // Routing whose state can't be read is still routing: its own disconnect reports why.
    let has_routing = view.routing.is_some() || view.routing_error.is_some();
    let has_reporting = view.local.reporting_installed;
    if !has_routing && !has_reporting {
        term::print_info(&format!("{name} is not connected to Nasiko."));
        return Ok(None);
    }
    let (routing, reporting) = if options.routing || options.reporting {
        if options.routing && !has_routing {
            term::print_info(&format!("{name}: LLM routing is not connected."));
        }
        if options.reporting && !has_reporting {
            term::print_info(&format!("{name}: session reporting is not set up."));
        }
        (
            options.routing && has_routing,
            options.reporting && has_reporting,
        )
    } else if prompter.interactive() && has_routing && has_reporting {
        let items = vec![
            "LLM routing and MCP gateway".to_string(),
            "Session reporting".to_string(),
        ];
        let chosen = prompter.multi_select(
            &format!("What should be removed from {name}?"),
            &items,
            &[true, true],
        )?;
        (chosen.contains(&0), chosen.contains(&1))
    } else if prompter.interactive() {
        (has_routing, has_reporting)
    } else {
        default_removal(
            view.router.is_some(),
            has_routing,
            has_reporting,
            view.local.id,
        )
    };
    Ok((routing || reporting).then_some(Removal {
        view,
        routing,
        reporting,
    }))
}

/// Without flags or a terminal, `disconnect` keeps its original meaning: remove LLM routing.
/// Reporting-only agents (Cursor) have nothing else to remove, so their reporting goes.
fn default_removal(
    supports_routing: bool,
    has_routing: bool,
    has_reporting: bool,
    id: &str,
) -> (bool, bool) {
    if !supports_routing {
        return (false, has_reporting);
    }
    if !has_routing && has_reporting {
        term::print_info(&format!(
            "LLM routing is not connected. To remove session reporting: {}",
            term::cyan(&format!("{} disconnect {id} --reporting", cli_name()))
        ));
    }
    (has_routing, false)
}

fn remove(removal: &Removal, force: bool, prompter: &dyn Prompter) -> Result<()> {
    let name = removal.view.local.display_name;
    if let (true, Some(router)) = (removal.routing, removal.view.router) {
        let force = allowed_while_running(name, router, force, "Disconnect", prompter)?;
        (router.disconnect)(force)?;
    }
    if removal.reporting {
        integration::uninstall(removal.view.local.id)?;
    }
    Ok(())
}

// ─── status ──────────────────────────────────────────────────────────────────

/// `nasiko status <agent>`: one agent in detail; without an agent, every agent at a glance.
pub fn status(target: Option<&str>) -> Result<()> {
    match target {
        None => print_overview(),
        Some(id) => {
            let views = load_views()?;
            let view = find_view(&views, id)?;
            match view.router {
                Some(router) => (router.status)()?,
                None => {
                    let value = coding_agent_router::status_value;
                    println!(
                        "{} LLM routing:       {}",
                        view.local.display_name,
                        value("not supported")
                    );
                    println!(
                        "{} session reporting: {}",
                        view.local.display_name,
                        value(&view.local.reporting_status)
                    );
                }
            }
            print_reporting_details(&view.local);
            Ok(())
        }
    }
}

/// Where an installed reporting hook lives and where it delivers — what `agents discover`
/// used to show as its CONFIG column, plus the cluster a rebind would move it from.
fn print_reporting_details(local: &LocalAgent) {
    if !local.reporting_installed {
        return;
    }
    println!("Reporting hook:    {}", local.hook_location);
    match &local.reporting_destination {
        Some(destination) => println!(
            "Reporting to:      {} ({}){}",
            destination.cluster_name,
            destination.cluster_url,
            if local.capture_content {
                ""
            } else {
                ", tokens, latency and cost only"
            }
        ),
        None => println!(
            "Reporting to:      no cluster recorded; reinstall with: {} connect {} --no-routing --reporting",
            cli_name(),
            local.id
        ),
    }
}

/// Every catalogued agent: installed or not, and what is connected.
pub fn print_overview() -> Result<()> {
    let views = load_views()?;
    println!("Coding agents:");
    print_agent_table(&views);
    let pending = integration::pending_event_count()?;
    let cli = cli_name();
    if pending > 0 {
        println!(
            "\nQueued session events: {pending} (deliver now: {})",
            term::cyan(&format!("{cli} agents sync"))
        );
    }
    println!(
        "\nConnect: {} · Details: {}",
        term::cyan(&format!("{cli} connect [agent]")),
        term::cyan(&format!("{cli} status <agent>"))
    );
    Ok(())
}

#[derive(Tabled)]
struct AgentRow {
    #[tabled(rename = "AGENT")]
    agent: String,
    #[tabled(rename = "INSTALLED")]
    installed: String,
    #[tabled(rename = "LLM ROUTING")]
    routing: String,
    #[tabled(rename = "MCP")]
    mcp: String,
    #[tabled(rename = "REPORTING")]
    reporting: String,
}

fn agent_row(view: &AgentView) -> AgentRow {
    let (routing, mcp) = match (&view.router, &view.routing) {
        (None, _) => ("not supported".to_string(), "not supported".to_string()),
        (Some(_), None) if view.routing_error.is_some() => {
            ("state unreadable".to_string(), "?".to_string())
        }
        (Some(_), None) => ("off".to_string(), "-".to_string()),
        (Some(_), Some(summary)) => (
            format!("on ({})", summary.cluster),
            if summary.mcp_installed { "on" } else { "off" }.to_string(),
        ),
    };
    let reporting = if view.local.reporting_active {
        if view.local.capture_content {
            "on".to_string()
        } else {
            "on (no content)".to_string()
        }
    } else if view.local.reporting_installed {
        view.local.reporting_status.clone()
    } else {
        "off".to_string()
    };
    AgentRow {
        agent: view.local.id.to_string(),
        installed: if view.local.detected { "yes" } else { "no" }.to_string(),
        routing,
        mcp,
        reporting,
    }
}

/// The agent table, its values colored on a terminal; [`agent_row`] itself stays plain text.
fn print_agent_table(views: &[AgentView]) {
    let value = coding_agent_router::status_value;
    let rows: Vec<AgentRow> = views
        .iter()
        .map(agent_row)
        .map(|row| AgentRow {
            agent: row.agent,
            installed: value(&row.installed),
            routing: value(&row.routing),
            mcp: value(&row.mcp),
            reporting: value(&row.reporting),
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Answers prompts from a script, failing on any question it wasn't given an answer for.
    struct Scripted {
        confirms: RefCell<VecDeque<bool>>,
        selects: RefCell<VecDeque<usize>>,
    }

    impl Scripted {
        fn new(confirms: &[bool], selects: &[usize]) -> Self {
            Self {
                confirms: RefCell::new(confirms.iter().copied().collect()),
                selects: RefCell::new(selects.iter().copied().collect()),
            }
        }
    }

    impl Prompter for Scripted {
        fn interactive(&self) -> bool {
            true
        }
        fn confirm(&self, question: &str, _default: bool) -> Result<bool> {
            self.confirms
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("unexpected confirm: {question}"))
        }
        fn select(&self, question: &str, _items: &[String], _default: usize) -> Result<usize> {
            self.selects
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("unexpected select: {question}"))
        }
        fn multi_select(
            &self,
            question: &str,
            _items: &[String],
            _defaults: &[bool],
        ) -> Result<Vec<usize>> {
            bail!("unexpected multi-select: {question}")
        }
    }

    fn view(id: &'static str, routed: bool, reporting: bool) -> AgentView {
        AgentView {
            local: LocalAgent {
                id,
                display_name: id,
                detected: true,
                reporting_supported: true,
                reporting_agent_name: reporting.then(|| format!("me-{id}")),
                reporting_declined: false,
                reporting_active: reporting,
                reporting_installed: reporting,
                reporting_status: if reporting {
                    "active (v1)"
                } else {
                    "not installed"
                }
                .into(),
                capture_content: true,
                hook_location: format!("~/.{id}"),
                reporting_destination: reporting.then(here),
            },
            router: router(id),
            routing_error: None,
            routing: routed.then(|| RoutingSummary {
                cluster: "prod".into(),
                cluster_url: "https://cp.example".into(),
                principal_id: Some("me".into()),
                agent_id: format!("id-{id}"),
                agent_name: format!("me-{id}"),
                mcp_installed: true,
            }),
        }
    }

    /// The active cluster in these tests.
    fn here() -> ReportingDestination {
        ReportingDestination {
            cluster_name: "prod".into(),
            cluster_url: "https://cp.example".into(),
            principal_id: "me".into(),
        }
    }

    fn plan_for(
        targets: Vec<AgentView>,
        options: &ConnectOptions,
        prompter: &dyn Prompter,
        load_configs: &dyn Fn() -> Result<Vec<Value>>,
    ) -> Result<Plan> {
        build_plan(targets, options, Some(&here()), prompter, load_configs)
    }

    fn configs() -> Result<Vec<Value>> {
        Ok(vec![
            serde_json::json!({"id": "c1", "name": "fast", "provider": "openai", "model": "gpt"}),
            serde_json::json!({"id": "c2", "name": "team", "provider": "anthropic", "model": "sonnet", "is_default": true}),
        ])
    }

    fn no_configs_expected() -> Result<Vec<Value>> {
        bail!("LLM configs must not be fetched here")
    }

    #[test]
    fn unattended_connect_keeps_todays_behaviour() {
        let plan = plan_for(
            vec![view("claude", false, false)],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: false }
        );
        assert_eq!(plan.agents[0].reporting, ReportingStep::Skip);
        assert_eq!(
            plan.choices.mcp, None,
            "each agent keeps its own MCP default"
        );
        assert_eq!(plan.choices.llm_config, None);
    }

    #[test]
    fn unattended_flags_select_each_part() {
        let options = ConnectOptions {
            no_mcp: true,
            reporting: true,
            no_content: true,
            config: Some("team".into()),
            ..ConnectOptions::default()
        };
        let plan = plan_for(
            vec![view("codex", false, false)],
            &options,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.choices.mcp, Some(false));
        assert_eq!(plan.choices.llm_config.as_deref(), Some("team"));
        assert_eq!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                capture_content: false,
                moving_from: None
            }
        );
    }

    #[test]
    fn cursor_is_reporting_only() {
        let plan = plan_for(
            vec![view("cursor", false, false)],
            &ConnectOptions {
                reporting: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Unsupported);
        assert!(matches!(
            plan.agents[0].reporting,
            ReportingStep::Install { .. }
        ));
    }

    #[test]
    fn unattended_keeps_an_existing_connection_unless_asked_to_change_it() {
        let kept = plan_for(
            vec![view("claude", true, false)],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(kept.agents[0].routing, RoutingStep::Keep);

        let changed = plan_for(
            vec![view("claude", true, false)],
            &ConnectOptions {
                config: Some("team".into()),
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            changed.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );
    }

    #[test]
    fn interactive_answers_apply_to_every_selected_agent() {
        // route? yes · config: "fast" · MCP? no · report? yes · content? no
        let prompter = Scripted::new(&[true, false, true, false], &[0]);
        let plan = plan_for(
            vec![view("claude", false, false), view("opencode", false, false)],
            &ConnectOptions::default(),
            &prompter,
            &configs,
        )
        .unwrap();
        assert_eq!(plan.choices.llm_config.as_deref(), Some("c1"));
        assert_eq!(plan.choices.llm_config_label.as_deref(), Some("fast"));
        assert_eq!(plan.choices.mcp, Some(false));
        for agent in &plan.agents {
            assert_eq!(agent.routing, RoutingStep::Connect { reconnect: false });
            assert_eq!(
                agent.reporting,
                ReportingStep::Install {
                    capture_content: false,
                    moving_from: None
                }
            );
        }
    }

    #[test]
    fn interactive_can_skip_routing_from_the_config_list() {
        // route? yes · config: the trailing "don't route" entry · report? no
        let prompter = Scripted::new(&[true, false], &[2]);
        let plan = plan_for(
            vec![view("claude", false, false)],
            &ConnectOptions::default(),
            &prompter,
            &configs,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Skip);
        assert_eq!(plan.agents[0].reporting, ReportingStep::Skip);
    }

    #[test]
    fn interactive_reconnect_of_an_already_connected_agent() {
        // keep or reconnect: reconnect · config: "team" (after "keep current" and "fast") ·
        // MCP? yes · (reporting already on)
        let prompter = Scripted::new(&[true], &[1, 2]);
        let plan = plan_for(
            vec![view("codex", true, true)],
            &ConnectOptions::default(),
            &prompter,
            &configs,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );
        assert_eq!(plan.agents[0].reporting, ReportingStep::Keep);
        assert_eq!(plan.choices.llm_config.as_deref(), Some("c2"));
    }

    #[test]
    fn nothing_to_change_is_detected() {
        let plan = plan_for(
            vec![view("claude", true, true)],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(!plan.agents.iter().any(AgentPlan::changes_anything));
    }

    #[test]
    fn unattended_disconnect_removes_routing_like_before() {
        assert_eq!(default_removal(true, true, true, "claude"), (true, false));
        assert_eq!(default_removal(false, false, true, "cursor"), (false, true));
        assert_eq!(default_removal(true, false, true, "codex"), (false, false));
    }

    #[test]
    fn coding_agent_targets_are_told_apart_from_control_planes() {
        for id in ["claude", "codex", "opencode", "cursor"] {
            assert!(is_coding_agent(id), "{id}");
        }
        assert!(!is_coding_agent("https://cp.example.com"));
        assert!(!is_coding_agent("prod"));
    }

    #[test]
    fn connect_flags_are_detected() {
        assert!(!ConnectOptions::default().is_set());
        assert!(
            ConnectOptions {
                no_mcp: true,
                ..ConnectOptions::default()
            }
            .is_set()
        );
    }

    fn reporting_to(id: &'static str, destination: Option<ReportingDestination>) -> AgentView {
        let mut view = view(id, false, true);
        view.local.reporting_destination = destination;
        view
    }

    fn elsewhere() -> ReportingDestination {
        ReportingDestination {
            cluster_name: "staging".into(),
            ..here()
        }
    }

    #[test]
    fn reporting_bound_to_another_cluster_moves_when_asked() {
        let reporting = ConnectOptions {
            reporting: true,
            no_routing: true,
            ..ConnectOptions::default()
        };
        let moved = plan_for(
            vec![reporting_to("claude", Some(elsewhere()))],
            &reporting,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            moved.agents[0].reporting,
            ReportingStep::Install {
                capture_content: true,
                moving_from: Some("staging".into())
            }
        );

        // Without --reporting, a script leaves existing reporting alone.
        let kept = plan_for(
            vec![reporting_to("claude", Some(elsewhere()))],
            &ConnectOptions {
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(kept.agents[0].reporting, ReportingStep::Keep);

        // On a terminal, it asks.
        let asked = plan_for(
            vec![reporting_to("claude", Some(elsewhere()))],
            &ConnectOptions {
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Scripted::new(&[true], &[]),
            &no_configs_expected,
        )
        .unwrap();
        assert!(matches!(
            asked.agents[0].reporting,
            ReportingStep::Install {
                moving_from: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn reporting_without_a_recorded_cluster_is_reinstalled() {
        let plan = plan_for(
            vec![reporting_to("codex", None)],
            &ConnectOptions {
                reporting: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(matches!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                moving_from: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn content_flags_change_reporting_that_is_already_on() {
        let tokens_only = plan_for(
            vec![view("claude", false, true)],
            &ConnectOptions {
                no_content: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            tokens_only.agents[0].reporting,
            ReportingStep::Install {
                capture_content: false,
                moving_from: None
            }
        );

        let mut without_content = view("claude", false, true);
        without_content.local.capture_content = false;
        let with_content = plan_for(
            vec![without_content.clone()],
            &ConnectOptions {
                content: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            with_content.agents[0].reporting,
            ReportingStep::Install {
                capture_content: true,
                moving_from: None
            }
        );

        // `--reporting` alone never turns prompt and response text back on.
        let kept = plan_for(
            vec![without_content],
            &ConnectOptions {
                reporting: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(kept.agents[0].reporting, ReportingStep::Keep);
    }

    #[test]
    fn a_content_flag_alone_turns_reporting_on() {
        for (options, capture_content) in [
            (
                ConnectOptions {
                    no_content: true,
                    ..ConnectOptions::default()
                },
                false,
            ),
            (
                ConnectOptions {
                    content: true,
                    ..ConnectOptions::default()
                },
                true,
            ),
        ] {
            let plan = plan_for(
                vec![view("claude", false, false)],
                &options,
                &Unattended,
                &no_configs_expected,
            )
            .unwrap();
            assert_eq!(
                plan.agents[0].reporting,
                ReportingStep::Install {
                    capture_content,
                    moving_from: None
                }
            );
        }
    }

    #[test]
    fn config_flag_means_reconnect_on_a_terminal_too() {
        // Only the MCP question is answered; a "keep or reconnect?" select would fail the test.
        let plan = plan_for(
            vec![view("claude", true, true)],
            &ConnectOptions {
                config: Some("team".into()),
                ..ConnectOptions::default()
            },
            &Scripted::new(&[true], &[]),
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );
    }

    #[test]
    fn a_script_refreshes_an_agent_that_updates_in_place() {
        let plan = plan_for(
            vec![view("opencode", true, false)],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Refresh);

        // Flags only about reporting leave its routing alone.
        let reporting_only = plan_for(
            vec![view("opencode", true, false)],
            &ConnectOptions {
                reporting: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(reporting_only.agents[0].routing, RoutingStep::Keep);
    }

    fn plan_with(
        routing: RoutingStep,
        current: Option<RoutingSummary>,
        current_agent_usable: bool,
    ) -> AgentPlan {
        let mut view = view("opencode", true, false);
        view.routing = current;
        let mut plan = AgentPlan::new(view, routing);
        plan.current_agent_usable = current_agent_usable;
        plan
    }

    fn connected(principal_id: Option<&str>, mcp_installed: bool) -> Option<RoutingSummary> {
        Some(RoutingSummary {
            cluster: "prod".into(),
            cluster_url: "https://cp.example/".into(),
            principal_id: principal_id.map(str::to_string),
            agent_id: "id-mine".into(),
            agent_name: "my-agent".into(),
            mcp_installed,
        })
    }

    #[test]
    fn a_refresh_keeps_the_agent_and_the_mcp_setting() {
        // A scripted refresh never has an MCP answer: it keeps what is set.
        let choices = RoutingChoices::default();
        let plan = plan_with(RoutingStep::Refresh, connected(Some("me"), false), true);
        let request = routing_request(&plan, &choices).unwrap();
        assert_eq!(request.agent, Some("id-mine"));
        assert_eq!(request.reused_agent_name, Some("my-agent"));
        assert!(!request.mcp, "a refresh must not add an MCP gateway");
        assert_eq!(request.llm_config, None);
    }

    #[test]
    fn a_reconnect_reuses_only_an_agent_verified_as_the_users() {
        let choices = RoutingChoices {
            mcp: Some(true),
            llm_config: Some("c2".into()),
            ..RoutingChoices::default()
        };
        let reconnect = RoutingStep::Connect { reconnect: true };

        let verified = plan_with(reconnect.clone(), connected(Some("me"), false), true);
        let request = routing_request(&verified, &choices).unwrap();
        assert_eq!(request.agent, Some("id-mine"));
        assert!(request.mcp, "a reconnect applies the MCP answer");
        assert_eq!(request.llm_config, Some("c2"));

        let unverified = plan_with(reconnect.clone(), connected(Some("me"), false), false);
        assert_eq!(routing_request(&unverified, &choices).unwrap().agent, None);

        // --agent always wins.
        let named = RoutingChoices {
            agent: Some("picked".into()),
            ..choices
        };
        let request = routing_request(&verified, &named).unwrap();
        assert_eq!(request.agent, Some("picked"));
        assert_eq!(request.reused_agent_name, None);
    }

    // ── checking the plan against the control plane ──

    fn plan_of(agents: Vec<AgentPlan>, llm_config: Option<&str>) -> Plan {
        Plan {
            agents,
            choices: RoutingChoices {
                llm_config: llm_config.map(str::to_string),
                ..RoutingChoices::default()
            },
            destination: Some(here()),
        }
    }

    fn config_must_not_be_checked(config: &str) -> Result<()> {
        panic!("LLM config '{config}' must not be checked here")
    }

    fn agent_must_not_be_checked(agent: &str, _: &str) -> Result<AgentLookup> {
        panic!("agent '{agent}' must not be checked here")
    }

    fn verify_with(
        plan: &mut Plan,
        look_up_agent: &dyn Fn(&str, &str) -> Result<AgentLookup>,
    ) -> Result<()> {
        verify_plan(
            plan,
            &Lookups {
                config: &|config: &str| match config {
                    "typo" => bail!("no LLM config named 'typo' in your library"),
                    _ => Ok(()),
                },
                look_up_agent,
            },
        )
    }

    /// `owns_agent` answers yes (usable) or no (gone).
    fn verify(plan: &mut Plan, owns_agent: &dyn Fn(&str, &str) -> Result<bool>) -> Result<()> {
        verify_with(plan, &|agent, integration| {
            Ok(if owns_agent(agent, integration)? {
                AgentLookup::Usable
            } else {
                AgentLookup::Gone
            })
        })
    }

    #[test]
    fn a_refused_lookup_stops_unless_the_connection_records_no_user() {
        let reconnect = RoutingStep::Connect { reconnect: true };
        // Made as this user: a 403 is a permission problem, not a deletion.
        let mut mine = plan_of(
            vec![plan_with(
                reconnect.clone(),
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        verify_with(&mut mine, &|_, _| Ok(AgentLookup::Refused)).unwrap();
        let blocked = mine.agents[0].blocked.clone().unwrap();
        assert!(
            blocked.contains("was refused; nothing was changed"),
            "{blocked}"
        );
        assert!(!mine.agents[0].changes_anything());

        // An older connection that doesn't record whose it is: treated as not yours.
        let mut older = plan_of(
            vec![plan_with(reconnect, connected(None, true), false)],
            None,
        );
        verify_with(&mut older, &|_, _| Ok(AgentLookup::Refused)).unwrap();
        assert!(!older.agents[0].current_agent_usable);
        assert!(older.agents[0].note.is_some());
    }

    #[test]
    fn verify_confirms_the_agent_a_reconnect_reuses() {
        let reconnect = RoutingStep::Connect { reconnect: true };
        let mut plan = plan_of(
            vec![plan_with(
                reconnect.clone(),
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        verify(&mut plan, &|agent, integration| {
            assert_eq!((agent, integration), ("id-mine", "opencode"));
            Ok(true)
        })
        .unwrap();
        assert!(plan.agents[0].current_agent_usable);
        assert!(plan.agents[0].note.is_none());

        // Gone: the default agent is used, and the plan says so.
        let mut gone = plan_of(
            vec![plan_with(
                reconnect.clone(),
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        verify(&mut gone, &|_, _| Ok(false)).unwrap();
        assert!(!gone.agents[0].current_agent_usable);
        assert!(gone.agents[0].note.as_deref().unwrap().contains("my-agent"));
        assert_eq!(
            routing_request(&gone.agents[0], &gone.choices)
                .unwrap()
                .agent,
            None
        );

        // An older connection with no recorded user is checked like the user's own.
        let mut older = plan_of(
            vec![plan_with(reconnect.clone(), connected(None, true), false)],
            None,
        );
        verify(&mut older, &|_, _| Ok(true)).unwrap();
        assert!(older.agents[0].current_agent_usable);

        // Another user's connection is never reused, so it is not even looked up.
        let mut theirs = plan_of(
            vec![plan_with(
                reconnect,
                connected(Some("someone-else"), true),
                false,
            )],
            None,
        );
        verify_with(&mut theirs, &agent_must_not_be_checked).unwrap();
        assert!(!theirs.agents[0].current_agent_usable);
    }

    #[test]
    fn verify_stops_before_anything_changes_when_it_cannot_ask() {
        let mut plan = plan_of(
            vec![plan_with(
                RoutingStep::Connect { reconnect: true },
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        verify(&mut plan, &|_, _| bail!("control plane unreachable")).unwrap();
        let blocked = plan.agents[0].blocked.clone().unwrap();
        assert!(blocked.contains("nothing was changed"), "{blocked}");
        assert!(!plan.agents[0].changes_anything());
    }

    #[test]
    fn verify_rejects_an_unknown_llm_config_only_when_routing_connects() {
        let connect = plan_with(RoutingStep::Connect { reconnect: false }, None, false);
        let error = verify_with(
            &mut plan_of(vec![connect], Some("typo")),
            &agent_must_not_be_checked,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("typo"), "{error}");

        let kept = plan_with(RoutingStep::Keep, connected(Some("me"), true), false);
        verify_plan(
            &mut plan_of(vec![kept], Some("typo")),
            &Lookups {
                config: &config_must_not_be_checked,
                look_up_agent: &agent_must_not_be_checked,
            },
        )
        .unwrap();
    }

    #[test]
    fn the_note_about_a_gone_agent_says_what_actually_happens() {
        let gone_note = |id: &'static str, routing: RoutingStep, agent: Option<&str>| {
            let mut planned = plan_with(routing, connected(Some("me"), true), false);
            planned.view.router = router(id);
            planned.reporting = ReportingStep::Install {
                capture_content: true,
                moving_from: None,
            };
            let mut plan = plan_of(vec![planned], None);
            plan.choices.agent = agent.map(str::to_string);
            verify(&mut plan, &|_, _| Ok(false)).unwrap();
            plan.agents[0].note.clone()
        };
        let reconnect = RoutingStep::Connect { reconnect: true };
        let default = gone_note("claude", reconnect.clone(), None).unwrap();
        assert!(default.contains("your default agent is used"), "{default}");
        let named = gone_note("claude", reconnect.clone(), Some("picked")).unwrap();
        assert!(named.contains("can't be reconnected"), "{named}");
        // OpenCode rolls a failed connect back itself: nothing to say about `--agent`.
        assert_eq!(gone_note("opencode", reconnect, Some("picked")), None);
        let kept = gone_note("claude", RoutingStep::Keep, None).unwrap();
        assert!(
            kept.contains("reporting goes under your default agent"),
            "{kept}"
        );
        assert!(kept.contains("LLM routing stays on my-agent"), "{kept}");
    }

    #[test]
    fn no_mcp_needs_routing() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            options: ConnectOptions,
        }
        use clap::Parser as _;
        assert!(Cli::try_parse_from(["connect", "--no-routing", "--no-mcp"]).is_err());
        assert!(Cli::try_parse_from(["connect", "--no-mcp"]).is_ok());
    }

    // ── R1: one agent's lookup never decides for the others ──

    #[test]
    fn a_refused_lookup_only_needed_to_restore_leaves_the_run_going() {
        let mut plan = plan_of(
            vec![plan_with(
                RoutingStep::Connect { reconnect: true },
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        plan.agents[0].view.router = router("claude");
        plan.choices.agent = Some("new-agent".into());
        verify_with(&mut plan, &|_, _| Ok(AgentLookup::Refused)).unwrap();
        assert!(plan.agents[0].blocked.is_none());
        let note = plan.agents[0].note.clone().unwrap();
        assert!(
            note.contains("refused") && note.contains("can't be reconnected"),
            "{note}"
        );
    }

    #[test]
    fn a_refused_lookup_for_reporting_keeps_routing_and_reports_under_the_default_agent() {
        let mut kept = plan_with(RoutingStep::Keep, connected(Some("me"), true), false);
        kept.reporting = ReportingStep::Install {
            capture_content: false,
            moving_from: None,
        };
        let mut plan = plan_of(vec![kept], None);
        verify_with(&mut plan, &|_, _| Ok(AgentLookup::Refused)).unwrap();
        assert!(plan.agents[0].blocked.is_none());
        assert!(!plan.agents[0].reports_under_routing_agent());
        assert!(
            plan.agents[0]
                .note
                .clone()
                .unwrap()
                .contains("default agent")
        );
    }

    #[test]
    fn an_agent_that_updates_in_place_needs_no_lookup_for_another_agent() {
        let mut plan = plan_of(
            vec![plan_with(
                RoutingStep::Connect { reconnect: true },
                connected(Some("me"), true),
                false,
            )],
            None,
        );
        plan.choices.agent = Some("new-agent".into());
        verify_with(&mut plan, &agent_must_not_be_checked).unwrap();
        assert!(plan.agents[0].note.is_none());
    }

    #[test]
    fn one_agents_failed_lookup_leaves_the_other_agents_going() {
        let reconnect = RoutingStep::Connect { reconnect: true };
        let mut failing = plan_with(reconnect.clone(), connected(Some("me"), true), false);
        if let Some(routing) = failing.view.routing.as_mut() {
            routing.agent_id = "id-unreachable".into();
        }
        let fine = plan_with(reconnect, connected(Some("me"), true), false);
        let mut plan = plan_of(vec![failing, fine], None);
        verify_with(&mut plan, &|agent, _| match agent {
            "id-unreachable" => bail!("control plane unreachable"),
            _ => Ok(AgentLookup::Usable),
        })
        .unwrap();
        assert!(plan.agents[0].blocked.is_some());
        assert!(plan.agents[1].blocked.is_none() && plan.agents[1].changes_anything());
    }

    #[test]
    fn a_blocked_agent_is_reported_as_failed_without_being_touched() {
        let mut blocked = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        blocked.blocked = Some("access to its routing agent was refused".into());
        let plan = plan_of(vec![blocked], None);
        let failures = apply_plan(&plan, &ConnectOptions::default(), &Unattended);
        assert_eq!(failures.len(), 1);
        assert!(calls().is_empty(), "nothing may run for a blocked agent");
    }

    // ── R2: a reconnect suggests keeping the current LLM config ──

    #[test]
    fn accepting_the_suggested_config_on_a_reconnect_keeps_the_current_one() {
        // keep or reconnect: reconnect · config: the suggested first choice · MCP? yes
        let plan = plan_for(
            vec![view("codex", true, true)],
            &ConnectOptions::default(),
            &Scripted::new(&[true], &[1, 0]),
            &configs,
        )
        .unwrap();
        assert_eq!(
            plan.choices.llm_config, None,
            "no config is sent: the agent keeps its own"
        );
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );
    }

    #[test]
    fn not_changing_routing_on_a_reconnect_keeps_the_connection() {
        // keep or reconnect: reconnect · config: the last choice, "don't change"
        let plan = plan_for(
            vec![view("codex", true, true)],
            &ConnectOptions::default(),
            &Scripted::new(&[], &[1, 3]),
            &configs,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Keep);
    }

    // ── R7: MCP on a reconnect keeps what is set ──

    #[test]
    fn a_reconnect_keeps_the_current_mcp_setting_unless_told() {
        let choices = RoutingChoices {
            llm_config: Some("c2".into()),
            ..RoutingChoices::default()
        };
        let reconnect = RoutingStep::Connect { reconnect: true };
        let without_mcp = plan_with(reconnect.clone(), connected(Some("me"), false), true);
        assert!(!routing_request(&without_mcp, &choices).unwrap().mcp);
        let with_mcp = plan_with(reconnect, connected(Some("me"), true), true);
        assert!(routing_request(&with_mcp, &choices).unwrap().mcp);
        let fresh = plan_with(RoutingStep::Connect { reconnect: false }, None, false);
        assert!(
            routing_request(&fresh, &choices).unwrap().mcp,
            "new connections get the gateway"
        );
        let told = RoutingChoices {
            mcp: Some(true),
            ..choices
        };
        assert!(routing_request(&without_mcp, &told).unwrap().mcp);
    }

    #[test]
    fn mcp_flag_on_a_connection_without_mcp_reconnects_it() {
        let mut without_mcp = view("claude", true, false);
        if let Some(routing) = without_mcp.routing.as_mut() {
            routing.mcp_installed = false;
        }
        let plan = plan_for(
            vec![without_mcp],
            &ConnectOptions {
                mcp: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );
        // Already on: nothing to change.
        let plan = plan_for(
            vec![view("claude", true, false)],
            &ConnectOptions {
                mcp: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Keep);
    }

    // ── R9: routing made as another account isn't kept silently ──

    #[test]
    fn a_script_keeping_routing_made_as_another_account_says_so() {
        let mut theirs = view("claude", true, false);
        if let Some(routing) = theirs.routing.as_mut() {
            routing.principal_id = Some("someone-else".into());
        }
        let plan = plan_for(
            vec![theirs],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Keep);
        let note = plan.agents[0].note.clone().unwrap();
        assert!(
            note.contains("another account") && note.contains("connect claude"),
            "{note}"
        );
    }

    // ── R10: what reporting does is said, and it follows the routing agent ──

    #[test]
    fn reporting_to_another_account_on_the_same_cluster_is_named_as_such() {
        let mut local = view("claude", false, true).local;
        local.reporting_destination = Some(ReportingDestination {
            principal_id: "someone-else".into(),
            ..here()
        });
        let (from, to) = reporting_elsewhere(&local, Some(&here())).unwrap();
        assert_eq!(from, "another account on prod");
        assert_eq!(to, "your account on prod");
        local.reporting_destination = Some(elsewhere());
        assert_eq!(
            reporting_elsewhere(&local, Some(&here())),
            Some(("staging".to_string(), "prod".to_string()))
        );
        local.reporting_destination = Some(here());
        assert_eq!(reporting_elsewhere(&local, Some(&here())), None);
    }

    #[test]
    fn reporting_under_another_agent_than_kept_routing_moves_when_asked() {
        let mut split = view("claude", true, true);
        split.local.reporting_agent_name = Some("default-agent".into());
        let reporting = ConnectOptions {
            reporting: true,
            ..ConnectOptions::default()
        };
        let plan = plan_for(
            vec![split.clone()],
            &reporting,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(matches!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                moving_from: None,
                ..
            }
        ));
        // A script that doesn't mention reporting leaves it alone.
        let plan = plan_for(
            vec![split],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].reporting, ReportingStep::Keep);
    }

    #[test]
    fn with_no_reporting_the_plan_does_not_promise_reporting_follows_routing() {
        let plan = plan_for(
            vec![view("claude", true, true)],
            &ConnectOptions {
                config: Some("team".into()),
                no_reporting: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(!plan.agents[0].reporting_follows_routing);
        assert_eq!(reporting_line(&plan.agents[0], &plan), "already on");
    }

    #[test]
    fn reporting_isnt_reinstalled_to_join_a_routing_agent_that_is_gone() {
        let mut split = plan_with(RoutingStep::Keep, connected(Some("me"), true), false);
        split.view.local.reporting_active = true;
        split.view.local.reporting_installed = true;
        split.view.local.capture_content = true;
        split.reporting = ReportingStep::Install {
            capture_content: true,
            moving_from: None,
        };
        let mut plan = plan_of(vec![split], None);
        verify(&mut plan, &|_, _| Ok(false)).unwrap();
        assert_eq!(plan.agents[0].reporting, ReportingStep::Keep);
        assert!(plan.agents[0].note.is_some(), "the plan still says why");
    }

    #[test]
    fn a_routing_failure_names_the_reporting_step_it_skipped() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, true);
        let error = format!("{:#}", apply_config(&plan, "bad").unwrap_err());
        assert!(
            error.contains("session reporting was not changed either"),
            "{error}"
        );
        assert!(error.contains("--no-routing --reporting"), "{error}");
    }

    // ── R11: unreadable routing state ──

    #[test]
    fn unreadable_routing_state_stops_only_a_run_that_would_change_routing() {
        let mut unreadable = view("codex", false, false);
        unreadable.routing_error = Some("unsupported Codex routing state version".into());
        let plan = plan_for(
            vec![unreadable.clone()],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(
            plan.agents[0]
                .blocked
                .clone()
                .unwrap()
                .contains("can't be read")
        );
        // Reporting only (the post-login offer, or --no-routing) still works.
        let plan = plan_for(
            vec![unreadable],
            &offer_options(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(plan.agents[0].blocked.is_none());
        assert!(matches!(
            plan.agents[0].reporting,
            ReportingStep::Install { .. }
        ));
    }

    // ── D-1: a script asked for reporting, not for text ──

    #[test]
    fn unattended_reporting_without_a_content_flag_sends_no_text() {
        let plan = plan_for(
            vec![view("claude", false, false)],
            &ConnectOptions {
                reporting: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                capture_content: false,
                moving_from: None
            }
        );
    }

    // ── R3 / R4: what the post-login offer says ──

    #[test]
    fn the_turn_on_hint_is_a_command_that_can_be_pasted() {
        let one = turn_on_reporting_hint(&["claude"]);
        assert_eq!(one, "nasiko connect claude --no-routing --reporting");
        let several = turn_on_reporting_hint(&["claude", "codex"]);
        assert!(several.contains("agent: claude, codex"), "{several}");
    }

    #[test]
    fn agent_commands_are_spelled_where_this_cli_has_them() {
        assert_eq!(agent_command("logs weather"), "nasiko logs weather");
        assert_eq!(maf_hint(), "nasiko maf");
    }

    #[test]
    fn the_offer_tells_about_reporting_that_misses_this_cluster_and_about_declines() {
        let reporting_here = view("claude", false, true);
        let mut reporting_elsewhere_view = view("codex", false, true);
        reporting_elsewhere_view.local.reporting_destination = Some(elsewhere());
        let mut declined = view("cursor", false, false);
        declined.local.reporting_declined = true;
        let not_installed = view("opencode", false, false);
        let lines = reporting_notices(
            &[
                reporting_here,
                reporting_elsewhere_view,
                declined,
                not_installed,
            ],
            Some(&here()),
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains("still reports sessions to staging"),
            "{lines:?}"
        );
        assert!(lines[1].contains("you said no before"), "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| line.contains("--no-routing --reporting"))
        );
    }

    #[test]
    fn reporting_joins_kept_routing_only_when_its_agent_is_verified() {
        let mut kept = plan_with(RoutingStep::Keep, connected(Some("me"), true), false);
        kept.reporting = ReportingStep::Install {
            capture_content: true,
            moving_from: None,
        };
        let mut plan = plan_of(vec![kept], None);
        verify(&mut plan, &|_, _| Ok(false)).unwrap();
        assert!(!plan.agents[0].reports_under_routing_agent());
        verify(&mut plan, &|_, _| Ok(true)).unwrap();
        assert!(plan.agents[0].reports_under_routing_agent());

        // Routing connected in the same run was just resolved by that connect.
        let connected_now = plan_with(RoutingStep::Connect { reconnect: false }, None, false);
        assert!(connected_now.reports_under_routing_agent());
        let skipped = plan_with(RoutingStep::Skip, None, false);
        assert!(!skipped.reports_under_routing_agent());
    }

    // ── flags behave the same for every agent ──

    #[test]
    fn no_mcp_on_a_connected_agent_reconnects_every_agent_alike() {
        let no_mcp = ConnectOptions {
            no_mcp: true,
            ..ConnectOptions::default()
        };
        for id in ["claude", "codex", "opencode"] {
            let plan = plan_for(
                vec![view(id, true, false)],
                &no_mcp,
                &Unattended,
                &no_configs_expected,
            )
            .unwrap();
            assert_eq!(
                plan.agents[0].routing,
                RoutingStep::Connect { reconnect: true },
                "{id}"
            );
            assert_eq!(plan.choices.mcp, Some(false), "{id}");
        }
        // With --reporting too: still a routing change.
        let plan = plan_for(
            vec![view("opencode", true, false)],
            &ConnectOptions {
                reporting: true,
                ..no_mcp.clone()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Connect { reconnect: true }
        );

        // MCP already off: nothing to change.
        let mut without_mcp = view("claude", true, false);
        if let Some(routing) = without_mcp.routing.as_mut() {
            routing.mcp_installed = false;
        }
        let plan = plan_for(
            vec![without_mcp],
            &no_mcp,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].routing, RoutingStep::Keep);
    }

    #[test]
    fn routing_flags_are_refused_for_agents_without_routing() {
        let config = ConnectOptions {
            config: Some("team".into()),
            ..ConnectOptions::default()
        };
        let error = refuse_routing_flags_without_routing(&[view("cursor", false, false)], &config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("session reporting only"), "{error}");
        // Mixed with an agent that routes, they apply to that one.
        assert!(
            refuse_routing_flags_without_routing(
                &[view("cursor", false, false), view("claude", false, false)],
                &config
            )
            .is_ok()
        );
        assert!(
            refuse_routing_flags_without_routing(
                &[view("cursor", false, false)],
                &ConnectOptions {
                    reporting: true,
                    ..ConnectOptions::default()
                }
            )
            .is_ok()
        );
    }

    // ── installed reporting keeps its settings ──

    fn stale(capture_content: bool) -> AgentView {
        let mut view = view("claude", false, true);
        view.local.reporting_active = false;
        view.local.reporting_status = "needs reinstall".into();
        view.local.capture_content = capture_content;
        view
    }

    #[test]
    fn a_stale_hook_is_reinstalled_with_its_saved_content_setting() {
        let reporting = ConnectOptions {
            reporting: true,
            no_routing: true,
            ..ConnectOptions::default()
        };
        let plan = plan_for(
            vec![stale(false)],
            &reporting,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                capture_content: false,
                moving_from: None
            }
        );

        // On a terminal it is asked about, and keeps the setting too.
        let asked = plan_for(
            vec![stale(false)],
            &ConnectOptions {
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Scripted::new(&[true], &[]),
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(asked.agents[0].reporting, plan.agents[0].reporting);

        // Only --content turns text on.
        let with_content = plan_for(
            vec![stale(false)],
            &ConnectOptions {
                content: true,
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(
            with_content.agents[0].reporting,
            ReportingStep::Install {
                capture_content: true,
                moving_from: None
            }
        );
    }

    #[test]
    fn a_script_without_reporting_flags_leaves_a_stale_hook_alone() {
        let plan = plan_for(
            vec![stale(true)],
            &ConnectOptions {
                no_routing: true,
                ..ConnectOptions::default()
            },
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(plan.agents[0].reporting, ReportingStep::Keep);
    }

    #[test]
    fn user_ids_match_whatever_case_the_token_writes_them_in() {
        let now = ReportingDestination {
            principal_id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            ..here()
        };
        let routing = connected(Some("0F8FAD5B-D9CB-469F-A165-70867728950E"), true).unwrap();
        assert!(owned_by(&routing, Some(&now)));
    }

    #[test]
    fn routing_belongs_to_a_login_only_on_the_same_cluster_and_user() {
        let mine = connected(Some("me"), true).unwrap();
        assert!(owned_by(&mine, Some(&here())));
        assert!(!owned_by(&mine, Some(&elsewhere())));
        assert!(!owned_by(&mine, None));
        let theirs = connected(Some("someone-else"), true).unwrap();
        assert!(!owned_by(&theirs, Some(&here())));
    }

    // ── applying a plan, against a fake agent that records what it is asked to do ──

    thread_local! {
        static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn calls() -> Vec<String> {
        CALLS.with(|calls| calls.borrow_mut().drain(..).collect())
    }

    /// Fails for the LLM config "late" (a failure only connecting itself meets, such as a local
    /// write) and for the agent "id-broken".
    fn fake_connect(options: &RoutingOptions<'_>) -> Result<ConfigChange> {
        let agent = options.agent.unwrap_or("default");
        let config = options.llm_config.unwrap_or("-");
        CALLS.with(|calls| {
            calls
                .borrow_mut()
                .push(format!("connect agent={agent} config={config}"))
        });
        match (config, agent) {
            ("late", _) | (_, "id-broken") => bail!("connect failed"),
            ("late-stuck", _) => Err(anyhow::Error::new(ConfigNotRestored("timeout".into()))
                .context("local install failed")),
            _ => Ok(ConfigChange::unchanged()),
        }
    }

    /// Rejects the LLM config "bad", as the control plane would, and can't undo "stuck".
    fn fake_check(options: &RoutingOptions<'_>) -> Result<()> {
        let config = options.llm_config.unwrap_or("-");
        CALLS.with(|calls| calls.borrow_mut().push(format!("check config={config}")));
        match config {
            "bad" => bail!("the router rejects this LLM config"),
            "stuck" => Err(anyhow::Error::new(ConfigNotRestored("timeout".into()))),
            _ => Ok(()),
        }
    }

    fn fake_disconnect(force: bool) -> Result<()> {
        CALLS.with(|calls| calls.borrow_mut().push(format!("disconnect force={force}")));
        Ok(())
    }

    fn fake_router(reconnects_in_place: bool) -> Router {
        Router {
            connect: fake_connect,
            disconnect: fake_disconnect,
            status: || Ok(()),
            summary: || Ok(None),
            processes: &[],
            reconnects_in_place,
            routing_target: String::new,
            mcp_target: "",
            check: (!reconnects_in_place)
                .then_some(fake_check as fn(&RoutingOptions<'_>) -> Result<()>),
        }
    }

    /// An agent whose reporting install always fails (its id is unknown to `integration`),
    /// without touching the filesystem. A reconnect replaces this login's own, verified agent.
    fn fake_plan(in_place: bool, routing: RoutingStep, report: bool) -> AgentPlan {
        let has_routing = routing != RoutingStep::Connect { reconnect: false };
        let mut view = view("fake", has_routing, false);
        view.router = Some(fake_router(in_place));
        let mut plan = AgentPlan::new(view, routing);
        plan.current_agent_usable = has_routing;
        if report {
            plan.reporting = ReportingStep::Install {
                capture_content: true,
                moving_from: None,
            };
        }
        plan
    }

    fn apply(agent: &AgentPlan, options: &ConnectOptions) -> Result<()> {
        apply_agent(
            agent,
            &RoutingChoices::default(),
            Some(&here()),
            options,
            &Unattended,
        )
    }

    fn apply_config(agent: &AgentPlan, llm_config: &str) -> Result<()> {
        let choices = RoutingChoices {
            llm_config: Some(llm_config.into()),
            ..RoutingChoices::default()
        };
        apply_agent(
            agent,
            &choices,
            Some(&here()),
            &ConnectOptions::default(),
            &Unattended,
        )
    }

    #[test]
    fn an_in_place_reconnect_never_disconnects_first() {
        let plan = fake_plan(true, RoutingStep::Connect { reconnect: true }, false);
        apply(&plan, &ConnectOptions::default()).unwrap();
        assert_eq!(calls(), ["connect agent=id-fake config=-"]);
    }

    #[test]
    fn other_reconnects_disconnect_first_and_honour_force() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        apply(&plan, &ConnectOptions::default()).unwrap();
        assert_eq!(
            calls(),
            [
                "check config=-",
                "disconnect force=false",
                "connect agent=id-fake config=-"
            ]
        );
        apply(
            &plan,
            &ConnectOptions {
                force: true,
                ..ConnectOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            calls(),
            [
                "check config=-",
                "disconnect force=true",
                "connect agent=id-fake config=-"
            ]
        );
    }

    #[test]
    fn a_check_that_cannot_undo_its_config_change_says_so() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        let error = format!("{:#}", apply_config(&plan, "stuck").unwrap_err());
        assert_eq!(calls(), ["check config=stuck"], "nothing may be removed");
        assert!(
            error.contains("left the agent's LLM config changed"),
            "{error}"
        );
        assert!(!error.contains("left as it is"), "{error}");
    }

    #[test]
    fn a_restore_says_when_the_tried_config_could_not_be_put_back() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        let error = apply_config(&plan, "late-stuck").unwrap_err().to_string();
        assert_eq!(calls().len(), 4);
        assert!(
            error.contains("keeps the LLM config being tried"),
            "{error}"
        );
    }

    #[test]
    fn a_restore_names_the_tried_agent_when_it_differs_from_the_previous_one() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        let choices = RoutingChoices {
            agent: Some("picked".into()),
            llm_config: Some("late-stuck".into()),
            ..RoutingChoices::default()
        };
        let error = apply_agent(
            &plan,
            &choices,
            Some(&here()),
            &ConnectOptions::default(),
            &Unattended,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(calls().last().unwrap(), "connect agent=id-fake config=-");
        assert!(
            error.contains("agent picked keeps the LLM config"),
            "{error}"
        );
    }

    #[test]
    fn a_reconnect_the_control_plane_rejects_removes_nothing() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        let error = apply_config(&plan, "bad").unwrap_err().to_string();
        assert_eq!(calls(), ["check config=bad"]);
        assert!(error.contains("left as it is"), "{error}");
    }

    #[test]
    fn a_failed_reconnect_restores_the_previous_connection() {
        let plan = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        let error = apply_config(&plan, "late").unwrap_err().to_string();
        assert_eq!(
            calls(),
            [
                "check config=late",
                "disconnect force=false",
                "connect agent=id-fake config=late",
                "connect agent=id-fake config=-",
            ]
        );
        assert!(
            error.contains("previous connection was restored"),
            "{error}"
        );
    }

    #[test]
    fn a_failed_reconnect_says_when_the_previous_one_cannot_come_back() {
        // Its agent wasn't verified as the user's: nothing to reconnect to.
        let mut unverified = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        unverified.current_agent_usable = false;
        let error = apply_config(&unverified, "late").unwrap_err().to_string();
        assert_eq!(
            calls(),
            [
                "check config=late",
                "disconnect force=false",
                "connect agent=default config=late"
            ]
        );
        assert!(error.contains("connect fake` again"), "{error}");

        // Reconnecting the previous one fails too.
        let mut broken = fake_plan(false, RoutingStep::Connect { reconnect: true }, false);
        if let Some(routing) = broken.view.routing.as_mut() {
            routing.agent_id = "id-broken".into();
        }
        let error = apply(&broken, &ConnectOptions::default())
            .unwrap_err()
            .to_string();
        assert_eq!(calls().len(), 4);
        assert!(error.contains("failed too"), "{error}");
    }

    #[test]
    fn a_failed_in_place_reconnect_keeps_the_agents_own_rollback() {
        let plan = fake_plan(true, RoutingStep::Connect { reconnect: true }, false);
        let error = apply_config(&plan, "late").unwrap_err().to_string();
        assert_eq!(calls(), ["connect agent=id-fake config=late"]);
        assert!(error.contains("LLM routing failed"), "{error}");
    }

    #[test]
    fn failed_reporting_rolls_back_only_routing_created_in_the_same_run() {
        let fresh = fake_plan(false, RoutingStep::Connect { reconnect: false }, true);
        assert!(apply(&fresh, &ConnectOptions::default()).is_err());
        assert_eq!(
            calls(),
            ["connect agent=default config=-", "disconnect force=true"]
        );

        let reconnected = fake_plan(false, RoutingStep::Connect { reconnect: true }, true);
        let error = apply(&reconnected, &ConnectOptions::default())
            .unwrap_err()
            .to_string();
        assert_eq!(
            calls(),
            [
                "check config=-",
                "disconnect force=false",
                "connect agent=id-fake config=-"
            ]
        );
        assert!(error.contains("reconnected and kept"), "{error}");

        let kept = fake_plan(false, RoutingStep::Keep, true);
        assert!(apply(&kept, &ConnectOptions::default()).is_err());
        assert!(calls().is_empty());
    }

    // ── offered after logging in ──

    #[test]
    fn without_a_terminal_reporting_is_not_offered() {
        let offer = reporting_offer(
            vec![view("claude", false, false)],
            Some(&here()),
            &Unattended,
        )
        .unwrap();
        assert!(matches!(offer, Offer::NotAsked));
    }

    #[test]
    fn a_no_to_the_offer_turns_nothing_on() {
        let offer = reporting_offer(
            vec![view("claude", false, false)],
            Some(&here()),
            &Scripted::new(&[false], &[]),
        )
        .unwrap();
        assert!(matches!(offer, Offer::Declined));
    }

    #[test]
    fn a_yes_plans_reporting_only_with_the_chosen_content_setting() {
        // report? yes · include text? no
        let offer = reporting_offer(
            vec![view("claude", true, false), view("codex", false, false)],
            Some(&here()),
            &Scripted::new(&[true, false], &[]),
        )
        .unwrap();
        let Offer::Accepted(plan) = offer else {
            panic!("expected a plan");
        };
        assert_eq!(
            plan.agents[0].routing,
            RoutingStep::Keep,
            "routing is left as it is"
        );
        assert_eq!(
            plan.agents[1].routing,
            RoutingStep::Skip,
            "no routing is added"
        );
        for agent in &plan.agents {
            assert_eq!(
                agent.reporting,
                ReportingStep::Install {
                    capture_content: false,
                    moving_from: None
                }
            );
        }
    }

    #[test]
    fn a_yes_reports_under_the_agent_routing_already_uses() {
        let offer = reporting_offer(
            vec![view("claude", true, false)],
            Some(&here()),
            &Scripted::new(&[true, true], &[]),
        )
        .unwrap();
        let Offer::Accepted(mut plan) = offer else {
            panic!("expected a plan");
        };
        verify(&mut plan, &|_, _| Ok(true)).unwrap();
        assert!(plan.agents[0].reports_under_routing_agent());
    }

    #[test]
    fn agent_names_read_naturally() {
        let agents: Vec<AgentPlan> = ["a", "b", "c"]
            .into_iter()
            .map(|id| AgentPlan::new(view(id, false, false), RoutingStep::Skip))
            .collect();
        assert_eq!(names(&agents, &[0]), "a");
        assert_eq!(names(&agents, &[0, 1]), "a and b");
        assert_eq!(names(&agents, &[0, 1, 2]), "a, b and c");
    }
}
