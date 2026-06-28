use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::agent_secrets;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentSpec};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(deploy))
        .route("/", get(list))
        .route("/{name}", get(status))
        .route("/{name}", delete(destroy))
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
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    let container_id = ContainerId::new(&req.name);

    // Resolve agent secrets into env if this agent exists in the catalog, and wire its
    // LLM SDK through the gateway (best-effort; skipped if the gateway isn't configured).
    let mut env = req.env;
    if let Some((agent_id, owner_id)) = resolve_agent_by_name(&state, &req.name).await {
        let secrets = agent_secrets::resolve_agent_env(&state.db, agent_id).await;
        for (k, v) in secrets {
            env.entry(k).or_insert(v);
        }
        crate::llm_wiring::inject_agent_llm_env(&state.db, &mut env, agent_id, Some(owner_id)).await;
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
        Ok(status) => (StatusCode::CREATED, Json(status)).into_response(),
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
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
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

async fn resolve_agent_by_name(state: &AppState, name: &str) -> Option<(Uuid, Uuid)> {
    sqlx::query_as::<_, (Uuid, Uuid)>("SELECT id, owner_id FROM agents WHERE name = $1")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}
