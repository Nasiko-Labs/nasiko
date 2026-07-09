use std::collections::HashMap;

use tracing::{info, warn};
use uuid::Uuid;

use crate::catalog::models::Agent;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, RuntimeState};

const AGENT_PORT: u16 = 8000;

/// Ensure seed agents are deployed and running.
///
/// Reads `SEED_AGENTS` env var (space-separated image refs, e.g.
/// "nasiko/echo-agent nasiko/nutrition:v2"). For each image:
///
/// 1. Upsert the DB record (insert if new, update image if changed)
/// 2. Check runtime status
/// 3. Deploy if not running or image changed
/// 4. Fetch agent card once healthy
///
/// Designed to run as a background task — does not block server startup.
pub async fn seed_agents_if_configured(state: &AppState) {
    let images = match std::env::var("SEED_AGENTS") {
        Ok(val) if !val.trim().is_empty() => {
            info!(images = %val, "SEED_AGENTS configured, checking deployments");
            val
        }
        _ => {
            info!("SEED_AGENTS not set, skipping agent seeding");
            return;
        }
    };

    let owner_id: Uuid = match sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(id)) => id,
        _ => {
            warn!("no admin user found, cannot seed agents (run bootstrap first)");
            return;
        }
    };

    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let openai_base = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_default();

    for image in images.split_whitespace() {
        let agent_name = extract_name(image);

        let existing = sqlx::query_as::<_, Agent>(
            "SELECT * FROM agents WHERE name = $1",
        )
        .bind(&agent_name)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

        let force_pull = std::env::var("SEED_FORCE_PULL").is_ok();
        let needs_deploy = match &existing {
            None => true,
            Some(agent) => {
                let image_changed = agent.image.as_deref() != Some(image);
                if image_changed || force_pull {
                    true
                } else {
                    // UUID-keyed (see agents::build_agent_spec / RUN-2c) — the deploy a
                    // few lines below keys on the same UUID, so the liveness probe must
                    // too or it always reports "not found" and redeploys every run.
                    let container_id = ContainerId::from_uuid(agent.id);
                    match state.runtime.status(&container_id).await {
                        Ok(status) => status.state != RuntimeState::Running,
                        Err(_) => true,
                    }
                }
            }
        };

        if !needs_deploy {
            info!(agent = %agent_name, "seed agent already running, skipping");
            continue;
        }

        info!(agent = %agent_name, image = %image, "seeding agent");

        let agent = match &existing {
            Some(a) => {
                // Update image if it changed
                if a.image.as_deref() != Some(image) {
                    let _ = sqlx::query(
                        "UPDATE agents SET image = $2, status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .bind(image)
                    .execute(&state.db)
                    .await;
                } else {
                    let _ = sqlx::query(
                        "UPDATE agents SET status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .execute(&state.db)
                    .await;
                }
                a.clone()
            }
            None => match register_agent(&state.db, &agent_name, image, owner_id).await {
                Ok(a) => a,
                Err(e) => {
                    warn!(agent = %agent_name, error = %e, "failed to register seed agent");
                    continue;
                }
            },
        };

        let mut env = HashMap::new();
        env.insert("PORT".into(), AGENT_PORT.to_string());
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

        // UUID-keyed (see agents::build_agent_spec) so a re-seed re-targets the same
        // workload rather than leaving a name-keyed orphan.
        let spec = crate::agents::build_agent_spec(
            agent.id,
            &agent_name,
            image.to_string(),
            vec![AGENT_PORT],
            env,
            None,
        );

        match state.runtime.deploy(&spec).await {
            Ok(status) => {
                info!(agent = %agent_name, ?status, "seed agent deployed");
                let agent_url = status.endpoint.clone().unwrap_or_default();
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .bind(&agent_url)
                .execute(&state.db)
                .await;

                // Wait for container to become healthy, then fetch agent card
                crate::agents::utils::fetch_agent_card_with_retry(
                    state.db.clone(),
                    state.http_client.clone(),
                    agent.id,
                    agent_url.clone(),
                )
                .await;
            }
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to deploy seed agent");
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .execute(&state.db)
                .await;
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
    owner_id: Uuid,
) -> Result<Agent, sqlx::Error> {
    sqlx::query_as::<_, Agent>(
        r#"INSERT INTO agents (name, owner_id, image, status, is_public, metadata)
           VALUES ($1, $2, $3, 'deploying', true, '{"seed": true}')
           RETURNING *"#,
    )
    .bind(name)
    .bind(owner_id)
    .bind(image)
    .fetch_one(db)
    .await
}
