//! Generic (non-Composio) MCP server registration + auth-type probe.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::permissions;
use nasiko_mcp_gateway::repo::{self, McpServer, NewMcpServer};
use nasiko_mcp_gateway::{McpError, repo::get_platform_auth_config_by_toolkit};

use super::{ApiError, ensure_admin, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

const AUTH_TYPES: [&str; 5] = ["none", "bearer", "basic", "oauth2", "url_param"];

#[derive(Debug, Deserialize)]
pub struct CreateServer {
    pub name: String,
    pub url: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub basic_username: Option<String>,
    pub basic_password: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    #[serde(default)]
    pub is_platform: bool,
    #[serde(default = "default_true")]
    pub is_active: bool,
}

fn default_transport() -> String {
    "streamable_http".to_string()
}
fn default_auth_type() -> String {
    "none".to_string()
}
fn default_true() -> bool {
    true
}

fn server_dto(s: &McpServer) -> Value {
    json!({
        "id": s.id,
        "name": s.name,
        "url": s.url,
        "transport": s.transport,
        "auth_type": s.auth_type,
        "url_param_name": s.url_param_name,
        "credential_header_name": s.credential_header_name,
        "description": s.description,
        "display_name": s.display_name,
        "logo_url": s.logo_url,
        "is_platform": s.is_platform,
        "is_active": s.is_active,
        "oauth_configured": s.oauth_configured(),
        "created_at": s.created_at,
        "updated_at": s.updated_at,
    })
}

/// `POST /api/mcp/servers` — register a platform (admin) or user-scoped server.
pub async fn create_server(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateServer>,
) -> Result<impl IntoResponse, ApiError> {
    if !AUTH_TYPES.contains(&body.auth_type.as_str()) {
        return Err(ApiError(McpError::BadRequest(format!(
            "auth_type must be one of {AUTH_TYPES:?}"
        ))));
    }
    if body.auth_type == "url_param" && body.url_param_name.is_none() {
        return Err(ApiError(McpError::BadRequest(
            "url_param_name is required when auth_type='url_param'".into(),
        )));
    }
    // SSRF guard: reject URLs resolving to private/internal addresses.
    nasiko_mcp_gateway::net::validate_public_url(&body.url).await?;

    // Scope: platform (admin) vs user-owned.
    let user_id = if body.is_platform {
        ensure_admin(&claims)?;
        None
    } else {
        Some(parse_user(&claims)?)
    };

    // Duplicate + name-collision guards (mirror the PoC).
    let existing = if body.is_platform {
        repo::get_platform_mcp_server_by_name(&state.mcp.db, &body.name).await?
    } else {
        repo::get_user_mcp_server_by_name(&state.mcp.db, user_id.unwrap(), &body.name).await?
    };
    if existing.is_some() {
        return Err(ApiError(McpError::Conflict(format!(
            "MCP server '{}' already exists in this scope",
            body.name
        ))));
    }
    if get_platform_auth_config_by_toolkit(&state.mcp.db, &body.name).await?.is_some() {
        return Err(ApiError(McpError::Conflict(format!(
            "'{}' is already a Composio toolkit — choose a different MCP server name",
            body.name
        ))));
    }

    // For user-scoped basic auth, precompute the Authorization: Basic header.
    let mut headers = body.headers.clone().unwrap_or_default();
    if body.auth_type == "basic"
        && !body.is_platform
        && let (Some(u), Some(p)) = (&body.basic_username, &body.basic_password)
    {
        let encoded = B64.encode(format!("{u}:{p}"));
        headers.insert("Authorization".to_string(), format!("Basic {encoded}"));
    }
    let headers_json = if headers.is_empty() {
        None
    } else {
        Some(serde_json::to_value(&headers).unwrap_or(Value::Null))
    };

    let new = NewMcpServer {
        name: body.name.clone(),
        url: body.url.clone(),
        transport: body.transport.clone(),
        auth_type: body.auth_type.clone(),
        url_param_name: body.url_param_name.clone(),
        credential_header_name: body.credential_header_name.clone(),
        headers: headers_json,
        description: body.description.clone(),
        display_name: body.display_name.clone(),
        logo_url: body.logo_url.clone(),
        is_platform: body.is_platform,
        user_id,
        is_active: body.is_active,
    };

    let server = repo::create_mcp_server(&state.mcp.db, &new).await?;
    tracing::info!(name = %server.name, auth_type = %server.auth_type, is_platform = server.is_platform, "registered mcp server");
    Ok((StatusCode::CREATED, Json(server_dto(&server))))
}

/// `GET /api/mcp/servers` — all servers visible to the caller (platform + own).
pub async fn list_servers(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let servers = repo::list_mcp_servers_for_user(&state.mcp.db, user_id).await?;
    let data: Vec<Value> = servers.iter().map(server_dto).collect();
    let total = data.len();
    Ok(Json(json!({ "data": data, "total": total })))
}

/// `DELETE /api/mcp/servers/{id}` — delete a platform (admin) or owned server,
/// cleaning up any per-agent permission rows that referenced it.
pub async fn delete_server(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let server = repo::get_mcp_server_by_id(&state.mcp.db, id)
        .await?
        .ok_or_else(|| ApiError(McpError::NotFound(format!("MCP server '{id}' not found"))))?;

    // Authorization + the scope used for permission cleanup.
    let cleanup_user: Option<Uuid> = if server.is_platform {
        ensure_admin(&claims)?;
        None // platform server gone for everyone
    } else {
        let user_id = parse_user(&claims)?;
        if server.user_id != Some(user_id) {
            return Err(ApiError(McpError::Forbidden("this server does not belong to you".into())));
        }
        Some(user_id)
    };

    // Snapshot affected (user, agent) pairs, delete the server + its permission
    // rows, then invalidate the permission cache for each pair.
    let pairs = repo::get_agent_pairs_for_server(&state.mcp.db, &server.name, cleanup_user).await?;
    repo::delete_mcp_server(&state.mcp.db, id).await?;
    repo::delete_agent_permissions_for_server(&state.mcp.db, &server.name, cleanup_user).await?;
    for (uid, aid) in pairs {
        permissions::invalidate_permission_cache(&state.mcp, uid, aid).await;
    }

    tracing::info!(name = %server.name, "deleted mcp server");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    pub url: String,
}

/// `POST /api/mcp/servers/probe` — detect a server's auth type without storing
/// anything. Returns `{ url, auth_type, requires, hint }`.
pub async fn probe_server(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<ProbeRequest>,
) -> Result<Json<Value>, ApiError> {
    let url = body.url.trim_end_matches('/').to_string();
    // SSRF guard before making a server-side request to a user-supplied URL.
    nasiko_mcp_gateway::net::validate_public_url(&url).await?;
    let probe = state
        .mcp
        .http_client
        .post(&url)
        .timeout(std::time::Duration::from_secs(10))
        .header("Content-Type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                           "clientInfo": {"name": "mcp-gateway-probe", "version": "1.0"}},
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(|e| McpError::Backend(format!("could not reach MCP server: {e}")))?;

    let status = probe.status();
    if status.is_success() {
        return Ok(Json(json!({
            "url": url, "auth_type": "none", "requires": "nothing",
            "hint": "This server requires no authentication.",
        })));
    }

    if status == reqwest::StatusCode::UNAUTHORIZED {
        let www_auth = probe
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if www_auth.contains("resource_metadata") {
            return Ok(Json(json!({
                "url": url, "auth_type": "oauth2", "requires": "oauth_flow",
                "hint": "This server uses OAuth 2.1. You will be redirected to authorize.",
            })));
        }
        return Ok(Json(json!({
            "url": url, "auth_type": "bearer", "requires": "api_key_input",
            "hint": "This server requires a Bearer token or API key.",
        })));
    }

    Ok(Json(json!({
        "url": url, "auth_type": "bearer", "requires": "api_key_input",
        "hint": format!("Server returned HTTP {}. It may require an API key.", status.as_u16()),
    })))
}
