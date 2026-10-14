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
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Args;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, MultiSelect, Select};
use serde_json::Value;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::{Client, unwrap_data};
use crate::commands::coding_agent_router::{self, RoutingOptions, RoutingSummary};
use crate::commands::integration::{self, InstallOptions, LocalAgent};
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
    /// Don't give the agent your MCP connectors through the Nasiko gateway
    #[arg(long, help_heading = HELP_HEADING)]
    pub no_mcp: bool,
    /// Report sessions to Nasiko observability (asked on a terminal, off otherwise)
    #[arg(long, conflicts_with = "no_reporting", help_heading = HELP_HEADING)]
    pub reporting: bool,
    /// Don't report sessions
    #[arg(long, help_heading = HELP_HEADING)]
    pub no_reporting: bool,
    /// Report tokens, latency and cost only — no prompt or response text
    #[arg(long, conflicts_with = "no_reporting", help_heading = HELP_HEADING)]
    pub no_content: bool,
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
            || self.no_mcp
            || self.reporting
            || self.no_reporting
            || self.no_content
            || self.yes
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

/// The name this CLI was started as (`nasiko` or `nasiko-ee`), for the hints it prints.
pub fn cli_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg| {
            Path::new(&arg)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .filter(|name| name == "nasiko" || name == "nasiko-ee")
        .unwrap_or_else(|| "nasiko".to_string())
}

/// Points a caller of a renamed command at its replacement. The old command still runs.
pub fn note_renamed(old: &str, new: &str) {
    let cli = cli_name();
    eprintln!(
        "note: `{cli} {old}` is now `{cli} {new}`; the old name will be removed in a future release."
    );
}

// ─── The agents ──────────────────────────────────────────────────────────────

/// The LLM-routing side of an agent. Cursor has none: it supports session reporting only.
#[derive(Clone, Copy)]
struct Router {
    connect: fn(&RoutingOptions<'_>) -> Result<()>,
    disconnect: fn(bool) -> Result<()>,
    status: fn() -> Result<()>,
    summary: fn() -> Result<Option<RoutingSummary>>,
    /// Process names that must be stopped before routing is removed.
    processes: &'static [&'static str],
    /// What routing writes, and where the MCP gateway is registered — shown before applying.
    routing_target: fn() -> String,
    mcp_target: &'static str,
}

fn router(id: &str) -> Option<Router> {
    match id {
        "claude" => Some(Router {
            connect: claude::connect,
            disconnect: claude::disconnect,
            status: claude::status,
            summary: claude::summary,
            processes: &["claude"],
            routing_target: || {
                format!(
                    "{} (ANTHROPIC_BASE_URL, apiKeyHelper)",
                    display_path(&claude::settings_path())
                )
            },
            mcp_target: "`claude mcp add nasiko` (user scope)",
        }),
        "codex" => Some(Router {
            connect: codex::connect,
            disconnect: codex::disconnect,
            status: codex::status,
            summary: codex::summary,
            processes: &["codex"],
            routing_target: || {
                format!(
                    "{} (nasiko model provider)",
                    display_path(&codex::config_path())
                )
            },
            mcp_target: "[mcp_servers.nasiko] in the same config.toml",
        }),
        "opencode" => Some(Router {
            connect: opencode::connect,
            disconnect: opencode::disconnect,
            status: opencode::status,
            summary: opencode::summary,
            processes: &["opencode"],
            routing_target: || display_path(&opencode::plugin_path()),
            mcp_target: "inside the same plugin",
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
}

impl AgentView {
    fn is_connected(&self) -> bool {
        self.routing.is_some() || self.local.reporting_installed
    }
}

fn load_views() -> Result<Vec<AgentView>> {
    integration::local_agents()?
        .into_iter()
        .map(|local| {
            let router = router(local.id);
            let routing = match router {
                Some(router) => (router.summary)()?,
                None => None,
            };
            Ok(AgentView {
                local,
                router,
                routing,
            })
        })
        .collect()
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
    Connect { reconnect: bool },
}

/// What `connect` will do about one agent's session reporting.
#[derive(Debug, Clone, PartialEq)]
enum ReportingStep {
    Skip,
    Keep,
    Install { capture_content: bool },
}

/// Answers shared by every agent connected in one run.
#[derive(Debug, Clone, Default, PartialEq)]
struct RoutingChoices {
    /// Config name or UUID; `None` keeps each agent's current config (or the user's default).
    llm_config: Option<String>,
    llm_config_label: Option<String>,
    mcp: bool,
}

struct AgentPlan {
    view: AgentView,
    routing: RoutingStep,
    reporting: ReportingStep,
}

impl AgentPlan {
    fn changes_anything(&self) -> bool {
        matches!(self.routing, RoutingStep::Connect { .. })
            || matches!(self.reporting, ReportingStep::Install { .. })
    }
}

struct Plan {
    agents: Vec<AgentPlan>,
    choices: RoutingChoices,
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
    ensure_logged_in(prompter.as_ref())?;

    let plan = build_plan(targets, options, prompter.as_ref(), &fetch_llm_configs)?;
    if !plan.agents.iter().any(AgentPlan::changes_anything) {
        print_nothing_to_do(&plan);
        return Ok(());
    }
    print_plan(&plan);
    if prompter.interactive() && !prompter.confirm("Apply these changes?", true)? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }

    let failures = apply_plan(&plan, options);
    println!();
    print_agent_table(
        &load_views()?
            .into_iter()
            .filter(|view| plan.agents.iter().any(|p| p.view.local.id == view.local.id))
            .collect::<Vec<_>>(),
    );
    if !failures.is_empty() {
        for (name, error) in &failures {
            eprintln!("error: {name}: {error}");
        }
        bail!(
            "{} of {} coding agents could not be connected",
            failures.len(),
            plan.agents.len()
        );
    }
    println!("\nCheck any time with: {} status", cli_name());
    Ok(())
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
        eprintln!("{error:#}");
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
fn build_plan(
    targets: Vec<AgentView>,
    options: &ConnectOptions,
    prompter: &dyn Prompter,
    load_configs: &dyn Fn() -> Result<Vec<Value>>,
) -> Result<Plan> {
    let mut agents: Vec<AgentPlan> = Vec::with_capacity(targets.len());
    for view in targets {
        let routing = existing_routing_step(&view, options, prompter)?;
        agents.push(AgentPlan {
            view,
            routing,
            reporting: ReportingStep::Skip,
        });
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
        mcp: !options.no_mcp,
        ..RoutingChoices::default()
    };
    let routed: Vec<usize> = (0..agents.len())
        .filter(|&i| matches!(agents[i].routing, RoutingStep::Connect { .. }))
        .collect();
    if !routed.is_empty() {
        match choose_llm_config(options, prompter, load_configs)? {
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
    if !routed.is_empty() && !options.no_mcp && prompter.interactive() {
        choices.mcp = prompter.confirm(
            &format!(
                "Give {} your MCP connectors through the Nasiko gateway?",
                names(&agents, &routed)
            ),
            true,
        )?;
    }

    // Reporting: one yes/no for every agent that isn't reporting yet.
    let unreported: Vec<usize> = (0..agents.len())
        .filter(|&i| !agents[i].view.local.reporting_active)
        .collect();
    for agent in agents.iter_mut() {
        if agent.view.local.reporting_active {
            agent.reporting = ReportingStep::Keep;
        }
    }
    if !unreported.is_empty() {
        let report = if options.no_reporting {
            false
        } else if options.reporting {
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
            let capture_content = if options.no_content {
                false
            } else if prompter.interactive() {
                prompter.confirm(
                    "Include prompt and response text? (No sends only tokens, latency and cost)",
                    true,
                )?
            } else {
                true
            };
            for &i in &unreported {
                agents[i].reporting = ReportingStep::Install { capture_content };
            }
        }
    }
    Ok(Plan { agents, choices })
}

/// The routing step before the shared questions: unsupported, keep what's there, or connect.
fn existing_routing_step(
    view: &AgentView,
    options: &ConnectOptions,
    prompter: &dyn Prompter,
) -> Result<RoutingStep> {
    if view.router.is_none() {
        return Ok(RoutingStep::Unsupported);
    }
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
    if !prompter.interactive() {
        // Scripts only replace a connection they explicitly asked to change.
        let asked_to_change = options.config.is_some() || options.agent.is_some();
        return Ok(if asked_to_change {
            RoutingStep::Connect { reconnect: true }
        } else {
            RoutingStep::Keep
        });
    }
    let items = vec![
        format!(
            "Keep it (agent {} on {})",
            current.agent_name, current.cluster
        ),
        "Reconnect with new settings".to_string(),
    ];
    let choice = prompter.select(
        &format!(
            "{} LLM routing is already connected.",
            view.local.display_name
        ),
        &items,
        0,
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
fn choose_llm_config(
    options: &ConnectOptions,
    prompter: &dyn Prompter,
    load_configs: &dyn Fn() -> Result<Vec<Value>>,
) -> Result<Option<ConfigChoice>> {
    if let Some(config) = &options.config {
        return Ok(Some(ConfigChoice {
            reference: Some(config.clone()),
            label: Some(config.clone()),
        }));
    }
    if !prompter.interactive() {
        return Ok(Some(ConfigChoice {
            reference: None,
            label: None,
        }));
    }
    let configs = load_configs()?;
    if configs.is_empty() {
        eprintln!(
            "You have no LLM configs yet. Create one with `{} llm-config create --name <name> --provider <provider> --model <model> --default`.",
            cli_name()
        );
        if prompter.confirm("Continue without LLM routing?", true)? {
            return Ok(None);
        }
        bail!("an LLM config is required for LLM routing");
    }
    let mut items: Vec<String> = configs.iter().map(config_label).collect();
    items.push("Don't route LLM calls through Nasiko".to_string());
    let default = configs
        .iter()
        .position(|config| config.get("is_default").and_then(Value::as_bool) == Some(true))
        .unwrap_or(0);
    let choice = prompter.select("LLM config to route through:", &items, default)?;
    let Some(config) = configs.get(choice) else {
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
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn print_plan(plan: &Plan) {
    println!("\nPlan:");
    for agent in &plan.agents {
        println!("  {}", agent.view.local.display_name);
        match (&agent.routing, agent.view.router) {
            (RoutingStep::Connect { reconnect }, Some(router)) => {
                let config = plan
                    .choices
                    .llm_config_label
                    .as_deref()
                    .map(|label| format!(" via LLM config {label}"))
                    .unwrap_or_default();
                let verb = if *reconnect { "reconnect" } else { "connect" };
                println!(
                    "    • LLM routing: {verb}{config} → {}",
                    (router.routing_target)()
                );
                if plan.choices.mcp {
                    println!("    • MCP gateway: {}", router.mcp_target);
                } else {
                    println!("    • MCP gateway: not added");
                }
            }
            (RoutingStep::Keep, _) => println!("    • LLM routing: keep the current connection"),
            (RoutingStep::Skip, _) => println!("    • LLM routing: not set up"),
            (RoutingStep::Unsupported, _) | (RoutingStep::Connect { .. }, None) => {
                println!("    • LLM routing: not supported")
            }
        }
        match &agent.reporting {
            ReportingStep::Install { capture_content } => println!(
                "    • Session reporting: hook in {} ({})",
                agent.view.local.hook_location,
                if *capture_content {
                    "with prompt and response text"
                } else {
                    "tokens, latency and cost only"
                }
            ),
            ReportingStep::Keep => println!("    • Session reporting: already on"),
            ReportingStep::Skip => println!("    • Session reporting: not set up"),
        }
    }
    println!();
}

fn print_nothing_to_do(plan: &Plan) {
    println!("Nothing to change.");
    let cli = cli_name();
    for agent in &plan.agents {
        let id = agent.view.local.id;
        if agent.routing == RoutingStep::Unsupported && agent.reporting == ReportingStep::Skip {
            println!(
                "{} supports session reporting only: {cli} connect {id} --reporting",
                agent.view.local.display_name
            );
        } else if agent.routing == RoutingStep::Keep {
            println!(
                "{} is already connected. To change it: {cli} connect {id} (on a terminal) or {cli} disconnect {id} first.",
                agent.view.local.display_name
            );
        }
    }
}

/// Applies each agent's plan in turn. One agent's failure never stops the others, and an agent
/// is never left half-connected: if its reporting can't be set up, its new routing is removed.
fn apply_plan(plan: &Plan, options: &ConnectOptions) -> Vec<(&'static str, String)> {
    let mut failures = Vec::new();
    for agent in &plan.agents {
        if !agent.changes_anything() {
            continue;
        }
        let name = agent.view.local.display_name;
        println!("\n── {name} ──");
        if let Err(error) = apply_agent(agent, &plan.choices, options) {
            failures.push((name, format!("{error:#}")));
        }
    }
    failures
}

fn apply_agent(
    agent: &AgentPlan,
    choices: &RoutingChoices,
    options: &ConnectOptions,
) -> Result<()> {
    let mut newly_routed = false;
    if let (RoutingStep::Connect { reconnect }, Some(router)) = (&agent.routing, agent.view.router)
    {
        if *reconnect {
            (router.disconnect)(false).context("could not remove the current LLM routing")?;
        }
        let routing = RoutingOptions {
            agent: options.agent.as_deref(),
            llm_config: choices.llm_config.as_deref(),
            mcp: choices.mcp,
        };
        (router.connect)(&routing).map_err(|error| {
            if *reconnect {
                error.context("LLM routing failed after the previous connection was removed; run connect again")
            } else {
                error.context("LLM routing failed")
            }
        })?;
        newly_routed = true;
    }
    if let ReportingStep::Install { capture_content } = agent.reporting {
        let result = reporting_agent_name(agent.view.router).and_then(|agent_name| {
            integration::install(InstallOptions {
                agent_id: agent.view.local.id,
                no_content: !capture_content,
                agent_name: agent_name.as_deref(),
            })
        });
        if let Err(error) = result {
            if let (true, Some(router)) = (newly_routed, agent.view.router) {
                eprintln!(
                    "Session reporting failed; removing the LLM routing set up a moment ago."
                );
                if let Err(rollback) = (router.disconnect)(true) {
                    eprintln!("warning: could not remove LLM routing: {rollback:#}");
                }
            }
            return Err(error.context("session reporting failed"));
        }
    }
    Ok(())
}

/// The agent reporting should use: the routing agent when routing is connected to the active
/// cluster, so one coding agent shows up as one agent; otherwise the account's default.
fn reporting_agent_name(router: Option<Router>) -> Result<Option<String>> {
    let Some(summary) = router
        .map(|router| (router.summary)())
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    let (cluster, entry) = crate::config::active_cluster()?;
    let same_cluster = summary.cluster == cluster
        && summary.cluster_url.trim_end_matches('/') == entry.url.trim_end_matches('/');
    Ok(same_cluster.then_some(summary.agent_name))
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
                println!("No coding agents are connected to Nasiko.");
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
            eprintln!("error: {failure}");
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
    let has_routing = view.routing.is_some();
    let has_reporting = view.local.reporting_installed;
    if !has_routing && !has_reporting {
        println!("{name} is not connected to Nasiko.");
        return Ok(None);
    }
    let (routing, reporting) = if options.routing || options.reporting {
        if options.routing && !has_routing {
            println!("{name}: LLM routing is not connected.");
        }
        if options.reporting && !has_reporting {
            println!("{name}: session reporting is not set up.");
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
        println!(
            "LLM routing is not connected. To remove session reporting: {} disconnect {id} --reporting",
            cli_name()
        );
    }
    (has_routing, false)
}

fn remove(removal: &Removal, force: bool, prompter: &dyn Prompter) -> Result<()> {
    let name = removal.view.local.display_name;
    if let (true, Some(router)) = (removal.routing, removal.view.router) {
        let mut force = force;
        if !force
            && prompter.interactive()
            && coding_agent_router::disconnect_preflight(name, router.processes, false).is_err()
        {
            force = prompter.confirm(
                &format!(
                    "{name} is still running; open sessions may fail API requests until restarted. Disconnect anyway?"
                ),
                false,
            )?;
            if !force {
                bail!("left connected; close {name} and run disconnect again");
            }
        }
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
                Some(router) => (router.status)(),
                None => {
                    println!(
                        "{} LLM routing:       not supported",
                        view.local.display_name
                    );
                    println!(
                        "{} session reporting: {}",
                        view.local.display_name, view.local.reporting_status
                    );
                    Ok(())
                }
            }
        }
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
        println!("\nQueued session events: {pending} (deliver now: {cli} agents sync)");
    }
    println!("\nConnect: {cli} connect [agent] · Details: {cli} status <agent>");
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

fn print_agent_table(views: &[AgentView]) {
    let rows: Vec<AgentRow> = views.iter().map(agent_row).collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
}

/// Shortens a path under `$HOME` to `~/…` for display.
fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
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
            },
            router: router(id),
            routing: routed.then(|| RoutingSummary {
                cluster: "prod".into(),
                cluster_url: "https://cp.example".into(),
                agent_name: format!("me-{id}"),
                mcp_installed: true,
            }),
        }
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
        let plan = build_plan(
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
        assert!(plan.choices.mcp);
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
        let plan = build_plan(
            vec![view("codex", false, false)],
            &options,
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert!(!plan.choices.mcp);
        assert_eq!(plan.choices.llm_config.as_deref(), Some("team"));
        assert_eq!(
            plan.agents[0].reporting,
            ReportingStep::Install {
                capture_content: false
            }
        );
    }

    #[test]
    fn cursor_is_reporting_only() {
        let plan = build_plan(
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
        let kept = build_plan(
            vec![view("claude", true, false)],
            &ConnectOptions::default(),
            &Unattended,
            &no_configs_expected,
        )
        .unwrap();
        assert_eq!(kept.agents[0].routing, RoutingStep::Keep);

        let changed = build_plan(
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
        let plan = build_plan(
            vec![view("claude", false, false), view("opencode", false, false)],
            &ConnectOptions::default(),
            &prompter,
            &configs,
        )
        .unwrap();
        assert_eq!(plan.choices.llm_config.as_deref(), Some("c1"));
        assert_eq!(plan.choices.llm_config_label.as_deref(), Some("fast"));
        assert!(!plan.choices.mcp);
        for agent in &plan.agents {
            assert_eq!(agent.routing, RoutingStep::Connect { reconnect: false });
            assert_eq!(
                agent.reporting,
                ReportingStep::Install {
                    capture_content: false
                }
            );
        }
    }

    #[test]
    fn interactive_can_skip_routing_from_the_config_list() {
        // route? yes · config: the trailing "don't route" entry · report? no
        let prompter = Scripted::new(&[true, false], &[2]);
        let plan = build_plan(
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
        // keep or reconnect: reconnect · config: default "team" · MCP? yes · (reporting already on)
        let prompter = Scripted::new(&[true], &[1, 1]);
        let plan = build_plan(
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
        let plan = build_plan(
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

    #[test]
    fn agent_names_read_naturally() {
        let agents: Vec<AgentPlan> = ["a", "b", "c"]
            .into_iter()
            .map(|id| AgentPlan {
                view: view(id, false, false),
                routing: RoutingStep::Skip,
                reporting: ReportingStep::Skip,
            })
            .collect();
        assert_eq!(names(&agents, &[0]), "a");
        assert_eq!(names(&agents, &[0, 1]), "a and b");
        assert_eq!(names(&agents, &[0, 1, 2]), "a, b and c");
    }
}
