use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // Overall grant summary
        .route("/{id}/grants", get(list_grants))
        // Public visibility
        .route(
            "/{id}/grants/public",
            post(make_public).delete(make_private),
        )
        .route("/{id}/visibility", get(get_visibility))
        // Per-user shares
        .route("/{id}/users", get(list_user_grants))
        .route(
            "/{id}/grants/users",
            get(list_user_grants).post(add_user_grant),
        )
        .route("/{id}/grants/users/{user_id}", delete(remove_user_grant))
        // Agent-to-agent call ACL
        .route(
            "/{id}/grants/agents",
            get(list_agent_grants).post(add_agent_grant),
        )
        .route(
            "/{id}/grants/agents/{target_agent_id}",
            delete(remove_agent_grant),
        )
        // Ownership transfer
        .route("/{id}/owner", put(transfer_owner))
}

// ── Access check helper ───────────────────────────────────────────────────────

/// All grant endpoints — reading who has access as well as adding/removing grants,
/// toggling public visibility, and transferring ownership — are management
/// operations over an agent's access-control list. Gate them on owner-or-superuser
/// (`can_manage_agent`), never on mere view access: otherwise a public agent's
/// viewer or an invoke-grantee could enumerate or rewrite its grants, or transfer
/// its ownership (RUN-9 / AUTH-1).
async fn check_access(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
) -> Result<(), axum::response::Response> {
    if !crate::acl::can_manage_agent(state, claims, agent_id).await {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    Ok(())
}

// ── GET /{id}/grants ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GrantsSummary {
    agent_id: Uuid,
    is_public: bool,
    /// UUIDs of users who have an explicit user-level share.
    user_grants: Vec<Uuid>,
    /// Agent UUIDs this agent is allowed to call (empty = unrestricted).
    agent_acl: Vec<Uuid>,
}

async fn list_grants(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let is_public: bool =
        sqlx::query_scalar("SELECT is_public FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);

    let user_grants: Vec<Uuid> = sqlx::query_scalar::<_, String>(
        "SELECT grantee_id FROM agent_grants WHERE agent_id = $1 AND grant_type = 'user'",
    )
    .bind(agent_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .filter_map(|s| s.parse().ok())
    .collect();

    let agent_acl: Vec<Uuid> =
        sqlx::query_scalar("SELECT target_agent_id FROM agent_acl WHERE caller_agent_id = $1")
            .bind(agent_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    Json(GrantsSummary {
        agent_id,
        is_public,
        user_grants,
        agent_acl,
    })
    .into_response()
}

// ── GET /{id}/visibility ─────────────────────────────────────────────────────

async fn get_visibility(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let row: Option<(bool,)> =
        sqlx::query_as("SELECT is_public FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    match row {
        Some((is_public,)) => {
            Json(serde_json::json!({ "agent_id": agent_id, "is_public": is_public }))
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ── POST /{id}/grants/public ─────────────────────────────────────────────────

async fn make_public(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let result = sqlx::query(
        "UPDATE agents SET is_public = true, updated_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            // Also mirror as a 'public' grant row so ACL queries remain consistent.
            let _ = sqlx::query(
                "INSERT INTO agent_grants (agent_id, grant_type, grantee_id)
                 VALUES ($1, 'public', '*') ON CONFLICT DO NOTHING",
            )
            .bind(agent_id)
            .execute(&state.db)
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "make_public db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── DELETE /{id}/grants/public ───────────────────────────────────────────────

async fn make_private(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let result = sqlx::query(
        "UPDATE agents SET is_public = false, updated_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            let _ = sqlx::query(
                "DELETE FROM agent_grants WHERE agent_id = $1 AND grant_type = 'public'",
            )
            .bind(agent_id)
            .execute(&state.db)
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "make_private db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── GET /{id}/grants/users ───────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
struct UserGrant {
    user_id: String,
    username: Option<String>,
}

async fn list_user_grants(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let rows: Vec<UserGrant> = sqlx::query_as(
        r#"SELECT ag.grantee_id AS user_id, u.username
           FROM agent_grants ag
           LEFT JOIN users u ON u.id = ag.grantee_id::uuid AND u.deleted_at IS NULL
           WHERE ag.agent_id = $1 AND ag.grant_type = 'user'"#,
    )
    .bind(agent_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    Json(rows).into_response()
}

// ── POST /{id}/grants/users ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AddUserGrantBody {
    user_id: Uuid,
}

async fn add_user_grant(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<AddUserGrantBody>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let user_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(body.user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !user_exists {
        return (StatusCode::NOT_FOUND, "user not found").into_response();
    }

    match sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id)
         VALUES ($1, 'user', $2::text)
         ON CONFLICT DO NOTHING",
    )
    .bind(agent_id)
    .bind(body.user_id.to_string())
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "add_user_grant db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── DELETE /{id}/grants/users/{user_id} ──────────────────────────────────────

async fn remove_user_grant(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, user_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    match sqlx::query(
        "DELETE FROM agent_grants WHERE agent_id = $1 AND grant_type = 'user' AND grantee_id = $2::text",
    )
    .bind(agent_id)
    .bind(user_id.to_string())
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, %user_id, "remove_user_grant db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── GET /{id}/grants/agents ──────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AgentGrant {
    target_agent_id: Uuid,
    target_name: Option<String>,
}

async fn list_agent_grants(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let rows: Vec<AgentGrant> = sqlx::query_as(
        r#"SELECT aa.target_agent_id, a.name AS target_name
           FROM agent_acl aa
           LEFT JOIN agents a ON a.id = aa.target_agent_id AND a.deleted_at IS NULL
           WHERE aa.caller_agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    Json(rows).into_response()
}

// ── POST /{id}/grants/agents ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AddAgentGrantBody {
    agent_id: Uuid,
}

async fn add_agent_grant(
    State(state): State<AppState>,
    claims: Claims,
    Path(caller_id): Path<Uuid>,
    Json(body): Json<AddAgentGrantBody>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, caller_id).await {
        return r;
    }

    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(body.agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !target_exists {
        return (StatusCode::NOT_FOUND, "target agent not found").into_response();
    }

    let granted_by: Option<Uuid> = claims.sub.parse().ok();

    match sqlx::query(
        "INSERT INTO agent_acl (caller_agent_id, target_agent_id, granted_by)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(caller_id)
    .bind(body.agent_id)
    .bind(granted_by)
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            tracing::error!(%e, %caller_id, "add_agent_grant db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── DELETE /{id}/grants/agents/{target_agent_id} ─────────────────────────────

async fn remove_agent_grant(
    State(state): State<AppState>,
    claims: Claims,
    Path((caller_id, target_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, caller_id).await {
        return r;
    }

    match sqlx::query("DELETE FROM agent_acl WHERE caller_agent_id = $1 AND target_agent_id = $2")
        .bind(caller_id)
        .bind(target_id)
        .execute(&state.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %caller_id, %target_id, "remove_agent_grant db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── PUT /{id}/owner ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TransferOwnerBody {
    new_owner_id: Uuid,
}

async fn transfer_owner(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<TransferOwnerBody>,
) -> impl IntoResponse {
    if let Err(r) = check_access(&state, &claims, agent_id).await {
        return r;
    }

    let new_owner_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(body.new_owner_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !new_owner_exists {
        return (StatusCode::NOT_FOUND, "new owner not found").into_response();
    }

    match sqlx::query(
        "UPDATE agents SET owner_id = $2, updated_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .bind(body.new_owner_id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "transfer_owner db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
