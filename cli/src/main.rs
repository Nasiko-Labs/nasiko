use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use nasiko::commands;
use nasiko::commands::local_agents::{self, ConnectOptions, DisconnectOptions};
use nasiko::{
    AgentDevCommands, AgentOpsCommands, IntegrationSubCommands, McpSubCommands, RegistrySubCommands,
};

/// Nasiko CLI — Build, deploy, and manage AI agents.
#[derive(Parser)]
#[command(name = "nasiko", version, about, long_about = None, override_help = HELP_TEXT)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

const HELP_TEXT: &str = "\
\x1b[1mNasiko CLI\x1b[0m — Build, deploy, and manage AI agents

\x1b[33mUsage:\x1b[0m nasiko <COMMAND>

\x1b[33mSetup:\x1b[0m
  up         Start local Nasiko cluster (agent devs)
  down       Stop local Nasiko cluster
  connect    Register a CP by URL, or connect local coding agents
  disconnect Disconnect local coding agents
  use        Switch active cluster
  clusters   List configured control planes
  auth       Authentication (login/status/logout)
  doctor     Check Docker, cluster, login, and registry setup
  completions  Print a shell tab-completion script (bash/zsh/fish/...)
  man        Print the man page (troff format)

\x1b[33mCreate:\x1b[0m
  new        Scaffold a new agent project
  skill      Manage agent skills (tools)
  card       Generate or update AgentCard.json
  validate   Validate agent directory structure

\x1b[33mTest:\x1b[0m
  claude     Run Claude Code through the Nasiko LLM router
  build      Build agent Docker image
  run        Build + run agent locally
  chat       Send a message via A2A protocol (--tui for full-screen)
  sessions   List chat sessions
  create-session  Create a new session on the active cluster
  history    Show message history for a session
  delete-session  Delete a session
  context-strategy  How your chat history is selected (pacms/topk/lastk)
  budget     How much chat history a request carries (low/medium/high)

\x1b[33mOperate:\x1b[0m
  push       Build + push image to cluster registry (no deploy)
  deploy     Build + push + deploy to active cluster
  upload     Upload source zip/dir and let the server build + deploy
  import     Deploy from an OCI/artifact-registry reference (server pulls it)
  ps         List running agents
  logs       Stream agent container logs
  stop       Stop agent container
  start      Start a stopped agent
  restart    Restart agent container
  scale      Scale agent container to N replicas
  rm         Terminate + deregister agent
  deployments  Deployment-level ops (list/get/restart)
  secrets    Manage encrypted secrets
  status     Cluster health + local coding agents (status <agent> for one)
  observe    Observability (stats/traces/trace/finops)
  maf        Multi-agent flow workflows (create/run/inspect)

\x1b[33mAgents:\x1b[0m
  agents     Manage deployed and local coding agents

\x1b[33mIntegrations:\x1b[0m
  github     GitHub integration (status/repos/connect/disconnect/clone)

\x1b[33mRegistry:\x1b[0m
  registry   Connect to and browse the artifact registry

\x1b[33mMCP:\x1b[0m
  mcp        MCP Gateway — connectors, connections, sharing, credentials, oauth, agent-tools

\x1b[33mOptions:\x1b[0m
  -h, --help     Print help
  -V, --version  Print version

Run \x1b[36mnasiko <command> --help\x1b[0m for details on any command.
";

const CONNECT_HELP: &str = "\
Config: ~/.nasiko/config.json

Coding agents get up to three things: LLM routing through the Nasiko LLM router, your MCP
connectors through the Nasiko gateway, and session reporting to Nasiko observability.
On a terminal, connect asks about each; flags answer the same questions in scripts.
Without a terminal, new routing gets MCP and reporting is off unless --reporting, --content
or --no-content is given; scripts report prompt and response text only with --content.

Examples:
  nasiko connect                       pick from the coding agents on this machine
  nasiko connect claude                set up Claude Code, asking about each part
  nasiko connect codex --config team -y
  nasiko connect cursor --reporting    Cursor supports session reporting only
  nasiko connect https://cp.example.com --name prod";

#[derive(Subcommand)]
#[command(subcommand_help_heading = "Commands")]
enum Commands {
    #[command(flatten)]
    Agent(AgentDevCommands),
    #[command(flatten)]
    Ops(AgentOpsCommands),
    #[command(flatten)]
    Cp(CpCommands),
    #[command(flatten)]
    Reg(RegistryCommands),
    #[command(flatten)]
    Mcp(McpCommands),
    #[command(flatten)]
    Integration(IntegrationCommands),
}

#[derive(Subcommand)]
#[command(next_help_heading = "Integrations")]
enum IntegrationCommands {
    /// Detect local coding agents and manage session reporting
    #[command(hide = true)]
    Integration {
        #[command(subcommand)]
        command: IntegrationSubCommands,
    },
    /// Show which coding agents are on this machine (alias for `integration status`)
    #[command(hide = true)]
    Integrations,
}

#[derive(Subcommand)]
#[command(next_help_heading = "Setup")]
enum CpCommands {
    /// Start local Nasiko cluster (pulls CP image from DockerHub)
    #[command(after_help = "Config: ~/.nasiko/.env")]
    Up,
    /// Stop local Nasiko cluster
    Down,
    /// Register a CP by URL, or connect local coding agents
    #[command(after_help = CONNECT_HELP)]
    Connect {
        /// Control-plane URL, or a coding agent: claude, codex, opencode, cursor.
        /// Omit to pick from the coding agents installed on this machine
        target: Option<String>,
        /// Name for a control plane registered by URL
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        coding_agent: ConnectOptions,
    },
    /// Disconnect local coding agents
    #[command(
        after_help = "Without --routing/--reporting and without a terminal, removes LLM routing only."
    )]
    Disconnect {
        /// Coding agent: claude, codex, opencode, cursor. Omit to pick from connected ones
        target: Option<String>,
        #[command(flatten)]
        options: DisconnectOptions,
    },
    /// Switch active control plane
    Use { name: String },
    /// List configured control planes
    Clusters,
    /// Control plane health and local coding agents (exits 1 if the cluster is unhealthy), or one
    /// coding agent in detail
    Status {
        /// Coding agent to show in detail: claude, codex, opencode, cursor
        target: Option<String>,
        /// Output the cluster's status as JSON (without the coding-agent overview)
        #[arg(short = 'j', long, conflicts_with = "target")]
        json: bool,
    },
    /// Authentication commands
    #[command(after_help = "Config: ~/.nasiko/config.json")]
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Manage your PACMS conversation-history budget tier (low/medium/high)
    Budget {
        #[command(subcommand)]
        command: BudgetCommands,
    },
    /// Manage your conversation-history context-selection strategy (pacms/topk/lastk)
    ContextStrategy {
        #[command(subcommand)]
        command: ContextStrategyCommands,
    },
    /// Check Docker, cluster reachability, login, and registry setup, with the fix for each problem
    Doctor,
    /// Print a shell tab-completion script
    #[command(after_help = "Example: source <(nasiko completions zsh)")]
    Completions {
        /// Shell to generate the script for
        shell: Shell,
    },
    /// Print the man page (troff format)
    #[command(after_help = "Example: nasiko man > /usr/local/share/man/man1/nasiko.1")]
    Man,
    /// Internal Claude Code credential helper
    #[command(name = "__claude-token", hide = true)]
    ClaudeToken,
    /// Internal coding-agent credential helper
    #[command(name = "__coding-agent-token", hide = true)]
    CodingAgentToken { agent: String },
}

#[derive(Subcommand)]
#[command(next_help_heading = "Registry")]
enum RegistryCommands {
    /// Connect to and browse the artifact registry
    Registry {
        #[command(subcommand)]
        command: RegistrySubCommands,
    },
}

#[derive(Subcommand)]
#[command(next_help_heading = "MCP")]
enum McpCommands {
    /// Manage MCP Gateway connectors, connections, and agent tool access
    Mcp {
        #[command(subcommand)]
        command: McpSubCommands,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Save API token for active cluster
    Login,
    /// Show current auth status
    Status,
    /// Clear stored token
    Logout,
    /// Print the authenticated user's profile
    Whoami,
}

#[derive(Subcommand)]
enum BudgetCommands {
    /// Show your current PACMS budget tier
    Get,
    /// Set your PACMS budget tier
    Set {
        /// low, medium, or high
        level: String,
    },
}

#[derive(Subcommand)]
enum ContextStrategyCommands {
    /// Show your current context-selection strategy
    Get,
    /// Set your context-selection strategy
    Set {
        /// pacms, topk, or lastk
        strategy: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Agent(cmd) => nasiko::dispatch_agent_dev(cmd),
        Commands::Ops(cmd) => nasiko::dispatch_agent_ops(cmd),
        Commands::Cp(cmd) => match cmd {
            CpCommands::Up => commands::dev::start(false),
            CpCommands::Down => commands::dev::stop(),
            CpCommands::Connect {
                target,
                name,
                coding_agent,
            } => match target.as_deref() {
                Some(url) if !local_agents::is_coding_agent(url) => {
                    if coding_agent.is_set() {
                        anyhow::bail!(
                            "coding-agent flags apply only to `nasiko connect [claude|codex|opencode|cursor]`"
                        );
                    }
                    commands::cluster::connect(url, name.as_deref())
                }
                agent => {
                    if name.is_some() {
                        anyhow::bail!("--name applies only when connecting a control-plane URL");
                    }
                    local_agents::connect(agent, &coding_agent)
                }
            },
            CpCommands::Disconnect { target, options } => {
                local_agents::disconnect(target.as_deref(), &options)
            }
            CpCommands::Use { name } => commands::cluster::use_cluster(&name),
            CpCommands::Clusters => commands::cluster::list(),
            CpCommands::Status { target, json } => match target.as_deref() {
                Some(agent) if local_agents::is_coding_agent(agent) => {
                    local_agents::status(Some(agent))
                }
                Some(other) => anyhow::bail!(
                    "unknown status target '{other}' (expected a coding agent: claude, codex, opencode, cursor)"
                ),
                // JSON is the cluster's status alone: the overview is text and would break it.
                None if json => commands::status::status(true),
                None => {
                    // The control plane may be down or unset; the local agents are still worth
                    // showing — and neither failure may hide the other.
                    let cluster = commands::status::status(false);
                    println!();
                    let overview = local_agents::print_overview();
                    match (cluster, overview) {
                        (Err(cluster), Err(overview)) => {
                            eprintln!("error: could not show the coding agents: {overview:#}");
                            Err(cluster)
                        }
                        (cluster, overview) => cluster.and(overview),
                    }
                }
            },
            CpCommands::Auth { command } => match command {
                AuthCommands::Login => commands::auth::login(),
                AuthCommands::Status => commands::auth::status(),
                AuthCommands::Logout => commands::auth::logout(),
                AuthCommands::Whoami => commands::auth::whoami(),
            },
            CpCommands::Budget { command } => match command {
                BudgetCommands::Get => commands::context_selection::budget_get(),
                BudgetCommands::Set { level } => commands::context_selection::budget_set(&level),
            },
            CpCommands::ContextStrategy { command } => match command {
                ContextStrategyCommands::Get => commands::context_selection::strategy_get(),
                ContextStrategyCommands::Set { strategy } => {
                    commands::context_selection::strategy_set(&strategy)
                }
            },
            CpCommands::Doctor => commands::doctor::doctor(),
            CpCommands::Completions { shell } => {
                write_completions(shell, &mut IgnoreBrokenPipe(std::io::stdout()));
                Ok(())
            }
            // clap_mangen already leaves hidden subcommands and flags out of the page.
            CpCommands::Man => Ok(clap_mangen::Man::new(Cli::command())
                .render(&mut IgnoreBrokenPipe(std::io::stdout()))?),
            CpCommands::ClaudeToken => commands::claude::credential(),
            CpCommands::CodingAgentToken { agent } => match agent.as_str() {
                "claude" => commands::claude::credential(),
                "codex" => commands::codex::credential(),
                "opencode" => commands::opencode::credential(),
                _ => anyhow::bail!(
                    "unknown credential target '{agent}' (expected: claude, codex, or opencode)"
                ),
            },
        },
        Commands::Reg(cmd) => match cmd {
            RegistryCommands::Registry { command } => nasiko::dispatch_registry(command),
        },
        Commands::Mcp(cmd) => match cmd {
            McpCommands::Mcp { command } => nasiko::dispatch_mcp(command),
        },
        Commands::Integration(cmd) => match cmd {
            IntegrationCommands::Integration { command } => nasiko::dispatch_integration(command),
            IntegrationCommands::Integrations => {
                nasiko::dispatch_integration(IntegrationSubCommands::Status)
            }
        },
    }
}

/// Writes `shell`'s tab-completion script for the whole CLI to `out`.
fn write_completions(shell: Shell, out: &mut impl std::io::Write) {
    let mut cmd = without_hidden_subcommands(&Cli::command());
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, out);
}

/// A copy of `cmd` with every hidden subcommand dropped, at every level. clap_complete's
/// generators include hidden subcommands, which would offer internal helpers
/// (`__coding-agent-token`, `agents report`, ...) on TAB. Keeps each command's arguments,
/// help text, and visible aliases — everything a completion script reads.
fn without_hidden_subcommands(cmd: &clap::Command) -> clap::Command {
    let mut stripped = clap::Command::new(cmd.get_name().to_string())
        .visible_aliases(cmd.get_visible_aliases().map(str::to_string))
        .args(cmd.get_arguments().cloned());
    if let Some(about) = cmd.get_about() {
        stripped = stripped.about(about.clone());
    }
    if let Some(version) = cmd.get_version() {
        stripped = stripped.version(version.to_string());
    }
    stripped.subcommands(
        cmd.get_subcommands()
            .filter(|sub| !sub.is_hide_set())
            .map(without_hidden_subcommands),
    )
}

/// Treats a `BrokenPipe` write as success, so a reader that closes the pipe early
/// (`| head`, quitting `less`) ends the command cleanly instead of panicking inside a
/// generator that `.expect()`s every write to succeed.
struct IgnoreBrokenPipe<W>(W);

impl<W: std::io::Write> std::io::Write for IgnoreBrokenPipe<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.write(buf) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(buf.len()),
            other => other,
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.flush() {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencode_connect_options() {
        let cli = Cli::try_parse_from([
            "nasiko",
            "connect",
            "opencode",
            "--agent",
            "local-agent",
            "--config",
            "production",
        ])
        .unwrap();
        let Commands::Cp(CpCommands::Connect {
            target,
            coding_agent,
            ..
        }) = cli.command
        else {
            panic!("expected connect command");
        };
        assert_eq!(target.as_deref(), Some("opencode"));
        assert_eq!(coding_agent.agent.as_deref(), Some("local-agent"));
        assert_eq!(coding_agent.config.as_deref(), Some("production"));
    }

    #[test]
    fn parses_every_connect_question_as_a_flag() {
        let cli = Cli::try_parse_from([
            "nasiko",
            "connect",
            "claude",
            "--no-mcp",
            "--reporting",
            "--no-content",
            "-y",
        ])
        .unwrap();
        let Commands::Cp(CpCommands::Connect { coding_agent, .. }) = cli.command else {
            panic!("expected connect command");
        };
        assert!(coding_agent.no_mcp && coding_agent.reporting && coding_agent.no_content);
        assert!(coding_agent.yes);

        assert!(
            Cli::try_parse_from([
                "nasiko",
                "connect",
                "claude",
                "--reporting",
                "--no-reporting"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "nasiko",
                "connect",
                "claude",
                "--no-routing",
                "--config",
                "x"
            ])
            .is_err()
        );
    }

    #[test]
    fn bare_connect_and_disconnect_pick_from_local_agents() {
        let connect = Cli::try_parse_from(["nasiko", "connect"]).unwrap();
        assert!(matches!(
            connect.command,
            Commands::Cp(CpCommands::Connect { target: None, .. })
        ));
        let disconnect = Cli::try_parse_from(["nasiko", "disconnect"]).unwrap();
        assert!(matches!(
            disconnect.command,
            Commands::Cp(CpCommands::Disconnect { target: None, .. })
        ));
    }

    #[test]
    fn parses_codex_routing_commands() {
        let connect = Cli::try_parse_from([
            "nasiko", "connect", "codex", "--agent", "agent", "--config", "config",
        ])
        .unwrap();
        assert!(matches!(
            connect.command,
            Commands::Cp(CpCommands::Connect { target, .. }) if target.as_deref() == Some("codex")
        ));

        let status = Cli::try_parse_from(["nasiko", "status", "codex"]).unwrap();
        assert!(matches!(
            status.command,
            Commands::Cp(CpCommands::Status { target, .. }) if target.as_deref() == Some("codex")
        ));
        let disconnect = Cli::try_parse_from(["nasiko", "disconnect", "codex"]).unwrap();
        assert!(matches!(
            disconnect.command,
            Commands::Cp(CpCommands::Disconnect { target, options })
                if target.as_deref() == Some("codex") && !options.force
        ));
        let forced = Cli::try_parse_from(["nasiko", "disconnect", "codex", "--force"]).unwrap();
        assert!(matches!(
            forced.command,
            Commands::Cp(CpCommands::Disconnect { target, options })
                if target.as_deref() == Some("codex") && options.force
        ));
        let reporting =
            Cli::try_parse_from(["nasiko", "disconnect", "codex", "--reporting"]).unwrap();
        assert!(matches!(
            reporting.command,
            Commands::Cp(CpCommands::Disconnect { options, .. })
                if options.reporting && !options.routing
        ));
        let helper = Cli::try_parse_from(["nasiko", "__coding-agent-token", "codex"]).unwrap();
        assert!(matches!(
            helper.command,
            Commands::Cp(CpCommands::CodingAgentToken { agent }) if agent == "codex"
        ));
    }

    #[test]
    fn parses_generic_and_legacy_credential_helpers() {
        let generic = Cli::try_parse_from(["nasiko", "__coding-agent-token", "opencode"]).unwrap();
        assert!(matches!(
            generic.command,
            Commands::Cp(CpCommands::CodingAgentToken { agent }) if agent == "opencode"
        ));
        let legacy = Cli::try_parse_from(["nasiko", "__claude-token"]).unwrap();
        assert!(matches!(
            legacy.command,
            Commands::Cp(CpCommands::ClaudeToken)
        ));
    }

    #[test]
    fn parses_opencode_status_and_disconnect() {
        let status = Cli::try_parse_from(["nasiko", "status", "opencode"]).unwrap();
        assert!(matches!(
            status.command,
            Commands::Cp(CpCommands::Status { target, .. }) if target.as_deref() == Some("opencode")
        ));
        let disconnect = Cli::try_parse_from(["nasiko", "disconnect", "opencode"]).unwrap();
        assert!(matches!(
            disconnect.command,
            Commands::Cp(CpCommands::Disconnect { target, options })
                if target.as_deref() == Some("opencode") && !options.force
        ));
    }

    #[test]
    fn parses_cluster_status_json_but_not_with_a_coding_agent_target() {
        let cluster = Cli::try_parse_from(["nasiko", "status", "--json"]).unwrap();
        assert!(matches!(
            cluster.command,
            Commands::Cp(CpCommands::Status {
                target: None,
                json: true
            })
        ));
        let plain = Cli::try_parse_from(["nasiko", "status"]).unwrap();
        assert!(matches!(
            plain.command,
            Commands::Cp(CpCommands::Status {
                target: None,
                json: false
            })
        ));
        assert!(Cli::try_parse_from(["nasiko", "status", "codex", "--json"]).is_err());
    }

    #[test]
    fn parses_rm_by_name_or_uuid_and_the_legacy_name_flag() {
        let rm = |args: &[&str]| {
            let cli = Cli::try_parse_from(args)?;
            let Commands::Ops(AgentOpsCommands::Rm { agent, name, force }) = cli.command else {
                panic!("expected rm");
            };
            Ok::<_, clap::Error>((agent, name, force))
        };
        assert_eq!(
            rm(&["nasiko", "rm", "weather"]).unwrap(),
            (Some("weather".into()), None, false)
        );
        assert_eq!(
            rm(&["nasiko", "rm", "--name", "weather", "-f"]).unwrap(),
            (None, Some("weather".into()), true)
        );
        assert_eq!(
            rm(&["nasiko", "rm", "-n", "weather"]).unwrap(),
            (None, Some("weather".into()), false)
        );
        assert!(rm(&["nasiko", "rm"]).is_err(), "an agent is required");
        assert!(
            rm(&["nasiko", "rm", "weather", "--name", "other"]).is_err(),
            "not both"
        );
    }

    #[test]
    fn parses_deploy_dry_run() {
        let cli = Cli::try_parse_from(["nasiko", "deploy", ".", "--dry-run", "--yes"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Ops(AgentOpsCommands::Deploy {
                dry_run: true,
                yes: true,
                ..
            })
        ));
    }

    #[test]
    fn parses_doctor_and_completions() {
        let doctor = Cli::try_parse_from(["nasiko", "doctor"]).unwrap();
        assert!(matches!(doctor.command, Commands::Cp(CpCommands::Doctor)));

        let completions = Cli::try_parse_from(["nasiko", "completions", "zsh"]).unwrap();
        assert!(matches!(
            completions.command,
            Commands::Cp(CpCommands::Completions { shell: Shell::Zsh })
        ));
        assert!(Cli::try_parse_from(["nasiko", "completions", "not-a-shell"]).is_err());

        let man = Cli::try_parse_from(["nasiko", "man"]).unwrap();
        assert!(matches!(man.command, Commands::Cp(CpCommands::Man)));
    }

    #[test]
    fn man_page_lists_commands_but_not_hidden_ones() {
        let mut page = Vec::new();
        clap_mangen::Man::new(Cli::command())
            .render(&mut page)
            .unwrap();
        let page = String::from_utf8(page).unwrap();
        assert!(page.starts_with(".ie"), "troff header");
        assert!(page.contains("deploy"));
        assert!(!page.contains("__coding\\-agent\\-token"));
        assert!(!page.contains("__coding-agent-token"));
    }

    #[test]
    fn completion_script_omits_hidden_internal_commands() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let mut script = Vec::new();
            write_completions(shell, &mut script);
            let script = String::from_utf8(script).unwrap();
            assert!(script.contains("deploy"), "{shell}");
            assert!(
                script.contains("list-uploaded"),
                "{shell}: nested subcommands kept"
            );
            assert!(!script.contains("__coding-agent-token"), "{shell}");
            assert!(!script.contains("__claude-token"), "{shell}");
        }

        // Hidden subcommands are dropped at every level, not just the top: `agents discover`
        // (hidden, renamed to `status`) is gone while its visible siblings stay. Checked on the
        // tree rather than the scripts, where "discover" also appears in other commands' help.
        let tree = without_hidden_subcommands(&Cli::command());
        let agents = tree
            .get_subcommands()
            .find(|c| c.get_name() == "agents")
            .expect("agents command");
        let names: Vec<&str> = agents.get_subcommands().map(|c| c.get_name()).collect();
        assert!(names.contains(&"list-uploaded"), "{names:?}");
        assert!(!names.contains(&"discover"), "{names:?}");
        assert!(!names.contains(&"report"), "{names:?}");
    }

    #[test]
    fn ignore_broken_pipe_reports_full_write_on_closed_pipe() {
        use std::io::Write;
        struct ClosedPipe;
        impl Write for ClosedPipe {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let mut out = IgnoreBrokenPipe(ClosedPipe);
        assert_eq!(out.write(b"abc").unwrap(), 3);
        assert!(out.flush().is_ok());
    }

    #[test]
    fn parses_coding_agent_management_under_agents() {
        let discover = Cli::try_parse_from(["nasiko", "agents", "discover"]).unwrap();
        assert!(matches!(
            discover.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Discover
            })
        ));

        let install =
            Cli::try_parse_from(["nasiko", "agents", "install", "claude", "--no-content"]).unwrap();
        assert!(matches!(
            install.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Install { agent, no_content }
            }) if agent == "claude" && no_content
        ));

        let report =
            Cli::try_parse_from(["nasiko", "agents", "report", "--agent", "opencode"]).unwrap();
        assert!(matches!(
            report.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Report { agent }
            }) if agent == "opencode"
        ));
    }

    #[test]
    fn parses_coding_agent_setup_and_import_under_agents() {
        let setup = Cli::try_parse_from(["nasiko", "agents", "setup"]).unwrap();
        assert!(matches!(
            setup.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Setup {
                    yes: false,
                    no_import: false,
                    wait: false
                }
            })
        ));

        let consented =
            Cli::try_parse_from(["nasiko", "agents", "setup", "--yes", "--no-import"]).unwrap();
        assert!(matches!(
            consented.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Setup {
                    yes: true,
                    no_import: true,
                    wait: false
                }
            })
        ));

        let import = Cli::try_parse_from(["nasiko", "agents", "import"]).unwrap();
        assert!(matches!(
            import.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Import
            })
        ));
    }

    #[test]
    fn parses_a_foreground_setup_the_import_status_and_the_background_worker() {
        let waiting = Cli::try_parse_from(["nasiko", "agents", "setup", "--wait"]).unwrap();
        assert!(matches!(
            waiting.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Setup { wait: true, .. }
            })
        ));

        let status = Cli::try_parse_from(["nasiko", "agents", "status"]).unwrap();
        assert!(matches!(
            status.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::Status
            })
        ));

        // What `agents setup` runs in the background.
        let worker = Cli::try_parse_from(["nasiko", "agents", "import-worker"]).unwrap();
        assert!(matches!(
            worker.command,
            Commands::Ops(AgentOpsCommands::Agents {
                command: nasiko::AgentsCommands::ImportWorker
            })
        ));
    }
}
