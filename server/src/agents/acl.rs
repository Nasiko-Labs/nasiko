use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::acl::allowed_targets;
use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/acl", get(get_agent_acl))
        .route("/{id}/acl", post(add_agent_acl))
        .route("/{id}/acl/{target_id}", delete(remove_agent_acl))
}

#[derive(Debug, Serialize)]
struct AclResponse {
    /// Always false — default-deny semantics: explicit grants required.
    unrestricted: bool,
    allowed: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AddAclBody {
    target_agent_id: Uuid,
}

// ─── GET /{id}/acl ───────────────────────────────────────────────────────────

async fn get_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if !crate::acl::can_access_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match allowed_targets(&state.db, agent_id).await {
        Ok(targets) => Json(AclResponse {
            unrestricted: false,
            allowed: targets,
        })
        .into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "get_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── POST /{id}/acl ──────────────────────────────────────────────────────────

async fn add_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<AddAclBody>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Verify target agent exists.
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(body.target_agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !target_exists {
        return (StatusCode::NOT_FOUND, "target agent not found").into_response();
    }

    match sqlx::query(
        "INSERT INTO agent_acl (caller_agent_id, target_agent_id, granted_by)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(agent_id)
    .bind(body.target_agent_id)
    .bind(user_id)
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "add_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── DELETE /{id}/acl/{target_id} ────────────────────────────────────────────

async fn remove_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, target_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match sqlx::query("DELETE FROM agent_acl WHERE caller_agent_id = $1 AND target_agent_id = $2")
        .bind(agent_id)
        .bind(target_id)
        .execute(&state.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, %target_id, "remove_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
