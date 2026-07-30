//! Owner-controlled connector sharing — path-based grant/revoke.

use axum::extract::State;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;

use super::super::{ApiError, ApiResponse, AppPath, AppQuery, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `201 Created` + `created_msg` for a genuinely new grant, `200 OK` +
/// `existing_msg` for a repeat of one that already existed — distinguishes
/// the two instead of always reporting "granted" regardless of which
/// happened. `view` is the JSON `create_share_grant` returns, which always
/// carries a `was_new` boolean.
fn grant_response(
    view: Value,
    created_msg: &'static str,
    existing_msg: &'static str,
) -> ApiResponse {
    let was_new = view.get("was_new").and_then(Value::as_bool).unwrap_or(true);
    if was_new {
        ApiResponse::created(view, created_msg)
    } else {
        ApiResponse::ok(view, existing_msg)
    }
}

/// `GET /api/mcp/connectors/{id}/grants` — list a connector's grants.
pub async fn list(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list_shares(&state, caller, claims.is_superuser, id).await?,
        "Connector grants retrieved successfully",
    ))
}

/// `POST /api/mcp/connectors/{id}/grants/public` — make connector public.
pub async fn grant_public(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let view = service::connectors::share(
        &state,
        caller,
        claims.is_superuser,
        id,
        service::connectors::ShareTarget::Public,
    )
    .await?;
    Ok(grant_response(
        view,
        "Connector made public",
        "Connector is already public",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/grants/public` — revoke public access.
pub async fn revoke_public(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::revoke(
        &state,
        caller,
        claims.is_superuser,
        id,
        service::connectors::ShareTarget::Public,
    )
    .await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Public access revoked",
    ))
}

/// `POST /api/mcp/connectors/{id}/grants/users/{user_id}` — grant to a user.
pub async fn grant_user(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((id, user_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let view = service::connectors::share(
        &state,
        caller,
        claims.is_superuser,
        id,
        service::connectors::ShareTarget::User(user_id),
    )
    .await?;
    Ok(grant_response(
        view,
        "Connector shared with user",
        "Connector is already shared with this user",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/grants/users/{user_id}` — revoke from a user.
pub async fn revoke_user(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((id, user_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::revoke(
        &state,
        caller,
        claims.is_superuser,
        id,
        service::connectors::ShareTarget::User(user_id),
    )
    .await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "User access revoked",
    ))
}

/// `POST /api/mcp/connectors/{id}/grants/agents/{agent_id}` — grant to an agent.
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
        service::connectors::grant_agent(&state, caller, claims.is_superuser, id, agent_id).await?;
    Ok(grant_response(
        view,
        "Connector granted to agent",
        "Connector is already granted to this agent",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/grants/agents/{agent_id}` — revoke from an agent.
pub async fn revoke_agent(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((id, agent_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::revoke_agent(&state, caller, claims.is_superuser, id, agent_id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Agent access revoked",
    ))
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

#[derive(Debug, Deserialize)]
pub struct ResolveShareTargetQuery {
    pub username: String,
}

/// `GET /api/mcp/share-targets/resolve?username=` — exact username → user_id,
/// for `share add`/`share remove` to target someone outside the caller's own
/// org-visibility scope (unlike `search_targets` above, this is intentionally
/// NOT scoped — see `service::connectors::resolve_share_target`'s doc comment).
pub async fn resolve_target(
    State(state): State<AppState>,
    _claims: Claims,
    AppQuery(query): AppQuery<ResolveShareTargetQuery>,
) -> Result<ApiResponse, ApiError> {
    Ok(ApiResponse::ok(
        service::connectors::resolve_share_target(&state, &query.username).await?,
        "Share target resolved successfully",
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

async fn agent_exists(state: &AppState, agent_id: Uuid) -> nasiko_mcp_gateway::Result<bool> {
    let ok = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;
    Ok(ok)
}
