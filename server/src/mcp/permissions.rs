//! Per-agent permission management — the Claude-Desktop-style connector UI
//! backend. All routes are gated by `ensure_can_manage_agent`.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::permissions::{self as perm_engine, ToolRuleInput};

use super::{ApiError, ensure_can_manage_agent, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/agents/{agent_id}/servers` — all servers visible to the user
/// with this agent's enabled/connected status.
pub async fn list_servers(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(perm_engine::list_servers_view(&state.mcp, user_id, agent_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct SetServerAccess {
    pub enabled: bool,
}

/// `PUT /api/mcp/agents/{agent_id}/servers/{server}` — toggle a server for the agent.
pub async fn set_server_access(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, server)): Path<(Uuid, String)>,
    Json(body): Json<SetServerAccess>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(
        perm_engine::set_server_access_view(&state.mcp, user_id, agent_id, &server, body.enabled).await?,
    ))
}

/// `GET /api/mcp/agents/{agent_id}/servers/{server}/tools` — tools for a server
/// with this agent's current stance per tool.
pub async fn list_server_tools(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, server)): Path<(Uuid, String)>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(perm_engine::list_server_tools_view(&state.mcp, user_id, agent_id, &server).await?))
}

/// `GET /api/mcp/agents/{agent_id}/tools` — the agent's current tool rules.
pub async fn list_tool_rules(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(Json(perm_engine::list_tool_rules_view(&state.mcp, user_id, agent_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct ToolRule {
    pub server_name: String,
    pub tool_pattern: String,
    pub stance: String,
}

#[derive(Debug, Deserialize)]
pub struct BulkToolUpdate {
    pub rules: Vec<ToolRule>,
}

/// `PUT /api/mcp/agents/{agent_id}/tools` — batch upsert tool permission rules.
pub async fn bulk_update_tools(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<BulkToolUpdate>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;

    let rules: Vec<ToolRuleInput> = body
        .rules
        .into_iter()
        .map(|r| ToolRuleInput { server_name: r.server_name, tool_pattern: r.tool_pattern, stance: r.stance })
        .collect();

    Ok(Json(perm_engine::bulk_update_tools(&state.mcp, user_id, agent_id, &rules).await?))
}

/// `DELETE /api/mcp/agents/{agent_id}/permissions` — reset to all-allowed.
pub async fn reset(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    let deleted = perm_engine::reset(&state.mcp, user_id, agent_id).await?;
    Ok((StatusCode::OK, Json(json!({ "rows_deleted": deleted }))))
}
