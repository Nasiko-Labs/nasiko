use sqlx::PgPool;
use uuid::Uuid;

pub mod context_part;

pub const SESSION_MEMORY_EXTENSION: &str = "urn:nasiko:a2a:session-memory";

/// A `host:port` snapshot parsed from `agents.url`.
#[derive(Debug, Clone)]
pub struct AgentEndpoint {
    pub host: String,
    pub port: u16,
}

/// A running agent resolved from the catalog.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub name: String,
    /// `None` when `agents.url` is empty: the row exists and the agent is
    /// running, but no endpoint snapshot has been persisted yet (a fresh
    /// Kubernetes deploy returns before the pod is Ready, so deploy-time
    /// persistence can race the workload). Callers should prefer a live
    /// runtime lookup and treat a missing snapshot as fatal only when that
    /// lookup fails too.
    pub endpoint: Option<AgentEndpoint>,
    /// The agent's A2A mount path (`agents.transport_path`, from its card) —
    /// e.g. `/a2a` for the a2a-go agents, `/` for the a2a-server-lf agents.
    /// `None`/empty/`/` mean root. The proxy must honor this: the A2A spec
    /// fixes no path, so POSTing a message to `/` 404s agents mounted elsewhere.
    pub transport_path: Option<String>,
    pub is_internal: bool,
    pub keeps_session_memory: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("agent not found")]
    NotFound,
    #[error("agent not running (status: {0})")]
    NotRunning(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Resolve an agent ID to its catalog record (name + stored endpoint
/// snapshot) from the database.
pub async fn resolve(db: &PgPool, agent_id: Uuid) -> Result<ResolvedAgent, ResolveError> {
    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT name, status, url, transport_path, is_internal, \
         is_internal AND COALESCE(capabilities -> 'extensions' @> jsonb_build_array(jsonb_build_object('uri', $2::text)), false) \
         AS keeps_session_memory FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .bind(SESSION_MEMORY_EXTENSION)
    .fetch_optional(db)
    .await?
    .ok_or(ResolveError::NotFound)?;

    if agent.status != "running" {
        return Err(ResolveError::NotRunning(agent.status));
    }

    let endpoint = match agent.url {
        Some(ref url) if !url.is_empty() => {
            let (host, port) = parse_host_port(url);
            Some(AgentEndpoint { host, port })
        }
        _ => None,
    };
    Ok(ResolvedAgent {
        name: agent.name,
        endpoint,
        transport_path: agent.transport_path,
        is_internal: agent.is_internal,
        keeps_session_memory: agent.keeps_session_memory,
    })
}

fn parse_host_port(url: &str) -> (String, u16) {
    let stripped = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    if let Some((h, p)) = host_port.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().unwrap_or(8000))
    } else {
        (host_port.to_string(), 8000)
    }
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    name: String,
    status: String,
    url: Option<String>,
    transport_path: Option<String>,
    is_internal: bool,
    keeps_session_memory: bool,
}
