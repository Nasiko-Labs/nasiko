//! Per-agent permission management — the Claude-Desktop-style connector UI
//! backend. All routes are gated by `ensure_can_manage_agent`.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::permissions as perm_engine;
use nasiko_mcp_gateway::provider::generic::LIST_TIMEOUT;
use nasiko_mcp_gateway::{McpError, credentials, repo};

use super::{ApiError, capitalize, ensure_can_manage_agent, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

const STANCES: [&str; 3] = ["allow", "ask", "block"];

/// `GET /api/mcp/agents/{agent_id}/servers` — all servers visible to the user
/// with this agent's enabled/connected status.
pub async fn list_servers(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;

    let access = repo::get_agent_server_access(&state.mcp.db, user_id, agent_id).await?;
    let access_map: HashMap<String, bool> =
        access.into_iter().map(|r| (r.server_name, r.enabled)).collect();

    let active_conns = repo::list_connections_by_user(&state.mcp.db, user_id, Some("ACTIVE")).await?;
    let connected_toolkits: std::collections::HashSet<String> =
        active_conns.into_iter().map(|c| c.toolkit).collect();

    let creds = repo::get_user_credentials_for_user(&state.mcp.db, user_id).await?;
    let tokens = repo::get_mcp_oauth_tokens_for_user(&state.mcp.db, user_id).await?;
    let cred_ids: std::collections::HashSet<Uuid> = creds.into_iter().map(|c| c.mcp_server_id).collect();
    let token_ids: std::collections::HashSet<Uuid> = tokens.into_iter().map(|t| t.mcp_server_id).collect();

    let mut entries: Vec<Value> = Vec::new();

    for ac in repo::list_platform_auth_configs(&state.mcp.db).await? {
        entries.push(json!({
            "server_name": ac.toolkit,
            "server_type": "composio",
            "enabled": access_map.get(&ac.toolkit).copied().unwrap_or(true),
            "connected": connected_toolkits.contains(&ac.toolkit),
            "display_name": ac.display_name.unwrap_or_else(|| capitalize(&ac.toolkit)),
            "logo_url": ac.logo_url,
        }));
    }
    for s in repo::list_mcp_servers_for_user(&state.mcp.db, user_id).await? {
        let connected = cred_ids.contains(&s.id) || token_ids.contains(&s.id) || s.auth_type == "none";
        entries.push(json!({
            "server_name": s.name,
            "server_type": "mcp",
            "enabled": access_map.get(&s.name).copied().unwrap_or(true),
            "connected": connected,
            "display_name": s.display_name.unwrap_or_else(|| capitalize(&s.name)),
            "logo_url": s.logo_url,
        }));
    }

    Ok(Json(json!({ "data": entries })))
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

    // Determine the server type (and that it exists).
    let server_type = if repo::get_platform_auth_config_by_toolkit(&state.mcp.db, &server).await?.is_some() {
        "composio"
    } else if repo::get_platform_mcp_server_by_name(&state.mcp.db, &server).await?.is_some()
        || repo::get_user_mcp_server_by_name(&state.mcp.db, user_id, &server).await?.is_some()
    {
        "mcp"
    } else {
        return Err(ApiError(McpError::NotFound(format!("server '{server}' not found"))));
    };

    let row = repo::upsert_agent_server_access(
        &state.mcp.db,
        user_id,
        agent_id,
        &server,
        server_type,
        body.enabled,
    )
    .await?;
    perm_engine::invalidate_permission_cache(&state.mcp, user_id, agent_id).await;

    Ok(Json(json!({
        "server_name": row.server_name,
        "server_type": row.server_type,
        "enabled": row.enabled,
    })))
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
    let perms = perm_engine::load_permission_context(&state.mcp, user_id, agent_id).await?;

    // Collect (name, description) pairs from the right source.
    let tools: Vec<(String, Option<String>)> =
        if repo::get_platform_auth_config_by_toolkit(&state.mcp.db, &server).await?.is_some() {
            match &state.mcp.providers.composio {
                Some(provider) => provider
                    .list_toolkit_tools(&server)
                    .await?
                    .into_iter()
                    .map(|t| (t.name, t.description))
                    .collect(),
                None => Vec::new(),
            }
        } else {
            // Generic MCP server: build its config (with the user's creds) and probe it.
            let built = credentials::build_generic_servers(&state.mcp, user_id).await?;
            match built.iter().find(|s| s.name == server) {
                Some(cfg) => state
                    .mcp
                    .providers
                    .mcp
                    .list_tools(cfg, LIST_TIMEOUT, None)
                    .await?
                    .into_iter()
                    .filter_map(|t| {
                        t.get("name").and_then(|n| n.as_str()).map(|name| {
                            (
                                name.to_string(),
                                t.get("description").and_then(|d| d.as_str()).map(str::to_string),
                            )
                        })
                    })
                    .collect(),
                None => Vec::new(),
            }
        };

    let out: Vec<Value> = tools
        .into_iter()
        .map(|(name, description)| {
            let stance = perms.get_stance(&server, &name);
            json!({ "name": name, "description": description, "stance": stance.as_str() })
        })
        .collect();

    Ok(Json(json!({ "data": out })))
}

/// `GET /api/mcp/agents/{agent_id}/tools` — the agent's current tool rules.
pub async fn list_tool_rules(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    let rows = repo::get_agent_tool_permissions(&state.mcp.db, user_id, agent_id).await?;
    let data: Vec<Value> = rows
        .into_iter()
        .map(|r| json!({ "server_name": r.server_name, "tool_pattern": r.tool_pattern, "stance": r.stance }))
        .collect();
    Ok(Json(json!({ "data": data })))
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

    for rule in &body.rules {
        if !STANCES.contains(&rule.stance.as_str()) {
            return Err(ApiError(McpError::BadRequest(format!(
                "stance must be one of {STANCES:?}"
            ))));
        }
    }

    let mut applied = Vec::with_capacity(body.rules.len());
    for rule in &body.rules {
        let row = repo::upsert_agent_tool_permission(
            &state.mcp.db,
            user_id,
            agent_id,
            &rule.server_name,
            &rule.tool_pattern,
            &rule.stance,
        )
        .await?;
        applied.push(json!({
            "server_name": row.server_name,
            "tool_pattern": row.tool_pattern,
            "stance": row.stance,
        }));
    }
    perm_engine::invalidate_permission_cache(&state.mcp, user_id, agent_id).await;

    Ok(Json(json!({ "data": applied })))
}

/// `DELETE /api/mcp/agents/{agent_id}/permissions` — reset to all-allowed.
pub async fn reset(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    let user_id = parse_user(&claims)?;
    let deleted = repo::delete_all_agent_permissions(&state.mcp.db, user_id, agent_id).await?;
    perm_engine::invalidate_permission_cache(&state.mcp, user_id, agent_id).await;
    Ok((StatusCode::OK, Json(json!({ "rows_deleted": deleted }))))
}
