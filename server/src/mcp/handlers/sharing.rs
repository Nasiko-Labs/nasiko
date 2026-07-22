//! Owner-controlled connector sharing.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;

use super::super::{ApiError, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    /// Grant to a specific username. Omit + set `public=true` to share with everyone.
    pub username: Option<String>,
    #[serde(default)]
    pub public: bool,
}

impl ShareRequest {
    fn into_target(self) -> Result<service::connectors::ShareTarget, ApiError> {
        use service::connectors::ShareTarget;
        if self.public {
            return Ok(ShareTarget::Public);
        }
        match self.username {
            Some(u) if !u.is_empty() => Ok(ShareTarget::User(u)),
            _ => Err(ApiError(McpError::BadRequest(
                "provide 'username' or set 'public': true".into(),
            ))),
        }
    }
}

/// `GET /api/mcp/connectors/{id}/share` — list a connector's grants.
pub async fn list(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(Json(
        service::connectors::list_shares(&state, caller, claims.is_superuser, id).await?,
    ))
}

/// `POST /api/mcp/connectors/{id}/share` — share by username or with everyone.
pub async fn share(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<ShareRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let target = body.into_target()?;
    let view = service::connectors::share(&state, caller, claims.is_superuser, id, target).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `DELETE /api/mcp/connectors/{id}/share` — revoke a share.
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<ShareRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let target = body.into_target()?;
    service::connectors::revoke(&state, caller, claims.is_superuser, id, target).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct SearchShareTargetsQuery {
    pub q: String,
}

/// `GET /api/mcp/share-targets?q=` — search users to share a connector with.
/// Open to any authenticated user, not admin-gated (any owner may need this);
/// EE scopes the result to the caller's org visibility (see the service layer).
pub async fn search_targets(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<SearchShareTargetsQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        service::connectors::search_share_targets(&state, claims, &query.q).await?,
    ))
}

/// `GET /api/mcp/connectors/{id}/consumers` — agents that have this connector
/// configured. Owner/admin-gated management view.
pub async fn consumers(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(Json(
        service::connectors::list_consumers(&state, caller, claims.is_superuser, id).await?,
    ))
}

/// `POST /api/mcp/connectors/{id}/grants/agents/{agent_id}` — share a
/// connector directly with a specific agent, independent of who owns it. Lets
/// that agent be configured with the connector (`PUT
/// /api/mcp/agents/{agent_id}/connectors/{connector_id}`) even if its owner has
/// no personal reachability to it otherwise. Owner/admin-gated, same as
/// user/public shares (enforced inside `service::connectors::grant_agent`).
pub async fn grant_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path((id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = parse_user(&claims)?;
    if !agent_exists(&state, agent_id).await? {
        return Err(ApiError(McpError::NotFound(format!(
            "agent '{agent_id}' not found"
        ))));
    }
    let view =
        service::connectors::grant_agent(&state, caller, claims.is_superuser, id, agent_id).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `DELETE /api/mcp/connectors/{id}/grants/agents/{agent_id}` — revoke.
pub async fn revoke_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path((id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::revoke_agent(&state, caller, claims.is_superuser, id, agent_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `create_share_grant` stores `grantee_id` as a free-form string with no FK
/// into `agents` — validate existence here so a mistyped/nonexistent agent id
/// doesn't silently create a grant nothing can ever match (mirrors EE's
/// `team_exists`/`department_exists` in `ee/server/src/mcp_sharing.rs`).
async fn agent_exists(state: &AppState, agent_id: Uuid) -> nasiko_mcp_gateway::Result<bool> {
    let ok = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;
    Ok(ok)
}
