//! Per-agent connector access + tool rules. Gated by `ensure_can_manage_agent`.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{ApiError, ensure_can_manage_agent, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/agents/{agent_id}/connectors` — connectors + per-agent status.
pub async fn list_connectors(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(service::permissions::list_connectors(&state, user_id, agent_id).await?))
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
    Json(body): Json<SetConnectorAccess>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(service::permissions::set_connector_access(&state, user_id, agent_id, connector_id, body.enabled).await?))
}

/// `GET /api/mcp/agents/{agent_id}/connectors/{connector_id}/tools` — tools + stances.
pub async fn list_connector_tools(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, connector_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(service::permissions::list_connector_tools(&state, user_id, agent_id, connector_id).await?))
}

/// `GET /api/mcp/agents/{agent_id}/tools` — the agent's current tool rules.
pub async fn list_tool_rules(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(service::permissions::list_tool_rules(&state, user_id, agent_id).await?))
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
    Json(body): Json<BulkToolUpdate>,
) -> Result<Json<Value>, ApiError> {
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
    Ok(Json(service::permissions::bulk_update_tools(&state, user_id, agent_id, &rules).await?))
}

/// `DELETE /api/mcp/agents/{agent_id}/permissions` — reset to all-allowed.
pub async fn reset(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    let deleted = service::permissions::reset(&state, user_id, agent_id).await?;
    Ok((StatusCode::OK, Json(json!({ "rows_deleted": deleted }))))
}
