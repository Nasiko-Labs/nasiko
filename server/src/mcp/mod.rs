//! MCP gateway HTTP surface.
//!
//! Per plan §20.1, the Axum route handlers live **here in the server crate** (so
//! they can use `AppState`, `Claims`, `acl`, `UsageTracker`) while all the pure
//! logic lives in the `nasiko-mcp-gateway` crate. Handlers are thin: extract
//! identity, gate access, and delegate to `state.mcp` (the crate's `McpState`).
//!
//! Four routers:
//!   * [`agent_gateway_router`] — `POST /api/mcp` only. Mounted OUTSIDE
//!     `require_auth`, with its own [`gateway::require_delegation`] auth layer,
//!     because an agent's only credential is the delegation token, never a
//!     session JWT (see `gateway.rs` module doc for why).
//!   * [`router`] — authed management routes, merged into the server's
//!     protected `/api` group (inherits `require_auth`).
//!   * [`public_api_router`] — unauthed routes merged under `/api` *after* the
//!     auth layer (`/api/mcp/oauth/callback`, `/api/mcp/webhooks/composio`) — they
//!     authenticate via OAuth state / HMAC, not a user JWT.
//!   * [`composio_callback_router`] — the root-level browser redirect target
//!     `GET /oauth/callback` (Composio OAuth completion).

mod catalog;
mod connect;
mod credentials;
mod gateway;
mod oauth;
mod permissions;
mod servers;
mod webhooks;

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;

use crate::auth::Claims;
use crate::state::AppState;

pub use gateway::require_delegation;

/// The agent-facing MCP JSON-RPC gateway — `POST /api/mcp` — on its own,
/// separate from every other route. Mount this with [`gateway::require_delegation`]
/// as its auth layer, NOT `require_auth`.
pub fn agent_gateway_router() -> Router<AppState> {
    Router::new().route("/mcp", post(gateway::mcp_gateway))
}

/// Authed MCP management routes. Merged into the server's `protected` group,
/// inheriting `require_auth`.
pub fn router() -> Router<AppState> {
    Router::new()
        // Public-safe catalog + platform toolkit (Composio auth-config) setup.
        .route("/mcp/catalog", get(catalog::get_catalog))
        .route("/mcp/auth-configs", get(catalog::list_auth_configs).post(catalog::create_auth_config))
        .route("/mcp/auth-configs/{auth_config_id}", delete(catalog::delete_auth_config))
        // Unified connect / disconnect / connection listing.
        .route("/mcp/connect", post(connect::connect_service))
        .route("/mcp/connections", get(connect::list_connections))
        .route("/mcp/connections/{toolkit}", delete(connect::disconnect_toolkit))
        // Generic MCP server registration + probe.
        .route("/mcp/servers", get(servers::list_servers).post(servers::create_server))
        .route("/mcp/servers/probe", post(servers::probe_server))
        .route("/mcp/servers/{id}", delete(servers::delete_server))
        // Per-user credentials (write-only).
        .route(
            "/mcp/servers/{id}/credential",
            post(credentials::register).delete(credentials::delete),
        )
        .route("/mcp/servers/{id}/credential/status", get(credentials::status))
        // MCP OAuth 2.1 per server.
        .route("/mcp/servers/{id}/oauth/authorize", post(oauth::authorize))
        .route("/mcp/servers/{id}/oauth/status", get(oauth::status))
        .route("/mcp/servers/{id}/oauth/token", delete(oauth::revoke))
        // Per-agent permission management (the connector UI backend).
        .route("/mcp/agents/{agent_id}/servers", get(permissions::list_servers))
        .route("/mcp/agents/{agent_id}/servers/{server}", put(permissions::set_server_access))
        .route("/mcp/agents/{agent_id}/servers/{server}/tools", get(permissions::list_server_tools))
        .route(
            "/mcp/agents/{agent_id}/tools",
            get(permissions::list_tool_rules).put(permissions::bulk_update_tools),
        )
        .route("/mcp/agents/{agent_id}/permissions", delete(permissions::reset))
}

/// Unauthed MCP routes served under `/api` (they authenticate via OAuth state /
/// HMAC). Merged after the auth layer.
pub fn public_api_router() -> Router<AppState> {
    Router::new()
        .route("/mcp/oauth/callback", get(oauth::callback))
        .route("/mcp/webhooks/composio", post(webhooks::composio))
}

/// Root-level browser redirect target for Composio OAuth completion.
pub fn composio_callback_router() -> Router<AppState> {
    Router::new().route("/oauth/callback", get(connect::oauth_callback))
}

// ─── Shared error + auth helpers ────────────────────────────────────────────

/// Wraps [`McpError`] as an HTTP response for the management routes.
pub(crate) struct ApiError(pub McpError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({ "error": self.0.client_message() }))).into_response()
    }
}

impl From<McpError> for ApiError {
    fn from(e: McpError) -> Self {
        ApiError(e)
    }
}

/// Parse the authenticated user's UUID from claims.
pub(crate) fn parse_user(claims: &Claims) -> Result<Uuid, ApiError> {
    claims
        .sub
        .parse()
        .map_err(|_| ApiError(McpError::Unauthorized("invalid user id in identity".into())))
}

/// Require that the caller can manage `agent_id` (owner / public / grant, or
/// superuser). Mirrors `catalog::agent_secrets::can_manage_agent`.
pub(crate) async fn ensure_can_manage_agent(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
) -> Result<(), ApiError> {
    if crate::acl::can_manage_agent(state, claims, agent_id).await {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden(
            "you do not have permission to manage this agent".into(),
        )))
    }
}

/// Require a superuser for platform-wide mutations (registering platform
/// toolkits / servers). OSS `Claims` carries no role hierarchy (see
/// `oss/server/src/auth/claims.rs`) — superuser is the only admin concept.
pub(crate) fn ensure_admin(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_superuser {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden(
            "admin privileges required for platform configuration".into(),
        )))
    }
}
