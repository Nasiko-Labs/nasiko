//! Per-agent connector access + tool rules. Gated by `ensure_can_manage_agent`.

use axum::extract::State;
use nasiko_mcp_gateway::McpError;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::super::{
    ApiError, ApiResponse, AppJson, AppPath, ensure_can_manage_agent,
    ensure_can_manage_agent_connector, parse_user, service,
};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/agents/{agent_id}/connectors` — connectors + per-agent status.
///
/// Same relaxed gate as `list_tool_rules` below (and `set_connector_access`/
/// `list_connector_tools`, already relaxed by `a9012ded`/`56e46b07`): a caller
/// who can't manage the whole agent still sees the connector(s) they
/// themselves can reach and that have already been granted to this agent
/// (narrowed, not blocked outright) — otherwise this endpoint (the natural
/// first call before `agent-tools enable`/`set-rule`) would be the one
/// sibling still hard-denying the exact caller those other endpoints already
/// permit, making them undiscoverable in practice.
pub async fn list_connectors(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(agent_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let mut data = service::permissions::list_connectors(&state, user_id, agent_id).await?;
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        let mut allowed: std::collections::HashMap<Uuid, bool> = std::collections::HashMap::new();
        if let Some(connectors) = data.get_mut("connectors").and_then(|v| v.as_array_mut()) {
            let mut kept = Vec::with_capacity(connectors.len());
            for c in connectors.drain(..) {
                let Some(connector_id) = c
                    .get("connector_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok())
                else {
                    continue;
                };
                let ok = match allowed.get(&connector_id) {
                    Some(v) => *v,
                    None => {
                        let v = crate::acl::can_manage_agent_connector(
                            &state,
                            &claims,
                            agent_id,
                            connector_id,
                        )
                        .await;
                        allowed.insert(connector_id, v);
                        v
                    }
                };
                if ok {
                    kept.push(c);
                }
            }
            *connectors = kept;
        }
        if allowed.is_empty() || allowed.values().all(|v| !v) {
            return Err(ApiError(McpError::Forbidden(
                "you do not have permission to manage this agent".into(),
            )));
        }
    }
    Ok(ApiResponse::ok(data, "Agent connectors retrieved successfully"))
}

#[derive(Debug, Deserialize)]
pub struct SetConnectorAccess {
    pub enabled: bool,
}

/// `PUT /api/mcp/agents/{agent_id}/connectors/{connector_id}` — toggle a connector.
///
/// Allows either full agent management, or the connector's own owner acting on
/// their connector once it's been granted to this agent (see
/// `ensure_can_manage_agent_connector`) — a connector owner can enable/disable
/// their own connector on someone else's agent without managing the agent itself.
pub async fn set_connector_access(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((agent_id, connector_id)): AppPath<(Uuid, Uuid)>,
    AppJson(body): AppJson<SetConnectorAccess>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent_connector(&state, &claims, agent_id, connector_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::permissions::set_connector_access(
            &state,
            user_id,
            agent_id,
            connector_id,
            body.enabled,
        )
        .await?,
        "Connector access updated successfully",
    ))
}

/// `GET /api/mcp/agents/{agent_id}/connectors/{connector_id}/tools` — tools + stances.
///
/// Same relaxed gate as `set_connector_access`: the connector's own owner can
/// view its tools/stances on this agent once it's been granted here.
pub async fn list_connector_tools(
    State(state): State<AppState>,
    claims: Claims,
    AppPath((agent_id, connector_id)): AppPath<(Uuid, Uuid)>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent_connector(&state, &claims, agent_id, connector_id).await?;
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::permissions::list_connector_tools(&state, user_id, agent_id, connector_id).await?,
        "Connector tools retrieved successfully",
    ))
}

/// `GET /api/mcp/agents/{agent_id}/tools` — the agent's current tool rules.
///
/// Agent-wide (every connector's rules at once), so a non-agent-manager never
/// gets the unfiltered view: if they can't manage the whole agent, the result
/// is narrowed to only the connector(s) they themselves own and that have
/// already been granted to this agent (`ensure_can_manage_agent_connector`),
/// e.g. so `agent-tools set-rule`'s read-modify-write can find that connector's
/// existing rules. A caller with no such connector still gets the original
/// 403 — this never turns into "any authenticated user can query any agent".
pub async fn list_tool_rules(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(agent_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let mut data = service::permissions::list_tool_rules(&state, agent_id).await?;
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        let mut allowed: std::collections::HashMap<Uuid, bool> = std::collections::HashMap::new();
        let mut saw_any_connector = false;
        if let Some(rules) = data.get_mut("rules").and_then(|v| v.as_array_mut()) {
            let mut kept = Vec::with_capacity(rules.len());
            for rule in rules.drain(..) {
                let Some(connector_id) = rule
                    .get("connector_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok())
                else {
                    continue;
                };
                saw_any_connector = true;
                let ok = match allowed.get(&connector_id) {
                    Some(v) => *v,
                    None => {
                        let v = crate::acl::can_manage_agent_connector(
                            &state,
                            &claims,
                            agent_id,
                            connector_id,
                        )
                        .await;
                        allowed.insert(connector_id, v);
                        v
                    }
                };
                if ok {
                    kept.push(rule);
                }
            }
            *rules = kept;
        }
        // No rules exist yet at all (e.g. a freshly granted, never-configured
        // connector) — fall back to an explicit ownership check so a caller
        // with zero relationship to this agent still gets 403, not an
        // indistinguishable empty 200.
        if !saw_any_connector {
            let user_id = parse_user(&claims)?;
            let owns_a_granted_connector =
                nasiko_mcp_gateway::repo::list_agent_granted_connectors(&state.db, agent_id)
                    .await
                    .map(|connectors| connectors.iter().any(|c| c.owner_id == Some(user_id)))
                    .unwrap_or(false);
            if !owns_a_granted_connector {
                return Err(ApiError(McpError::Forbidden(
                    "you do not have permission to manage this agent".into(),
                )));
            }
        } else if allowed.values().all(|v| !v) {
            return Err(ApiError(McpError::Forbidden(
                "you do not have permission to manage this agent".into(),
            )));
        }
    }
    Ok(ApiResponse::ok(data, "Tool rules retrieved successfully"))
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
///
/// Each rule names its own `connector_id`, so the gate is per-connector: full
/// agent management, or (for each distinct connector referenced) that
/// connector's own owner acting on a connector already granted to this agent —
/// same relaxed rule as `set_connector_access`. An empty batch still requires
/// full agent management, since there's no connector to scope a narrower
/// check to.
pub async fn bulk_update_tools(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(agent_id): AppPath<Uuid>,
    AppJson(body): AppJson<BulkToolUpdate>,
) -> Result<ApiResponse, ApiError> {
    let connector_ids: std::collections::HashSet<Uuid> =
        body.rules.iter().map(|r| r.connector_id).collect();
    if connector_ids.is_empty() {
        ensure_can_manage_agent(&state, &claims, agent_id).await?;
    } else {
        for connector_id in connector_ids {
            ensure_can_manage_agent_connector(&state, &claims, agent_id, connector_id).await?;
        }
    }
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
    AppPath(agent_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_can_manage_agent(&state, &claims, agent_id).await?;
    tracing::info!(%agent_id, caller = %claims.sub, "resetting agent tool permissions");
    let deleted = service::permissions::reset(&state, agent_id).await?;
    Ok(ApiResponse::ok(
        json!({ "rows_deleted": deleted }),
        "Agent permissions reset successfully",
    ))
}
