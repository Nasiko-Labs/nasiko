//! Owner-controlled connector sharing.

use axum::extract::State;
use serde::Deserialize;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;

use super::super::{ApiError, ApiResponse, AppJson, AppPath, AppQuery, parse_user, service};
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
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list_shares(&state, caller, claims.is_superuser, id).await?,
        "Connector shares retrieved successfully",
    ))
}

/// `POST /api/mcp/connectors/{id}/share` — share by username or with everyone.
pub async fn share(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
    AppJson(body): AppJson<ShareRequest>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let target = body.into_target()?;
    let view =
        service::connectors::share(&state, caller, claims.is_superuser, id, target).await?;
    Ok(ApiResponse::created(view, "Connector shared successfully"))
}

/// `DELETE /api/mcp/connectors/{id}/share` — revoke a share.
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
    AppJson(body): AppJson<ShareRequest>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let target = body.into_target()?;
    service::connectors::revoke(&state, caller, claims.is_superuser, id, target).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Connector share revoked successfully"))
}

#[derive(Debug, Deserialize)]
pub struct SearchShareTargetsQuery {
    pub q: String,
}

/// `GET /api/mcp/share-targets?q=` — search users to share a connector with.
pub async fn search_targets(
    State(state): State<AppState>,
    claims: Claims,
    AppQuery(query): AppQuery<SearchShareTargetsQuery>,
) -> Result<ApiResponse, ApiError> {
    Ok(ApiResponse::ok(
        service::connectors::search_share_targets(&state, claims, &query.q).await?,
        "Share targets retrieved successfully",
    ))
}

/// `GET /api/mcp/connectors/{id}/consumers` — agents that have this connector configured.
pub async fn consumers(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list_consumers(&state, caller, claims.is_superuser, id).await?,
        "Connector consumers retrieved successfully",
    ))
}

/// `POST /api/mcp/connectors/{id}/grants/agents/{agent_id}` — share with a specific agent.
pub async fn grant_agent(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((id, agent_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    if !agent_exists(&state, agent_id).await? {
        return Err(ApiError(McpError::NotFound(format!(
            "agent '{agent_id}' not found"
        ))));
    }
    let view =
        service::connectors::grant_agent(&state, caller, claims.is_superuser, id, agent_id)
            .await?;
    Ok(ApiResponse::created(view, "Connector granted to agent successfully"))
}

/// `DELETE /api/mcp/connectors/{id}/grants/agents/{agent_id}` — revoke agent grant.
pub async fn revoke_agent(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((id, agent_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::revoke_agent(&state, caller, claims.is_superuser, id, agent_id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Agent connector grant revoked successfully"))
}

async fn agent_exists(state: &AppState, agent_id: Uuid) -> nasiko_mcp_gateway::Result<bool> {
    let ok = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;
    Ok(ok)
}
