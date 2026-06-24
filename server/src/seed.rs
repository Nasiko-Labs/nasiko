use std::collections::HashMap;

use tracing::{info, warn};
use uuid::Uuid;

use crate::catalog::models::Agent;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentSpec};

const OSS_USER_ID: Uuid = Uuid::nil();

/// Seed reference agents from pre-built public Docker images.
///
/// Reads `SEED_AGENTS` env var (space-separated image refs, e.g.
/// "nasiko/echo-agent nasiko/qa-bot"). For each image that doesn't
/// already have a registered agent, creates a DB entry and deploys
/// the container via the orchestrator.
pub async fn seed_agents_if_configured(state: &AppState) {
    let images = match std::env::var("SEED_AGENTS") {
        Ok(val) if !val.trim().is_empty() => val,
        _ => return,
    };

    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let openai_base = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_default();

    for (idx, image) in images.split_whitespace().enumerate() {
        let agent_name = extract_name(image);
        let agent_port = 5000u16 + idx as u16;

        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE name = $1)",
        )
        .bind(&agent_name)
        .fetch_one(&state.db)
        .await
        .unwrap_or(true);

        if exists {
            info!(agent = %agent_name, "seed agent already registered, skipping");
            continue;
        }

        info!(agent = %agent_name, image = %image, "seeding reference agent");

        let agent = match register_agent(&state.db, &agent_name, image).await {
            Ok(a) => a,
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to register seed agent");
                continue;
            }
        };

        let mut env = HashMap::new();
        if !openai_key.is_empty() {
            env.insert("OPENAI_API_KEY".into(), openai_key.clone());
        }
        if !openai_base.is_empty() {
            env.insert("OPENAI_BASE_URL".into(), openai_base.clone());
        }
        if !otel_endpoint.is_empty() {
            env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), otel_endpoint.clone());
        }
        let discovery_url = std::env::var("A2A_DISCOVERY_URL")
            .unwrap_or_else(|_| "http://host.docker.internal:8080".into());
        env.insert("A2A_DISCOVERY_URL".into(), discovery_url);

        let spec = DeploymentSpec {
            container_id: ContainerId::new(agent_name.clone()),
            name: agent_name.clone(),
            image: image.to_string(),
            min_replicas: 1,
            max_replicas: 1,
            env_vars: env,
            ports: vec![agent_port],
            resources: None,
        };

        match state.runtime.deploy(&spec).await {
            Ok(status) => {
                info!(agent = %agent_name, ?status, "seed agent deployed");
                let agent_url = status.endpoint.clone().unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'running', url = $2 WHERE id = $1",
                )
                .bind(agent.id)
                .bind(&agent_url)
                .execute(&state.db)
                .await;

                // Wait for container to be ready, then fetch agent card
                for _ in 0..10 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if fetch_and_apply_agent_card(state, agent.id, &agent_url).await {
                        break;
                    }
                }
            }
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to deploy seed agent");
            }
        }
    }
}

/// Extract agent name from image ref: "nasiko/echo-agent:v1" -> "echo-agent"
fn extract_name(image: &str) -> String {
    let without_tag = image.split(':').next().unwrap_or(image);
    without_tag
        .rsplit('/')
        .next()
        .unwrap_or(without_tag)
        .to_string()
}

async fn register_agent(
    db: &sqlx::PgPool,
    name: &str,
    image: &str,
) -> Result<Agent, sqlx::Error> {
    sqlx::query_as::<_, Agent>(
        r#"INSERT INTO agents (name, owner_id, image, status, metadata)
           VALUES ($1, $2, $3, 'deploying', '{"seed": true}')
           RETURNING *"#,
    )
    .bind(name)
    .bind(OSS_USER_ID)
    .bind(image)
    .fetch_one(db)
    .await
}

/// After deploy, fetch the agent's card and update the DB with skills/description.
/// Returns true if the card was fetched successfully.
async fn fetch_and_apply_agent_card(state: &AppState, agent_id: Uuid, agent_url: &str) -> bool {
    if agent_url.is_empty() {
        return false;
    }

    let base = agent_url.trim_end_matches('/');

    // Try both well-known paths (new spec uses agent-card.json)
    let urls = [
        format!("{base}/.well-known/agent-card.json"),
        format!("{base}/.well-known/agent.json"),
    ];

    let mut card: Option<serde_json::Value> = None;
    for url in &urls {
        if let Ok(resp) = state.http_client.get(url).send().await
            && resp.status().is_success()
                && let Ok(v) = resp.json::<serde_json::Value>().await {
                    card = Some(v);
                    break;
                }
    }

    let card = match card {
        Some(c) => c,
        None => return false,
    };

    let _ = sqlx::query(
        r#"UPDATE agents SET
             description = COALESCE($2, description),
             skills = COALESCE($3, skills),
             tags = COALESCE($4, tags),
             capabilities = COALESCE($5, capabilities),
             updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
    .bind(card.get("description").and_then(|v| v.as_str()))
    .bind(card.get("skills"))
    .bind(
        card.get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            }),
    )
    .bind(card.get("capabilities"))
    .execute(&state.db)
    .await;

    true
}
