pub mod api;
pub mod commands;
pub mod config;
pub mod oci;
pub mod skill;
pub mod util;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
#[command(next_help_heading = "Create")]
pub enum AgentDevCommands {
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
pub enum AgentOpsCommands {
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
    /// Upload source directory or .zip and let the server build + deploy (no local Docker needed)
    #[command(after_help = "Reads: AgentCard.json (for name/version defaults)\nSource can be a directory (auto-zipped) or a pre-made .zip file")]
    Upload {
        /// Agent directory or .zip file (defaults to current directory)
        #[arg(default_value = ".")]
        source: String,
        /// Agent name (defaults to 'name' in AgentCard.json, then directory name)
        #[arg(long, short = 'n')]
        name: Option<String>,
        /// Version tag (defaults to 'version' in AgentCard.json, then 'latest')
        #[arg(long, short = 'v')]
        version: Option<String>,
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
    /// Scale agent container to N replicas
    Scale { agent: String, replicas: u32 },
    /// Terminate + deregister agent
    Rm {
        agent: String,
        #[arg(short, long)]
        force: bool,
    },
    /// Send a message via A2A protocol
    Chat {
        /// A2A endpoint URL (uses active cluster if omitted)
        url: Option<String>,
        /// Message (omit for interactive mode)
        message: Option<String>,
        /// Chat directly with a deployed agent by name or ID (resolved via the CP registry)
        #[arg(long, short = 'a')]
        agent: Option<String>,
        /// Launch full-screen TUI (ratatui)
        #[arg(long)]
        tui: bool,
        /// Resume a previous session by ID (TUI mode)
        #[arg(long)]
        resume: Option<String>,
        /// Route message to an existing session (one-shot mode)
        #[arg(long)]
        session_id: Option<String>,
    },
    /// List chat sessions
    Sessions {
        /// A2A endpoint URL (uses active cluster if omitted)
        #[arg(long)]
        endpoint: Option<String>,
        /// Pagination cursor from a previous listing
        #[arg(long)]
        cursor: Option<String>,
        /// Maximum number of sessions to return (default 50)
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Create a new chat session on the active cluster
    #[command(name = "create-session")]
    CreateSession {
        /// A2A agent URL to associate with the session
        #[arg(long)]
        agent: Option<String>,
    },
    /// Show message history for a session
    #[command(name = "history")]
    History {
        /// Session ID
        session_id: String,
    },
    /// Delete a chat session
    #[command(name = "delete-session")]
    DeleteSession {
        /// Session ID
        session_id: String,
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
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
    /// Observability: agent stats, distributed traces, FinOps cost dashboard
    Observe {
        #[command(subcommand)]
        command: ObserveCommands,
    },
    /// Manage encrypted secrets
    #[command(after_help = "Secrets are stored on the active cluster (encrypted at rest)")]
    Secrets {
        #[command(subcommand)]
        command: SecretsCommands,
    },
}

#[derive(Subcommand)]
pub enum AgentsCommands {
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
pub enum GithubCommands {
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
pub enum ObserveCommands {
    /// List sessions across all agents (ObservabilityService / Tempo)
    Sessions {
        /// ISO-8601 start of the reporting window (default: 7 days ago)
        #[arg(long)]
        start_time: Option<String>,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show full detail for a session (traces, tokens, cost)
    Session {
        /// Session ID (trace ID)
        session_id: String,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show span tree and costs for a trace (ObservabilityService)
    #[command(name = "trace-detail")]
    TraceDetail {
        /// Trace ID
        trace_id: String,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show detail for a single span including prompt/completion content
    Span {
        /// Trace ID
        trace_id: String,
        /// Span ID
        span_id: String,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show project-level stats for an agent (ObservabilityService)
    #[command(name = "project-stats")]
    ProjectStats {
        /// Agent ID (name or UUID)
        agent_id: String,
        /// ISO-8601 start of the reporting window (default: 24 h ago)
        #[arg(long)]
        start_time: Option<String>,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show FinOps cost dashboard (ObservabilityService)
    #[command(name = "finops-dashboard")]
    FinopsDashboard {
        /// ISO-8601 start of the cost window (default: 24 h ago)
        #[arg(long)]
        start_time: Option<String>,
        /// Output the raw API response as JSON
        #[arg(long)]
        json: bool,
    },
    /// Fetch AI-powered cost insights (calls finops-dashboard then LLM)
    Insights {
        /// ISO-8601 start of the cost window (default: 24 h ago)
        #[arg(long)]
        start_time: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SecretsCommands {
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
pub enum SkillCommands {
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

#[derive(Subcommand)]
pub enum RegistrySubCommands {
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
        /// Minimum relevance score (0.0-1.0) for semantic matches
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

pub fn dispatch_agent_dev(cmd: AgentDevCommands) -> Result<()> {
    match cmd {
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
    }
}

pub fn dispatch_agent_ops(cmd: AgentOpsCommands) -> Result<()> {
    match cmd {
        AgentOpsCommands::Deploy { image, name, port, env_file, env } => {
            commands::deploy::deploy(&image, name.as_deref(), port, env_file.as_deref(), &env)
        }
        AgentOpsCommands::Push { image, name } => {
            commands::push::push(&image, name.as_deref())
        }
        AgentOpsCommands::Upload { source, name, version, port, env_file, env } => {
            commands::upload::upload(&source, name.as_deref(), version.as_deref(), port, env_file.as_deref(), &env)
        }
        AgentOpsCommands::Ps { json } => commands::agents::ps(json),
        AgentOpsCommands::Logs { agent, tail, follow } => commands::agents::logs(&agent, tail, follow),
        AgentOpsCommands::Stop { agent } => commands::agents::stop(&agent),
        AgentOpsCommands::Start { agent } => commands::agents::start(&agent),
        AgentOpsCommands::Restart { agent } => commands::agents::restart(&agent),
        AgentOpsCommands::Scale { agent, replicas } => commands::agents::scale(&agent, replicas),
        AgentOpsCommands::Rm { agent, force } => commands::agents::rm(&agent, force),
        AgentOpsCommands::Chat { url, message, agent, tui, resume, session_id } => {
            // `nasiko chat "some message"` — a lone positional *containing
            // a space* is a natural-language message for the orchestrator,
            // not a resolvable target. Targets (a URL, an agent UUID/name,
            // or "orchestrator") are always a single token, so checking
            // for whitespace — not "isn't an http(s) URL" — is what
            // actually distinguishes the two: the old check reclassified
            // *any* non-URL positional (including a bare agent name) as
            // the message, so `nasiko chat my-agent` sent "my-agent" as
            // the message to the orchestrator instead of resolving it as
            // the chat target.
            let (url, message) = match (url, message) {
                (Some(u), None) if u.contains(' ') => (None, Some(u)),
                other => other,
            };
            let target_label = agent.as_deref().unwrap_or("").to_string();
            let resolved = match (url, agent) {
                // `url` is documented to accept a full URL, an agent
                // UUID/name, or "orchestrator" — `resolve_chat_target`
                // is what actually implements that (URL passthrough,
                // orchestrator special-case, agent lookup by id/name);
                // using `u` as-is here skipped that resolution entirely,
                // so `nasiko chat <agent-name>` sent the literal name as
                // the HTTP endpoint instead of resolving it first.
                (Some(u), _) => commands::agents::resolve_chat_target(&u)?,
                (None, Some(a)) => {
                    let base = config::active_url()?;
                    let id = commands::agents::resolve_agent_id(&a)?;
                    format!("{}/api/agents/{}", base.trim_end_matches('/'), id)
                }
                (None, None) => {
                    let base = config::active_url()?;
                    format!("{}/api/orchestrator/a2a", base.trim_end_matches('/'))
                }
            };
            if tui || resume.is_some() {
                commands::tui::run_tui(&resolved, resume.as_deref())
            } else {
                commands::chat::chat(&resolved, message.as_deref(), session_id.as_deref(), &target_label)
            }
        }
        AgentOpsCommands::Sessions { endpoint, cursor, limit } => {
            commands::tui::list_sessions(endpoint.as_deref(), cursor.as_deref(), limit)
        }
        AgentOpsCommands::CreateSession { agent } => {
            commands::tui::create_session(agent.as_deref())
        }
        AgentOpsCommands::History { session_id } => {
            commands::tui::session_history(&session_id)
        }
        AgentOpsCommands::DeleteSession { session_id, yes } => {
            commands::tui::delete_session(&session_id, yes)
        }
        AgentOpsCommands::Github { command } => match command {
            GithubCommands::Status => commands::github::status(),
            GithubCommands::Repos => commands::github::repos(),
            GithubCommands::Connect => commands::github::connect(),
            GithubCommands::Disconnect => commands::github::disconnect(),
            GithubCommands::Clone { repo, branch } => {
                commands::github::clone(repo.as_deref(), Some(branch.as_str()))
            }
        },
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
        AgentOpsCommands::Observe { command } => match command {
            ObserveCommands::Sessions { start_time, json } => {
                commands::observe::sessions(start_time.as_deref(), json)
            }
            ObserveCommands::Session { session_id, json } => {
                commands::observe::session_detail(&session_id, json)
            }
            ObserveCommands::TraceDetail { trace_id, json } => {
                commands::observe::trace_detail(&trace_id, json)
            }
            ObserveCommands::Span { trace_id, span_id, json } => {
                commands::observe::span_detail(&trace_id, &span_id, json)
            }
            ObserveCommands::ProjectStats { agent_id, start_time, json } => {
                commands::observe::project_stats(&agent_id, start_time.as_deref(), json)
            }
            ObserveCommands::FinopsDashboard { start_time, json } => {
                commands::observe::finops_dashboard(start_time.as_deref(), json)
            }
            ObserveCommands::Insights { start_time } => {
                commands::observe::insights(start_time.as_deref())
            }
        },
        AgentOpsCommands::Secrets { command } => match command {
            SecretsCommands::Set { key, value, agent } => commands::secrets::set(&key, &value, agent.as_deref()),
            SecretsCommands::Get { key, agent } => commands::secrets::get(&key, agent.as_deref()),
            SecretsCommands::Ls { agent } => commands::secrets::ls(agent.as_deref()),
            SecretsCommands::Rm { key, agent } => commands::secrets::rm(&key, agent.as_deref()),
        },
    }
}

pub fn dispatch_registry(cmd: RegistrySubCommands) -> Result<()> {
    match cmd {
        RegistrySubCommands::Connect { url } => commands::registry::connect(&url),
        RegistrySubCommands::Disconnect => commands::registry::disconnect(),
        RegistrySubCommands::Status => commands::registry::status(),
        RegistrySubCommands::Search { query, artifact_type, framework, top, min_score, json } => {
            commands::registry::search(query.as_deref(), artifact_type.as_deref(), framework.as_deref(), top, min_score, json)
        }
        RegistrySubCommands::List { artifact_type, json } => {
            commands::registry::list(artifact_type.as_deref(), json)
        }
    }
}
