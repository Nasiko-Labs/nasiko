use sqlx::PgPool;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, ContainerRuntime};

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
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Resolve an agent ID to its network endpoint.
///
/// Looks up the agent in the database. If the agent has a stored URL, parses it.
/// Otherwise falls back to the container runtime for endpoint resolution.
pub async fn resolve(
    db: &PgPool,
    runtime: &dyn ContainerRuntime,
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

    if let Some(ref url) = agent.url
        && !url.is_empty()
    {
        let (host, port) = parse_host_port(url);
        return Ok(AgentEndpoint {
            host,
            port,
            name: agent.name,
        });
    }

    let container_id = ContainerId::new(agent.name.clone());
    let endpoint = runtime
        .endpoint(&container_id)
        .await
        .map_err(|e| ResolveError::Runtime(e.to_string()))?;

    let (host, port) = parse_host_port(&endpoint);
    Ok(AgentEndpoint {
        host,
        port,
        name: agent.name,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_with_port() {
        let (host, port) = parse_host_port("http://10.0.0.1:9000/path");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 9000);
    }

    #[test]
    fn parse_host_port_without_port() {
        let (host, port) = parse_host_port("http://my-agent");
        assert_eq!(host, "my-agent");
        assert_eq!(port, 8000);
    }

    #[test]
    fn parse_host_port_bare() {
        let (host, port) = parse_host_port("container-name:8080");
        assert_eq!(host, "container-name");
        assert_eq!(port, 8080);
    }
}
