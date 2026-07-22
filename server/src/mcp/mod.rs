//! MCP gateway HTTP surface.
//!
//! Layered: `mod.rs` assembles routes + shared helpers; `handlers/` does axum
//! extraction + ACL; `service/` wraps the `nasiko-mcp-gateway` crate (all logic +
//! SQL live in the crate, so `ee/` reuses it via the same routers).

mod handlers;
mod service;

use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;

use crate::auth::Claims;
use crate::state::AppState;

pub use handlers::gateway::require_delegation;

/// Agent-facing MCP JSON-RPC gateway — `POST /api/mcp` — mounted with
/// [`require_delegation`], NOT `require_auth`.
pub fn agent_gateway_router() -> Router<AppState> {
    Router::new().route("/mcp", post(handlers::gateway::mcp_gateway))
}

/// Authed MCP management routes (inherit `require_auth`).
pub fn router() -> Router<AppState> {
    Router::new()
        // Catalog + platform Composio connector registration.
        .route("/mcp/catalog", get(handlers::catalog::get_catalog))
        .route("/mcp/auth-configs", get(handlers::catalog::list_auth_configs).post(handlers::catalog::create_auth_config))
        .route(
            "/mcp/auth-configs/{connector_id}",
            patch(handlers::catalog::update_auth_config).delete(handlers::catalog::delete_auth_config),
        )
        // Unified connect / disconnect / connection listing.
        .route("/mcp/connect", post(handlers::connect::connect_service))
        .route("/mcp/connections", get(handlers::connect::list_connections))
        .route("/mcp/connections/{connector_id}", delete(handlers::connect::disconnect))
        // Custom MCP connector registration + probe + sharing.
        .route("/mcp/connectors", get(handlers::connectors::list).post(handlers::connectors::create))
        .route("/mcp/connectors/probe", post(handlers::connectors::probe))
        .route(
            "/mcp/connectors/{id}",
            get(handlers::connectors::get).patch(handlers::connectors::update).delete(handlers::connectors::delete),
        )
        .route(
            "/mcp/connectors/{id}/share",
            get(handlers::sharing::list).post(handlers::sharing::share).delete(handlers::sharing::revoke),
        )
        .route("/mcp/share-targets", get(handlers::sharing::search_targets))
        .route("/mcp/connectors/{id}/consumers", get(handlers::sharing::consumers))
        .route("/mcp/connectors/{id}/pin", post(handlers::connectors::pin).delete(handlers::connectors::unpin))
        .route("/mcp/connectors/pinned", get(handlers::connectors::pinned))
        .route("/mcp/connectors/recent", get(handlers::connectors::recent))
        // Per-user credentials (write-only).
        .route(
            "/mcp/connectors/{id}/credential",
            post(handlers::credentials::register).delete(handlers::credentials::delete),
        )
        .route("/mcp/connectors/{id}/credential/status", get(handlers::credentials::status))
        // MCP OAuth 2.1 per connector.
        .route("/mcp/connectors/{id}/oauth/authorize", post(handlers::oauth::authorize))
        .route("/mcp/connectors/{id}/oauth/status", get(handlers::oauth::status))
        .route("/mcp/connectors/{id}/oauth/token", delete(handlers::oauth::revoke))
        // Per-agent connector access + tool rules.
        .route("/mcp/agents/{agent_id}/connectors", get(handlers::permissions::list_connectors))
        .route("/mcp/agents/{agent_id}/connectors/{connector_id}", put(handlers::permissions::set_connector_access))
        .route(
            "/mcp/agents/{agent_id}/connectors/{connector_id}/tools",
            get(handlers::permissions::list_connector_tools),
        )
        .route(
            "/mcp/agents/{agent_id}/tools",
            get(handlers::permissions::list_tool_rules).put(handlers::permissions::bulk_update_tools),
        )
        .route("/mcp/agents/{agent_id}/permissions", delete(handlers::permissions::reset))
}

/// Unauthed MCP routes served under `/api` (authenticate via OAuth state / HMAC).
pub fn public_api_router() -> Router<AppState> {
    Router::new()
        .route("/mcp/oauth/callback", get(handlers::oauth::callback))
        .route("/mcp/webhooks/composio", post(handlers::webhooks::composio))
}

/// Root-level browser redirect target for Composio OAuth completion.
pub fn composio_callback_router() -> Router<AppState> {
    Router::new().route("/oauth/callback", get(handlers::connect::oauth_callback))
}

// ─── Shared error + auth helpers ────────────────────────────────────────────

/// Wraps [`McpError`] as an HTTP response for the management routes.
pub(crate) struct ApiError(pub McpError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
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
    claims.sub.parse().map_err(|_| ApiError(McpError::Unauthorized("invalid user id in identity".into())))
}

/// Require that the caller can manage `agent_id` (owner / grant / superuser).
pub(crate) async fn ensure_can_manage_agent(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
) -> Result<(), ApiError> {
    if crate::acl::can_manage_agent(state, claims, agent_id).await {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden("you do not have permission to manage this agent".into())))
    }
}

/// Require a superuser for platform-wide mutations (registering composio connectors).
pub(crate) fn ensure_admin(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_superuser {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden("admin privileges required for platform configuration".into())))
    }
}
