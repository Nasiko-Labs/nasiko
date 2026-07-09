//! Generic (non-Composio) MCP server registration + auth-type probe — pure
//! logic behind the server's `/api/mcp/servers` routes.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{McpError, Result};
use crate::permissions;
use crate::repo::{self, McpServer, NewMcpServer};
use crate::state::McpState;

pub const AUTH_TYPES: [&str; 5] = ["none", "bearer", "basic", "oauth2", "url_param"];

pub fn server_dto(s: &McpServer) -> Value {
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

/// The auth shapes [`probe_initialize`] can detect from an MCP server's
/// `initialize` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedAuthType {
    None,
    OAuth2,
    Bearer,
}

impl DetectedAuthType {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectedAuthType::None => "none",
            DetectedAuthType::OAuth2 => "oauth2",
            DetectedAuthType::Bearer => "bearer",
        }
    }
}

/// Classify a response status + `WWW-Authenticate` header value into an auth
/// type. Pure (no I/O) so it's unit-testable independent of the network call
/// in [`probe_initialize`].
fn classify_response(status: StatusCode, www_authenticate: &str) -> DetectedAuthType {
    if status.is_success() {
        return DetectedAuthType::None;
    }
    if status == StatusCode::UNAUTHORIZED {
        return if www_authenticate.contains("resource_metadata") {
            DetectedAuthType::OAuth2
        } else {
            DetectedAuthType::Bearer
        };
    }
    // Any other non-2xx response: assume it wants some kind of key/token.
    DetectedAuthType::Bearer
}

/// POST a bare `initialize` request at `url` and classify the response into an
/// auth type + the raw status code (for hint text). Shared by the
/// `/mcp/servers/probe` endpoint and the auto-register path in
/// `connect::generic_connect` — previously duplicated (and subtly inconsistent
/// on how a non-2xx/non-401 response was classified) between the two call
/// sites.
pub async fn probe_initialize(
    http_client: &reqwest::Client,
    url: &str,
) -> std::result::Result<(DetectedAuthType, u16), reqwest::Error> {
    let resp = http_client
        .post(url)
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
        .await?;

    let status = resp.status();
    let www = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    Ok((classify_response(status, &www), status.as_u16()))
}

/// `POST /mcp/servers/probe` view: detect a server's auth type without storing
/// anything. Returns `{ url, auth_type, requires, hint }`. Propagates network
/// errors — this is an explicit user-triggered probe, unlike the best-effort
/// detection during auto-registration in `connect::generic_connect`.
pub async fn probe_server_view(state: &McpState, url: &str) -> Result<Value> {
    let url = url.trim_end_matches('/').to_string();
    // SSRF guard before making a server-side request to a user-supplied URL.
    crate::net::validate_public_url(&url).await?;

    let (detected, status) = probe_initialize(&state.http_client, &url)
        .await
        .map_err(|e| McpError::Backend(format!("could not reach MCP server: {e}")))?;

    let (requires, hint) = match detected {
        DetectedAuthType::None => ("nothing", "This server requires no authentication.".to_string()),
        DetectedAuthType::OAuth2 => {
            ("oauth_flow", "This server uses OAuth 2.1. You will be redirected to authorize.".to_string())
        }
        DetectedAuthType::Bearer if status == StatusCode::UNAUTHORIZED.as_u16() => {
            ("api_key_input", "This server requires a Bearer token or API key.".to_string())
        }
        DetectedAuthType::Bearer => {
            ("api_key_input", format!("Server returned HTTP {status}. It may require an API key."))
        }
    };

    Ok(json!({ "url": url, "auth_type": detected.as_str(), "requires": requires, "hint": hint }))
}

/// Inputs for registering a new platform (admin) or user-scoped MCP server.
pub struct NewServerInput {
    pub name: String,
    pub url: String,
    pub transport: String,
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub basic_username: Option<String>,
    pub basic_password: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_platform: bool,
    pub is_active: bool,
}

/// Register a generic MCP server: validation, SSRF guard, duplicate/collision
/// checks, basic-auth header pre-computation, and the repo write. `user_id` is
/// `None` for a platform server (caller must already have checked admin) and
/// `Some` for a user-scoped one.
pub async fn create_server(
    state: &McpState,
    user_id: Option<Uuid>,
    input: NewServerInput,
) -> Result<McpServer> {
    if !AUTH_TYPES.contains(&input.auth_type.as_str()) {
        return Err(McpError::BadRequest(format!("auth_type must be one of {AUTH_TYPES:?}")));
    }
    if input.auth_type == "url_param" && input.url_param_name.is_none() {
        return Err(McpError::BadRequest("url_param_name is required when auth_type='url_param'".into()));
    }
    // SSRF guard: reject URLs resolving to private/internal addresses.
    crate::net::validate_public_url(&input.url).await?;

    // Duplicate + name-collision guards (mirror the PoC).
    let existing = if input.is_platform {
        repo::get_platform_mcp_server_by_name(&state.db, &input.name).await?
    } else {
        repo::get_user_mcp_server_by_name(&state.db, user_id.expect("user-scoped server needs user_id"), &input.name)
            .await?
    };
    if existing.is_some() {
        return Err(McpError::Conflict(format!("MCP server '{}' already exists in this scope", input.name)));
    }
    if repo::get_platform_auth_config_by_toolkit(&state.db, &input.name).await?.is_some() {
        return Err(McpError::Conflict(format!(
            "'{}' is already a Composio toolkit — choose a different MCP server name",
            input.name
        )));
    }

    // For user-scoped basic auth, precompute the Authorization: Basic header.
    let mut headers = input.headers.clone().unwrap_or_default();
    if input.auth_type == "basic"
        && !input.is_platform
        && let (Some(u), Some(p)) = (&input.basic_username, &input.basic_password)
    {
        let encoded = B64.encode(format!("{u}:{p}"));
        headers.insert("Authorization".to_string(), format!("Basic {encoded}"));
    }
    let headers_json = if headers.is_empty() { None } else { Some(serde_json::to_value(&headers).unwrap_or(Value::Null)) };

    let new = NewMcpServer {
        name: input.name,
        url: input.url,
        transport: input.transport,
        auth_type: input.auth_type,
        url_param_name: input.url_param_name,
        credential_header_name: input.credential_header_name,
        headers: headers_json,
        description: input.description,
        display_name: input.display_name,
        logo_url: input.logo_url,
        is_platform: input.is_platform,
        user_id,
        is_active: input.is_active,
    };

    let server = repo::create_mcp_server(&state.db, &new).await?;
    tracing::info!(name = %server.name, auth_type = %server.auth_type, is_platform = server.is_platform, "registered mcp server");
    Ok(server)
}

/// `GET /api/mcp/servers` view: all servers visible to `user_id` (platform +
/// own).
pub async fn list_servers_view(state: &McpState, user_id: Uuid) -> Result<Value> {
    let servers = repo::list_mcp_servers_for_user(&state.db, user_id).await?;
    let data: Vec<Value> = servers.iter().map(server_dto).collect();
    let total = data.len();
    Ok(json!({ "data": data, "total": total }))
}

/// Look up a server by id for a pending delete. The server layer uses this to
/// decide authorization (platform → admin, owned → matching user) before
/// calling [`delete_server`].
pub async fn get_server_for_deletion(state: &McpState, id: Uuid) -> Result<McpServer> {
    repo::get_mcp_server_by_id(&state.db, id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("MCP server '{id}' not found")))
}

/// Delete a server (already authorized by the caller) and clean up any
/// per-agent permission rows + permission cache entries that referenced it.
/// `cleanup_user` is `None` for a platform server (gone for everyone) or
/// `Some(user_id)` for an owned one.
pub async fn delete_server(state: &McpState, server: &McpServer, cleanup_user: Option<Uuid>) -> Result<()> {
    // Snapshot affected (user, agent) pairs, delete the server + its permission
    // rows, then invalidate the permission cache for each pair.
    let pairs = repo::get_agent_pairs_for_server(&state.db, &server.name, cleanup_user).await?;
    repo::delete_mcp_server(&state.db, server.id).await?;
    repo::delete_agent_permissions_for_server(&state.db, &server.name, cleanup_user).await?;
    for (uid, aid) in pairs {
        permissions::invalidate_permission_cache(state, uid, aid).await;
    }
    tracing::info!(name = %server.name, "deleted mcp server");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_2xx_is_none() {
        assert_eq!(classify_response(StatusCode::OK, ""), DetectedAuthType::None);
        assert_eq!(classify_response(StatusCode::CREATED, "Bearer"), DetectedAuthType::None);
    }

    #[test]
    fn classify_401_with_resource_metadata_is_oauth2() {
        assert_eq!(
            classify_response(StatusCode::UNAUTHORIZED, "Bearer resource_metadata=\"...\""),
            DetectedAuthType::OAuth2
        );
    }

    #[test]
    fn classify_401_without_resource_metadata_is_bearer() {
        assert_eq!(classify_response(StatusCode::UNAUTHORIZED, "Bearer realm=\"mcp\""), DetectedAuthType::Bearer);
        assert_eq!(classify_response(StatusCode::UNAUTHORIZED, ""), DetectedAuthType::Bearer);
    }

    #[test]
    fn classify_other_non_2xx_defaults_to_bearer() {
        assert_eq!(classify_response(StatusCode::INTERNAL_SERVER_ERROR, ""), DetectedAuthType::Bearer);
        assert_eq!(classify_response(StatusCode::NOT_FOUND, ""), DetectedAuthType::Bearer);
        assert_eq!(classify_response(StatusCode::FORBIDDEN, ""), DetectedAuthType::Bearer);
    }

    #[test]
    fn detected_auth_type_as_str() {
        assert_eq!(DetectedAuthType::None.as_str(), "none");
        assert_eq!(DetectedAuthType::OAuth2.as_str(), "oauth2");
        assert_eq!(DetectedAuthType::Bearer.as_str(), "bearer");
    }

    #[test]
    fn auth_types_contains_all_five_known_kinds() {
        for kind in ["none", "bearer", "basic", "oauth2", "url_param"] {
            assert!(AUTH_TYPES.contains(&kind), "missing {kind}");
        }
        assert_eq!(AUTH_TYPES.len(), 5);
    }
}
