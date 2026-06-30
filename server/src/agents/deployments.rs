use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::acl::user_can_access_agent;
use crate::auth::Claims;
use crate::catalog::agent_secrets;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/deployments", get(list_deployments))
        .route("/deployment/{deployment_id}/restart", post(restart_deployment))
        .route("/{id}/deployment", get(get_agent_deployment))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DeploymentRow {
    id: Uuid,
    agent_id: Uuid,
    build_id: Uuid,
    namespace: String,
    replicas: i16,
    status: String,
    service_url: Option<String>,
    owner_id: Option<Uuid>,
    agent_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    crash_reason: Option<String>,
    crashed_at: Option<chrono::DateTime<chrono::Utc>>,
    last_logs: Option<String>,
    restart_count: i32,
    k8s_deployment_name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AgentDeployInfo {
    name: String,
    image: String,
    agent_id: Uuid,
    build_id: Option<Uuid>,
    owner_id: Option<Uuid>,
    /// Stored ports from migration 013 — None for agents deployed before the migration.
    spec_ports: Option<Vec<i32>>,
    /// Stored image from migration 013 — None falls back to agents.image.
    spec_image: Option<String>,
    /// K8s deployment name when running on Kubernetes. None for Docker agents.
    k8s_deployment_name: Option<String>,
}

// ─── GET /deployments ────────────────────────────────────────────────────────

async fn list_deployments(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let rows = if claims.is_superuser {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at,
                    d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                    d.k8s_deployment_name
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at,
                    d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                    d.k8s_deployment_name
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             WHERE d.owner_id = $1
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_deployments db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── GET /{id}/deployment ────────────────────────────────────────────────────

async fn get_agent_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match sqlx::query_as::<_, DeploymentRow>(
        "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                d.status::text as status, d.service_url, d.owner_id,
                a.name as agent_name, d.created_at,
                d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                d.k8s_deployment_name
         FROM agent_deployments d
         LEFT JOIN agents a ON a.id = d.agent_id
         WHERE d.agent_id = $1 AND d.status != 'stopped'
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "get_agent_deployment db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── POST /deployment/{id}/restart ──────────────────────────────────────────

async fn restart_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(deployment_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Fetch deployment and agent info together, including stored spec columns.
    let info = match sqlx::query_as::<_, AgentDeployInfo>(
        "SELECT a.name, a.image, a.id as agent_id,
                d.build_id, d.owner_id,
                d.spec_ports, d.spec_image, d.k8s_deployment_name
         FROM agent_deployments d
         JOIN agents a ON a.id = d.agent_id
         WHERE d.id = $1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %deployment_id, "restart_deployment: fetch error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Owner or superuser only.
    if !claims.is_superuser {
        let is_owner = info.owner_id.map(|o| o == user_id).unwrap_or(false);
        if !is_owner && !user_can_access_agent(&state.db, user_id, info.agent_id).await {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let container_id = ContainerId::new(&info.name);

    // 409 if the agent is already running or starting — no-op restart makes no sense.
    match state.runtime.status(&container_id).await {
        Ok(status) if matches!(
            status.state,
            nasiko_runtime::RuntimeState::Running | nasiko_runtime::RuntimeState::Pending
        ) => {
            return (StatusCode::CONFLICT, "agent is already running or starting").into_response();
        }
        _ => {} // crashed, stopped, failed, unknown, or not found → proceed
    }

    // Resolve stored ports; fall back to port 8000 for pre-migration agents.
    let ports: Vec<u16> = if let Some(stored) = info.spec_ports.filter(|v| !v.is_empty()) {
        tracing::info!(agent_id = %info.agent_id, ports = ?stored, "restart: using stored spec ports");
        stored.into_iter().map(|p| p as u16).collect()
    } else {
        tracing::warn!(agent_id = %info.agent_id, "restart: spec_ports not stored, falling back to port 8000");
        vec![8000]
    };

    // Resolve stored image; fall back to agents.image for pre-migration agents.
    let image = info.spec_image.unwrap_or_else(|| info.image.clone());

    // Resolve agent secrets for environment.
    let secrets = agent_secrets::resolve_agent_env(&state.db, info.agent_id).await;

    if let Some(k8s_name) = &info.k8s_deployment_name {
        // ── K8s path: scale-to-1 (avoids tearing down and recreating the Deployment) ──
        tracing::info!(agent_id = %info.agent_id, k8s_name, "restart: using K8s scale-to-1 path");
        let k8s_id = ContainerId::new(k8s_name);
        if let Err(e) = state.runtime.scale(&k8s_id, 1).await {
            tracing::error!(%e, %deployment_id, k8s_name, "restart_deployment: scale-to-1 failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("scale failed: {e}")).into_response();
        }
    } else {
        // ── Docker path: destroy + recreate ──────────────────────────────────────────
        let _ = state.runtime.destroy(&container_id).await;

        let spec = DeploymentSpec {
            container_id: ContainerId::new(&info.name),
            name: info.name.clone(),
            image,
            ports,
            env_vars: secrets,
            min_replicas: 1,
            max_replicas: 1,
            resources: None,
        };

        if let Err(e) = state.runtime.deploy(&spec).await {
            tracing::error!(%e, %deployment_id, "restart_deployment: deploy failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("deploy failed: {e}")).into_response();
        }
    }

    // Mark old deployment stopped, insert new deployment row.
    let _ = sqlx::query(
        "UPDATE agent_deployments SET status = 'stopped', updated_at = now() WHERE id = $1",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await;

    let new_deploy_id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id)
         VALUES ($1, $2, 'running', $3)
         RETURNING id",
    )
    .bind(info.agent_id)
    .bind(info.build_id)
    .bind(info.owner_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    // Clear crash fields on the new deployment row.
    if let Some(new_id) = new_deploy_id {
        let _ = sqlx::query(
            "UPDATE agent_deployments SET
               crash_reason = NULL, crashed_at = NULL, last_logs = NULL,
               restart_count = 0, pod_name = NULL
             WHERE id = $1",
        )
        .bind(new_id)
        .execute(&state.db)
        .await;
        tracing::info!(agent_id = %info.agent_id, "restart: crash fields cleared");
    }

    let _ = sqlx::query("UPDATE agents SET status = 'running', updated_at = now() WHERE id = $1")
        .bind(info.agent_id)
        .execute(&state.db)
        .await;

    match new_deploy_id {
        Some(id) => (StatusCode::OK, Json(serde_json::json!({ "deployment_id": id }))).into_response(),
        None => StatusCode::OK.into_response(),
    }
}
