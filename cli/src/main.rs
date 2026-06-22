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
  up         Start local Nasiko cluster (agent devs)
  down       Stop local Nasiko cluster
  connect    Register a CP by URL
  use        Switch active cluster
  clusters   List configured control planes
  auth       Authentication (login/status/logout)
  dev        Start infra only (contributors)

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
  start      Start a stopped agent
  restart    Restart agent container
  rm         Terminate + deregister agent
  secrets    Manage encrypted secrets
  status     Cluster health + metrics

\x1b[33mAgents:\x1b[0m
  agents     Manage and browse agents (ls/get/deploy/search/info/frameworks/list-uploaded/chat)

\x1b[33mIntegrations:\x1b[0m
  github     GitHub integration (status/repos/connect/disconnect/clone)

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
    #[command(after_help = "Creates: AgentCard.json, Dockerfile, src/")]
    New {
        /// Template name (e.g., openai, claude-sdk). Omit for interactive mode.
        template: Option<String>,
        /// Project directory name
        name: Option<String>,
    },
    /// Build agent Docker image
    #[command(after_help = "Reads: Dockerfile, AgentCard.json (for image tag)")]
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
    #[command(after_help = "Checks: AgentCard.json, Dockerfile, src/")]
    Validate {
        /// Agent directory
        #[arg(default_value = ".")]
        directory: String,
    },
    /// Generate or update AgentCard.json
    #[command(after_help = "Writes: AgentCard.json")]
    Card {
        /// Describe what your agent does (used for LLM generation)
        description: Option<String>,
        /// Agent directory
        #[arg(long, default_value = ".")]
        dir: String,
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
    #[command(after_help = "Reads: AgentCard.json, Dockerfile\nWrites: .nasiko/agent.json (agent ID binding)")]
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
        /// Live-tail: stream new log lines as they arrive (SSE)
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Stop agent container
    Stop { agent: String },
    /// Start a stopped agent
    Start { agent: String },
    /// Restart agent container (picks up new secrets/env)
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
    /// GitHub integration
    Github {
        #[command(subcommand)]
        command: GithubCommands,
    },
    /// Manage and browse agents
    Agents {
        #[command(subcommand)]
        command: AgentsCommands,
    },
}

#[derive(Subcommand)]
enum AgentsCommands {
    /// List all deployed agents
    #[command(alias = "list")]
    Ls,
    /// Get details for a specific agent by name or ID
    Get {
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, short = 'f', default_value = "details")]
        format: String,
    },
    /// Deploy an agent from a .zip file or directory
    Deploy {
        source: String,
        #[arg(long, short = 'n')]
        name: Option<String>,
    },
    /// Search the public Nasiko Artifact Registry
    Search {
        query: Option<String>,
        #[arg(long, short = 't')]
        artifact_type: Option<String>,
        #[arg(long, short = 'f')]
        framework: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, short = 'o')]
        owner: Option<String>,
        #[arg(long, short = 'l', default_value = "50")]
        limit: usize,
    },
    /// Get details for a specific artifact from the registry
    Info {
        name: String,
        #[arg(long, short = 'o', default_value = "nasiko")]
        owner: String,
        #[arg(long, short = 'v')]
        version: Option<String>,
    },
    /// List available frameworks in the artifact registry
    Frameworks,
    /// List agents uploaded by the current user
    #[command(name = "list-uploaded")]
    ListUploaded,
    /// Chat directly with a locally running agent
    Chat {
        #[arg(long, short = 'u', default_value = "http://localhost:5000")]
        url: String,
        message: Option<String>,
        #[arg(long, short = 's')]
        session_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum GithubCommands {
    /// Show GitHub connection status
    Status,
    /// List accessible GitHub repositories
    Repos,
    /// Connect GitHub account via OAuth
    Connect,
    /// Disconnect GitHub
    Disconnect,
    /// Clone a GitHub repo and deploy as an agent
    Clone {
        /// Repository (owner/repo). Omit for interactive picker.
        repo: Option<String>,
        /// Branch to clone
        #[arg(long, default_value = "main")]
        branch: String,
    },
}

#[derive(Subcommand)]
#[command(next_help_heading = "Setup")]
enum CpCommands {
    /// Start local Nasiko cluster (pulls CP image from DockerHub)
    #[command(after_help = "Config: ~/.nasiko/dev.env")]
    Up,
    /// Stop local Nasiko cluster
    Down,
    /// Start infra for local development (contributors only)
    #[command(after_help = "Config: ~/.nasiko/dev.env\nThen run: cargo run -p nasiko-server")]
    Dev {
        #[command(subcommand)]
        command: Option<DevCommands>,
    },
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
    /// Manage encrypted secrets
    #[command(after_help = "Secrets are stored on the active cluster (encrypted at rest)")]
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
    /// Semantically discover artifacts by natural-language query
    #[command(alias = "discover")]
    Search {
        /// Natural-language query (e.g. "nutrition planning")
        query: Option<String>,
        /// Filter by type (agent, skill, tool)
        #[arg(short = 't', long = "type")]
        artifact_type: Option<String>,
        /// Filter by framework
        #[arg(short = 'f', long)]
        framework: Option<String>,
        /// Max results to return
        #[arg(long, default_value_t = 10)]
        top: u32,
        /// Minimum relevance score (0.0–1.0) for semantic matches
        #[arg(long)]
        min_score: Option<f32>,
        /// Output as JSON
        #[arg(short = 'j', long)]
        json: bool,
    },
    /// List all artifacts in the registry
    List {
        /// Filter by type (agent, skill, tool)
        #[arg(short = 't', long = "type")]
        artifact_type: Option<String>,
        /// Output as JSON
        #[arg(short = 'j', long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DevCommands {
    /// Stop dev infrastructure
    Stop,
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
    /// Set a secret (vault-wide, or agent-specific with --agent)
    Set {
        key: String,
        value: String,
        /// Target a specific agent (otherwise sets in your vault)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Get a secret value
    Get {
        key: String,
        /// Target a specific agent
        #[arg(long)]
        agent: Option<String>,
    },
    /// List secrets
    Ls {
        /// Target a specific agent (otherwise lists your vault)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Remove a secret
    Rm {
        key: String,
        /// Target a specific agent
        #[arg(long)]
        agent: Option<String>,
    },
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
            AgentDevCommands::Card { description, dir } => commands::card::card(&dir, description.as_deref()),
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
            AgentOpsCommands::Ps { json } => commands::agents::ps(json),
            AgentOpsCommands::Logs { agent, tail, follow } => commands::agents::logs(&agent, tail, follow),
            AgentOpsCommands::Stop { agent } => commands::agents::stop(&agent),
            AgentOpsCommands::Start { agent } => commands::agents::start(&agent),
            AgentOpsCommands::Restart { agent } => commands::agents::restart(&agent),
            AgentOpsCommands::Rm { agent, force } => commands::agents::rm(&agent, force),
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
            AgentOpsCommands::Agents { command } => match command {
                AgentsCommands::Ls => commands::agents::cmd_ls(),
                AgentsCommands::Get { agent_id, name, format } => {
                    commands::agents::cmd_get(agent_id.as_deref(), name.as_deref(), &format)
                }
                AgentsCommands::Deploy { source, name } => {
                    commands::agents::cmd_deploy(&source, name.as_deref())
                }
                AgentsCommands::Search { query, artifact_type, framework, tags, owner, limit } => {
                    commands::agents::cmd_search(query.as_deref(), artifact_type.as_deref(), framework.as_deref(), tags.as_deref(), owner.as_deref(), limit)
                }
                AgentsCommands::Info { name, owner, version } => {
                    commands::agents::cmd_info(&name, &owner, version.as_deref())
                }
                AgentsCommands::Frameworks => commands::agents::cmd_frameworks(),
                AgentsCommands::ListUploaded => commands::agents::cmd_list_uploaded(),
                AgentsCommands::Chat { url, message, session_id } => {
                    commands::chat::agent_chat(&url, message.as_deref(), session_id.as_deref())
                }
            },
            AgentOpsCommands::Github { command } => match command {
                GithubCommands::Status => commands::github::status(),
                GithubCommands::Repos => commands::github::repos(),
                GithubCommands::Connect => commands::github::connect(),
                GithubCommands::Disconnect => commands::github::disconnect(),
                GithubCommands::Clone { repo, branch } => {
                    commands::github::clone(repo.as_deref(), Some(branch.as_str()))
                }
            },
        },
        Commands::Cp(cmd) => match cmd {
            CpCommands::Up => commands::dev::start(false),
            CpCommands::Down => commands::dev::stop(),
            CpCommands::Dev { command } => match command {
                None => commands::dev::start(true),
                Some(DevCommands::Stop) => commands::dev::stop(),
                Some(DevCommands::Env) => commands::dev::env_template(),
            },
            CpCommands::Connect { url, name } => commands::cluster::connect(&url, name.as_deref()),
            CpCommands::Use { name } => commands::cluster::use_cluster(&name),
            CpCommands::Clusters => commands::cluster::list(),
            CpCommands::Status => commands::status::status(),
            CpCommands::Auth { command } => match command {
                AuthCommands::Login => commands::auth::login(),
                AuthCommands::Status => commands::auth::status(),
                AuthCommands::Logout => commands::auth::logout(),
            },
            CpCommands::Secrets { command } => match command {
                SecretsCommands::Set { key, value, agent } => commands::secrets::set(&key, &value, agent.as_deref()),
                SecretsCommands::Get { key, agent } => commands::secrets::get(&key, agent.as_deref()),
                SecretsCommands::Ls { agent } => commands::secrets::ls(agent.as_deref()),
                SecretsCommands::Rm { key, agent } => commands::secrets::rm(&key, agent.as_deref()),
            },
        },
        Commands::Reg(cmd) => match cmd {
            RegistryCommands::Registry { command } => match command {
                RegistrySubCommands::Connect { url } => commands::registry::connect(&url),
                RegistrySubCommands::Disconnect => commands::registry::disconnect(),
                RegistrySubCommands::Status => commands::registry::status(),
                RegistrySubCommands::Search { query, artifact_type, framework, top, min_score, json } => {
                    commands::registry::search(query.as_deref(), artifact_type.as_deref(), framework.as_deref(), top, min_score, json)
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
