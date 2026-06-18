mod api;
mod commands;
mod config;
mod oci;
mod skill;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
  dev        Start local platform (infra + CP)
  connect    Register a CP by URL
  use        Switch active cluster
  cps        List configured control planes
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

\x1b[33mOperate:\x1b[0m
  push       Build + push image to cluster registry (no deploy)
  deploy     Build + push + deploy to active cluster
  ps         List running agents
  logs       Stream agent container logs
  stop       Stop agent container
  restart    Restart agent container
  rm         Terminate + deregister agent
  secrets    Manage encrypted secrets
  status     Cluster health + metrics

\x1b[33mRegistry:\x1b[0m
  registry   Connect to and browse the artifact registry
  publish    Publish to the artifact registry

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
}

#[derive(Subcommand)]
#[command(next_help_heading = "Create")]
enum AgentDevCommands {
    /// Scaffold a new agent project
    New {
        /// Template name (e.g., openai, claude-sdk). Omit for interactive mode.
        template: Option<String>,
        /// Project directory name
        name: Option<String>,
    },
    /// Build agent Docker image
    Build {
        /// Agent directory
        #[arg(default_value = ".")]
        directory: String,
        /// Override image tag (default: name:version from AgentCard.json)
        #[arg(long)]
        tag: Option<String>,
        /// Target platform (e.g., linux/amd64)
        #[arg(long)]
        platform: Option<String>,
    },
    /// Build + run agent locally
    Run {
        #[arg(default_value = ".")]
        path: String,
        /// Override port
        #[arg(long, default_value = "5000")]
        port: u16,
    },
    /// Validate agent directory structure
    Validate {
        /// Agent directory
        #[arg(default_value = ".")]
        directory: String,
    },
    /// Generate or update AgentCard.json
    Card {
        /// Agent directory
        #[arg(default_value = ".")]
        directory: String,
    },
    /// Manage agent skills (tools)
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
}

#[derive(Subcommand)]
#[command(next_help_heading = "Operate")]
enum AgentOpsCommands {
    /// Build + push + deploy to active cluster
    Deploy {
        /// Local Docker image or agent directory
        image: String,
        /// Agent name (defaults to image name before ':')
        #[arg(long)]
        name: Option<String>,
        /// Container port
        #[arg(long, default_value = "5000")]
        port: u16,
        /// Path to .env file with KEY=VALUE pairs
        #[arg(long)]
        env_file: Option<String>,
        /// Environment variable override (can be repeated: -e KEY=VALUE)
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
    },
    /// Push image to cluster OCI registry (without deploying)
    Push {
        /// Local Docker image or agent directory
        image: String,
        /// Agent name (defaults to image name)
        #[arg(long)]
        name: Option<String>,
    },
    /// List running agents
    Ps {
        /// Output as JSON
        #[arg(short = 'j', long)]
        json: bool,
    },
    /// Stream agent container logs
    Logs {
        agent: String,
        #[arg(short = 'n', long, default_value = "50")]
        tail: u32,
    },
    /// Stop agent container
    Stop { agent: String },
    /// Restart agent container
    Restart { agent: String },
    /// Terminate + deregister agent
    Rm {
        agent: String,
        #[arg(short, long)]
        force: bool,
    },
    /// Send a message via A2A protocol
    Chat {
        /// A2A endpoint URL
        url: String,
        /// Message (omit for interactive mode)
        message: Option<String>,
        /// Launch full-screen TUI (ratatui)
        #[arg(long)]
        tui: bool,
        /// Resume a previous session by ID
        #[arg(long)]
        resume: Option<String>,
    },
    /// List chat sessions
    Sessions {
        /// A2A endpoint URL (uses active cluster if omitted)
        #[arg(long)]
        endpoint: Option<String>,
    },
}

#[derive(Subcommand)]
#[command(next_help_heading = "Setup")]
enum CpCommands {
    /// Start local platform (infra + CP)
    Dev {
        #[command(subcommand)]
        command: Option<DevCommands>,
    },
    /// Register a CP by URL
    Connect {
        /// Control plane URL
        url: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Switch active control plane
    Use { name: String },
    /// List configured control planes
    Cps,
    /// Control plane health + metrics
    Status,
    /// Authentication commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Manage encrypted secrets
    Secrets {
        #[command(subcommand)]
        command: SecretsCommands,
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
    /// Publish to the artifact registry
    Publish {
        /// Directory containing AgentCard.json or skill.json
        #[arg(default_value = ".")]
        directory: String,
        /// Owner/namespace in the registry
        #[arg(long)]
        owner: Option<String>,
    },
}

#[derive(Subcommand)]
enum RegistrySubCommands {
    /// Connect to an artifact registry
    Connect { url: String },
    /// Disconnect from the artifact registry
    Disconnect,
    /// Show connected registry
    Status,
    /// Search the registry
    Search {
        /// Search query
        query: Option<String>,
        /// Filter by type (agent, skill)
        #[arg(short = 't', long, name = "type")]
        artifact_type: Option<String>,
        /// Filter by framework
        #[arg(short = 'f', long)]
        framework: Option<String>,
        /// Output as JSON
        #[arg(short = 'j', long)]
        json: bool,
    },
    /// List all artifacts in the registry
    List {
        /// Filter by type (agent, skill)
        #[arg(short = 't', long, name = "type")]
        artifact_type: Option<String>,
        /// Output as JSON
        #[arg(short = 'j', long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DevCommands {
    /// Stop local platform containers
    Stop,
    /// Start infrastructure only (Postgres, Redis, RustFS), skip CP binary
    Infra,
    /// Generate or show the dev.env configuration file
    Env,
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Save API token for active cluster
    Login,
    /// Show current auth status
    Status,
    /// Clear stored token
    Logout,
}

#[derive(Subcommand)]
enum SecretsCommands {
    Set { key: String, value: String },
    Get { key: String },
    Ls,
    Rm { key: String },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Add a skill to the current agent project
    Add {
        /// Skill name (e.g., web-search)
        name: String,
        /// Agent project directory
        #[arg(long, default_value = ".")]
        dir: String,
    },
    /// Remove a skill from the current project
    Remove {
        /// Skill name
        name: String,
        /// Agent project directory
        #[arg(long, default_value = ".")]
        dir: String,
    },
    /// List skills in current project
    List {
        /// Agent project directory
        #[arg(long, default_value = ".")]
        dir: String,
    },
    /// Search available skills
    Search {
        /// Search query
        query: Option<String>,
        /// Filter by framework
        #[arg(long)]
        framework: Option<String>,
    },
    /// Show skill details
    Info {
        /// Skill name
        name: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Agent(cmd) => match cmd {
            AgentDevCommands::New { template, name } => match template {
                Some(t) => commands::scaffold::new_agent(&t, name.as_deref().unwrap_or(&t)),
                None => commands::scaffold::new_agent_interactive(name.as_deref()),
            },
            AgentDevCommands::Build { directory, tag, platform } => {
                commands::build::build(&directory, tag.as_deref(), platform.as_deref())
            }
            AgentDevCommands::Run { path, port } => commands::dev::run(&path, port),
            AgentDevCommands::Validate { directory } => commands::validate::validate(&directory),
            AgentDevCommands::Card { directory } => commands::card::card(&directory),
            AgentDevCommands::Skill { command } => match command {
                SkillCommands::Add { name, dir } => commands::skill::add(&name, &dir),
                SkillCommands::Remove { name, dir } => commands::skill::remove(&name, &dir),
                SkillCommands::List { dir } => commands::skill::list(&dir),
                SkillCommands::Search { query, framework } => {
                    commands::skill::search(query.as_deref(), framework.as_deref())
                }
                SkillCommands::Info { name } => commands::skill::info(&name),
            },
        },
        Commands::Ops(cmd) => match cmd {
            AgentOpsCommands::Deploy { image, name, port, env_file, env } => {
                commands::deploy::deploy(&image, name.as_deref(), port, env_file.as_deref(), &env)
            }
            AgentOpsCommands::Push { image, name } => {
                commands::push::push(&image, name.as_deref())
            }
            AgentOpsCommands::Ps { json } => commands::lifecycle::ps(json),
            AgentOpsCommands::Logs { agent, tail } => commands::lifecycle::logs(&agent, tail),
            AgentOpsCommands::Stop { agent } => commands::lifecycle::stop(&agent),
            AgentOpsCommands::Restart { agent } => commands::lifecycle::restart(&agent),
            AgentOpsCommands::Rm { agent, force } => commands::lifecycle::rm(&agent, force),
            AgentOpsCommands::Chat { url, message, tui, resume } => {
                if tui || resume.is_some() {
                    commands::tui::run_tui(&url, resume.as_deref())
                } else {
                    commands::chat::chat(&url, message.as_deref())
                }
            }
            AgentOpsCommands::Sessions { endpoint } => {
                commands::tui::list_sessions(endpoint.as_deref())
            }
        },
        Commands::Cp(cmd) => match cmd {
            CpCommands::Dev { command } => match command {
                None => commands::dev::start(false),
                Some(DevCommands::Infra) => commands::dev::start(true),
                Some(DevCommands::Stop) => commands::dev::stop(),
                Some(DevCommands::Env) => commands::dev::env_template(),
            },
            CpCommands::Connect { url, name } => commands::cluster::connect(&url, name.as_deref()),
            CpCommands::Use { name } => commands::cluster::use_cluster(&name),
            CpCommands::Cps => commands::cluster::list(),
            CpCommands::Status => commands::status::status(),
            CpCommands::Auth { command } => match command {
                AuthCommands::Login => commands::auth::login(),
                AuthCommands::Status => commands::auth::status(),
                AuthCommands::Logout => commands::auth::logout(),
            },
            CpCommands::Secrets { command } => match command {
                SecretsCommands::Set { key, value } => commands::secrets::set(&key, &value),
                SecretsCommands::Get { key } => commands::secrets::get(&key),
                SecretsCommands::Ls => commands::secrets::ls(),
                SecretsCommands::Rm { key } => commands::secrets::rm(&key),
            },
        },
        Commands::Reg(cmd) => match cmd {
            RegistryCommands::Registry { command } => match command {
                RegistrySubCommands::Connect { url } => commands::registry::connect(&url),
                RegistrySubCommands::Disconnect => commands::registry::disconnect(),
                RegistrySubCommands::Status => commands::registry::status(),
                RegistrySubCommands::Search { query, artifact_type, framework, json } => {
                    commands::registry::search(query.as_deref(), artifact_type.as_deref(), framework.as_deref(), json)
                }
                RegistrySubCommands::List { artifact_type, json } => {
                    commands::registry::list(artifact_type.as_deref(), json)
                }
            },
            RegistryCommands::Publish { directory, owner } => {
                commands::publish::publish(&directory, owner.as_deref())
            }
        },
    }
}
