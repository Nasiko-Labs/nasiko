//! Generic (non-Composio) MCP server registration + auth-type probe.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use nasiko_mcp_gateway::servers::{self, NewServerInput};

use super::{ApiError, ensure_admin, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

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

/// `POST /api/mcp/servers` — register a platform (admin) or user-scoped server.
pub async fn create_server(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateServer>,
) -> Result<impl IntoResponse, ApiError> {
    // Scope: platform (admin) vs user-owned.
    let user_id = if body.is_platform {
        ensure_admin(&claims)?;
        None
    } else {
        Some(parse_user(&claims)?)
    };

    let server = servers::create_server(
        &state.mcp,
        user_id,
        NewServerInput {
            name: body.name,
            url: body.url,
            transport: body.transport,
            auth_type: body.auth_type,
            url_param_name: body.url_param_name,
            credential_header_name: body.credential_header_name,
            headers: body.headers,
            basic_username: body.basic_username,
            basic_password: body.basic_password,
            description: body.description,
            display_name: body.display_name,
            logo_url: body.logo_url,
            is_platform: body.is_platform,
            is_active: body.is_active,
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(servers::server_dto(&server))))
}

/// `GET /api/mcp/servers` — all servers visible to the caller (platform + own).
pub async fn list_servers(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(Json(servers::list_servers_view(&state.mcp, user_id).await?))
}

/// `DELETE /api/mcp/servers/{id}` — delete a platform (admin) or owned server,
/// cleaning up any per-agent permission rows that referenced it.
pub async fn delete_server(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let server = servers::get_server_for_deletion(&state.mcp, id).await?;

    // Authorization + the scope used for permission cleanup.
    let cleanup_user: Option<Uuid> = if server.is_platform {
        ensure_admin(&claims)?;
        None // platform server gone for everyone
    } else {
        let user_id = parse_user(&claims)?;
        if server.user_id != Some(user_id) {
            return Err(ApiError(nasiko_mcp_gateway::McpError::Forbidden(
                "this server does not belong to you".into(),
            )));
        }
        Some(user_id)
    };

    servers::delete_server(&state.mcp, &server, cleanup_user).await?;
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
    Ok(Json(servers::probe_server_view(&state.mcp, &body.url).await?))
}
