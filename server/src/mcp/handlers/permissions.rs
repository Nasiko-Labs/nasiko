//! Per-agent connector access + tool rules. Gated by `ensure_can_manage_agent`.

use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::super::{ApiError, ApiResponse, AppJson, ensure_can_manage_agent, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/agents/{agent_id}/connectors` — connectors + per-agent status.
pub async fn list_connectors(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::permissions::list_connectors(&state, user_id, agent_id).await?,
        "Agent connectors retrieved successfully",
    ))
}

#[derive(Debug, Deserialize)]
pub struct SetConnectorAccess {
    pub enabled: bool,
}

/// `PUT /api/mcp/agents/{agent_id}/connectors/{connector_id}` — toggle a connector.
pub async fn set_connector_access(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, connector_id)): Path<(Uuid, Uuid)>,
    AppJson(body): AppJson<SetConnectorAccess>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::permissions::set_connector_access(
            &state, user_id, agent_id, connector_id, body.enabled,
        )
        .await?,
        "Connector access updated successfully",
    ))
}

/// `GET /api/mcp/agents/{agent_id}/connectors/{connector_id}/tools` — tools + stances.
pub async fn list_connector_tools(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, connector_id)): Path<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::permissions::list_connector_tools(&state, user_id, agent_id, connector_id)
            .await?,
        "Connector tools retrieved successfully",
    ))
}

/// `GET /api/mcp/agents/{agent_id}/tools` — the agent's current tool rules.
pub async fn list_tool_rules(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    Ok(ApiResponse::ok(
        service::permissions::list_tool_rules(&state, agent_id).await?,
        "Tool rules retrieved successfully",
    ))
}

#[derive(Debug, Deserialize)]
pub struct ToolRule {
    pub connector_id: Uuid,
    pub tool_pattern: String,
    pub stance: String,
}

#[derive(Debug, Deserialize)]
pub struct BulkToolUpdate {
    pub rules: Vec<ToolRule>,
}

/// `PUT /api/mcp/agents/{agent_id}/tools` — batch upsert tool rules.
pub async fn bulk_update_tools(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    AppJson(body): AppJson<BulkToolUpdate>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    let rules: Vec<service::permissions::ToolRuleInput> = body
        .rules
        .into_iter()
        .map(|r| service::permissions::ToolRuleInput {
            connector_id: r.connector_id,
            tool_pattern: r.tool_pattern,
            stance: r.stance,
        })
        .collect();
    Ok(ApiResponse::ok(
        service::permissions::bulk_update_tools(&state, user_id, agent_id, &rules).await?,
        "Tool rules updated successfully",
    ))
}

/// `DELETE /api/mcp/agents/{agent_id}/permissions` — reset to all-allowed.
pub async fn reset(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    tracing::info!(%agent_id, caller = %claims.sub, "resetting agent tool permissions");
    let deleted = service::permissions::reset(&state, agent_id).await?;
    Ok(ApiResponse::ok(
        json!({ "rows_deleted": deleted }),
        "Agent permissions reset successfully",
    ))
}
