use anyhow::Result;
use clap::{Parser, Subcommand};
use nasiko::commands;
use nasiko::{AgentDevCommands, AgentOpsCommands, McpSubCommands, RegistrySubCommands};

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
  use        Switch active cluster
  clusters   List configured control planes
  auth       Authentication (login/status/logout)

\x1b[33mCreate:\x1b[0m
  new        Scaffold a new agent project
  skill      Manage agent skills (tools)
  card       Generate or update AgentCard.json
  validate   Validate agent directory structure

\x1b[33mTest:\x1b[0m
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

\x1b[33mAgents:\x1b[0m
  agents     Manage and browse agents (ls/get/deploy/search/info/frameworks/list-uploaded/chat)

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
}

#[derive(Subcommand)]
#[command(next_help_heading = "Setup")]
enum CpCommands {
    /// Start local Nasiko cluster (pulls CP image from DockerHub)
    #[command(after_help = "Config: ~/.nasiko/.env")]
    Up,
    /// Stop local Nasiko cluster
    Down,
    /// Register a CP by URL
    #[command(after_help = "Config: ~/.nasiko/config.json")]
    Connect {
        /// Control plane URL
        url: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Switch active control plane
    Use { name: String },
    /// List configured control planes
    Clusters,
    /// Control plane health + metrics
    Status,
    /// Authentication commands
    #[command(after_help = "Config: ~/.nasiko/config.json")]
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
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
            CpCommands::Connect { url, name } => commands::cluster::connect(&url, name.as_deref()),
            CpCommands::Use { name } => commands::cluster::use_cluster(&name),
            CpCommands::Clusters => commands::cluster::list(),
            CpCommands::Status => commands::status::status(),
            CpCommands::Auth { command } => match command {
                AuthCommands::Login => commands::auth::login(),
                AuthCommands::Status => commands::auth::status(),
                AuthCommands::Logout => commands::auth::logout(),
                AuthCommands::Whoami => commands::auth::whoami(),
            },
        },
        Commands::Reg(cmd) => match cmd {
            RegistryCommands::Registry { command } => nasiko::dispatch_registry(command),
        },
        Commands::Mcp(cmd) => match cmd {
            McpCommands::Mcp { command } => nasiko::dispatch_mcp(command),
        },
    }
}
