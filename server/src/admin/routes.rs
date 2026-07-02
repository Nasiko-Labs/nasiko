use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Claims;
use crate::catalog::agent_secrets;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentSpec};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(deploy))
        .route("/", get(list))
        .route("/{name}", get(status))
        .route("/{name}", delete(destroy))
        .route("/{name}/stop", post(stop))
        .route("/{name}/start", post(start))
        .route("/{name}/restart", post(restart))
        .route("/{name}/scale", post(scale))
        .route("/{name}/logs", get(logs))
}

#[derive(Deserialize)]
struct DeployRequest {
    image: String,
    name: String,
    #[serde(default)]
    ports: Vec<u16>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    #[serde(default)]
    replicas: Option<u32>,
}

async fn deploy(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    let container_id = ContainerId::new(&req.name);

    // Start with env from request (inline -e flags)
    let mut env = req.env;

    // Resolve vault + agent secrets (vault = base, agent = override, request = highest)
    if let Some(agent_id) = resolve_agent_id_by_name(&state, &req.name).await {
        let owner_id: Uuid = claims.sub.parse().unwrap_or_default();
        let resolved = resolve_full_env(&state.db, owner_id, agent_id).await;
        // resolved secrets are base; request env overrides
        for (k, v) in resolved {
            env.entry(k).or_insert(v);
        }
    }

    let spec = DeploymentSpec {
        container_id,
        name: req.name.clone(),
        image: req.image,
        ports: if req.ports.is_empty() { vec![8000] } else { req.ports },
        env_vars: env,
        min_replicas: req.replicas.unwrap_or(1),
        max_replicas: req.replicas.unwrap_or(1),
        resources: None,
    };

    match state.runtime.deploy(&spec).await {
        Ok(status) => {
            // Update the catalog URL + record deployment (fire-and-forget).
            if let Some(agent_id) = resolve_agent_id_by_name(&state, &req.name).await {
                let db = state.db.clone();
                let endpoint = status.endpoint.clone();
                tokio::spawn(async move {
                    // Write the live endpoint URL + running status back to the catalog.
                    if let Some(url) = endpoint {
                        let _ = sqlx::query(
                            "UPDATE agents SET url = $1, status = 'running', updated_at = now() WHERE id = $2",
                        )
                        .bind(&url)
                        .bind(agent_id)
                        .execute(&db)
                        .await;
                    }

                    let build_id: Option<Uuid> = sqlx::query_scalar(
                        "SELECT id FROM agent_builds WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
                    )
                    .bind(agent_id)
                    .fetch_optional(&db)
                    .await
                    .ok()
                    .flatten();

                    if let Some(build_id) = build_id {
                        let _ = sqlx::query(
                            "INSERT INTO agent_deployments (agent_id, build_id, status)
                             VALUES ($1, $2, 'running')",
                        )
                        .bind(agent_id)
                        .bind(build_id)
                        .execute(&db)
                        .await;
                    }
                });
            }
            (StatusCode::CREATED, Json(status)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    match state.runtime.list().await {
        Ok(containers) => Json(containers).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn status(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.status(&id).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

async fn destroy(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.destroy(&id).await {
        Ok(()) => {
            if let Some(agent_id) = resolve_agent_id_by_name(&state, &name).await {
                let db = state.db.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE agent_deployments SET status = 'stopped', updated_at = now()
                         WHERE agent_id = $1 AND status != 'stopped'",
                    )
                    .bind(agent_id)
                    .execute(&db)
                    .await;
                });
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn stop(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.scale(&id, 0).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn start(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.scale(&id, 1).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn restart(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Look up agent record to get image, owner, and port
    let agent: Option<(Uuid, Uuid, String, i32)> = sqlx::query_as(
        "SELECT id, owner_id, image, port FROM agents WHERE name = $1",
    )
    .bind(&name)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((agent_id, owner_id, image, port)) = agent else {
        // No agent record — fall back to simple container restart
        let id = ContainerId::new(&name);
        return match state.runtime.restart(&id).await {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
    };

    // Resolve env: vault (base) + agent secrets (override)
    let env = resolve_full_env(&state.db, owner_id, agent_id).await;

    let container_id = ContainerId::new(&name);

    // Destroy and redeploy with fresh env
    let _ = state.runtime.destroy(&container_id).await;

    let spec = DeploymentSpec {
        container_id,
        name: name.clone(),
        image,
        ports: vec![port as u16],
        env_vars: env,
        min_replicas: 1,
        max_replicas: 1,
        resources: None,
    };

    match state.runtime.deploy(&spec).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct ScaleRequest {
    replicas: u32,
}

async fn scale(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.scale(&id, req.replicas).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: u32,
}
fn default_tail() -> u32 { 100 }

async fn logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    axum::extract::Query(q): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let id = ContainerId::new(&name);
    match state.runtime.logs(&id, q.tail).await {
        Ok(lines) => Json(lines).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn resolve_agent_id_by_name(state: &AppState, name: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE name = $1")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

/// Resolve the full env for an agent: vault secrets (base) + agent secrets (override).
async fn resolve_full_env(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    agent_id: Uuid,
) -> std::collections::HashMap<String, String> {
    use crate::secrets::crypto::SecretsCrypto;

    let crypto = SecretsCrypto::for_user(owner_id);
    let mut env = std::collections::HashMap::new();

    // 1. Vault secrets (user-level, lower precedence)
    let vault_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, encrypted_value FROM user_secrets WHERE user_id = $1",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    for (name, encrypted) in vault_rows {
        if let Ok(value) = crypto.decrypt(&encrypted) {
            env.insert(name, value);
        }
    }

    // 2. Agent secrets (higher precedence — overrides vault)
    let agent_secrets = agent_secrets::resolve_agent_env(db, agent_id).await;
    for (k, v) in agent_secrets {
        env.insert(k, v);
    }

    env
}
