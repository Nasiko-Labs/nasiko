use anyhow::Result;
use clap::{Parser, Subcommand};
use nasiko::commands;
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
  connect    Register a CP by URL
  disconnect Disconnect an integration
  use        Switch active cluster
  clusters   List configured control planes
  auth       Authentication (login/status/logout)

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

\x1b[33mOperate:\x1b[0m
  push       Build + push image to cluster registry (no deploy)
  deploy     Build + push + deploy to active cluster
  upload     Upload source zip/dir and let the server build + deploy
  ps         List running agents
  logs       Stream agent container logs
  stop       Stop agent container
  start      Start a stopped agent
  restart    Restart agent container
  scale      Scale agent container to N replicas
  rm         Terminate + deregister agent
  deployments  Deployment-level ops (list/get/restart)
  secrets    Manage encrypted secrets
  status     Cluster health + metrics
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
    /// Register a CP by URL, or connect a supported coding agent
    #[command(after_help = "Config: ~/.nasiko/config.json")]
    Connect {
        /// Control-plane URL, `claude`, `codex`, or `opencode`
        target: String,
        #[arg(long)]
        name: Option<String>,
        /// Existing routing agent name or UUID (coding-agent targets only)
        #[arg(long)]
        agent: Option<String>,
        /// Nasiko LLM config name or UUID (coding-agent targets only)
        #[arg(long)]
        config: Option<String>,
    },
    /// Disconnect a local integration
    Disconnect {
        target: String,
        /// Disconnect even when the coding agent is still running
        #[arg(long)]
        force: bool,
    },
    /// Switch active control plane
    Use { name: String },
    /// List configured control planes
    Clusters,
    /// Control plane health + metrics
    Status { target: Option<String> },
    /// Authentication commands
    #[command(after_help = "Config: ~/.nasiko/config.json")]
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
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
                agent,
                config,
            } => {
                if matches!(target.as_str(), "claude" | "codex" | "opencode") {
                    if name.is_some() {
                        anyhow::bail!("--name applies only when connecting a control-plane URL");
                    }
                    match target.as_str() {
                        "claude" => commands::claude::connect(agent.as_deref(), config.as_deref()),
                        "codex" => commands::codex::connect(agent.as_deref(), config.as_deref()),
                        "opencode" => {
                            commands::opencode::connect(agent.as_deref(), config.as_deref())
                        }
                        _ => unreachable!(),
                    }
                } else {
                    if agent.is_some() || config.is_some() {
                        anyhow::bail!(
                            "--agent and --config apply only to `nasiko connect claude|codex|opencode`"
                        );
                    }
                    commands::cluster::connect(&target, name.as_deref())
                }
            }
            CpCommands::Disconnect { target, force } => match target.as_str() {
                "claude" => commands::claude::disconnect(force),
                "codex" => commands::codex::disconnect(force),
                "opencode" => commands::opencode::disconnect(force),
                _ => anyhow::bail!(
                    "unknown integration '{target}' (expected: claude, codex, or opencode)"
                ),
            },
            CpCommands::Use { name } => commands::cluster::use_cluster(&name),
            CpCommands::Clusters => commands::cluster::list(),
            CpCommands::Status { target } => match target.as_deref() {
                Some("claude") => commands::claude::status(),
                Some("codex") => commands::codex::status(),
                Some("opencode") => commands::opencode::status(),
                Some(other) => {
                    anyhow::bail!(
                        "unknown status target '{other}' (expected: claude, codex, or opencode)"
                    )
                }
                None => commands::status::status(),
            },
            CpCommands::Auth { command } => match command {
                AuthCommands::Login => commands::auth::login(),
                AuthCommands::Status => commands::auth::status(),
                AuthCommands::Logout => commands::auth::logout(),
                AuthCommands::Whoami => commands::auth::whoami(),
            },
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
            agent,
            config,
            ..
        }) = cli.command
        else {
            panic!("expected connect command");
        };
        assert_eq!(target, "opencode");
        assert_eq!(agent.as_deref(), Some("local-agent"));
        assert_eq!(config.as_deref(), Some("production"));
    }

    #[test]
    fn parses_codex_routing_commands() {
        let connect = Cli::try_parse_from([
            "nasiko", "connect", "codex", "--agent", "agent", "--config", "config",
        ])
        .unwrap();
        assert!(matches!(
            connect.command,
            Commands::Cp(CpCommands::Connect { target, .. }) if target == "codex"
        ));

        let status = Cli::try_parse_from(["nasiko", "status", "codex"]).unwrap();
        assert!(matches!(
            status.command,
            Commands::Cp(CpCommands::Status { target }) if target.as_deref() == Some("codex")
        ));
        let disconnect = Cli::try_parse_from(["nasiko", "disconnect", "codex"]).unwrap();
        assert!(matches!(
            disconnect.command,
            Commands::Cp(CpCommands::Disconnect { target, force }) if target == "codex" && !force
        ));
        let forced = Cli::try_parse_from(["nasiko", "disconnect", "codex", "--force"]).unwrap();
        assert!(matches!(
            forced.command,
            Commands::Cp(CpCommands::Disconnect { target, force }) if target == "codex" && force
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
            Commands::Cp(CpCommands::Status { target }) if target.as_deref() == Some("opencode")
        ));
        let disconnect = Cli::try_parse_from(["nasiko", "disconnect", "opencode"]).unwrap();
        assert!(matches!(
            disconnect.command,
            Commands::Cp(CpCommands::Disconnect { target, force }) if target == "opencode" && !force
        ));
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
}
