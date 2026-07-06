use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AgentEndpoint {
    pub host: String,
    pub port: u16,
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("agent not found")]
    NotFound,
    #[error("agent not running (status: {0})")]
    NotRunning(String),
    #[error("agent has no endpoint URL configured")]
    NoEndpoint,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Resolve an agent ID to its network endpoint from the database.
pub async fn resolve(
    db: &PgPool,
    agent_id: Uuid,
) -> Result<AgentEndpoint, ResolveError> {
    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT name, status, url FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?
    .ok_or(ResolveError::NotFound)?;

    if agent.status != "running" {
        return Err(ResolveError::NotRunning(agent.status));
    }

    match agent.url {
        Some(ref url) if !url.is_empty() => {
            let (host, port) = parse_host_port(url);
            Ok(AgentEndpoint {
                host,
                port,
                name: agent.name,
            })
        }
        _ => Err(ResolveError::NoEndpoint),
    }
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
}
