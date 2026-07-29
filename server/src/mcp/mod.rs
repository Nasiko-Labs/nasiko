//! MCP gateway HTTP surface.
//!
//! Layered: `mod.rs` assembles routes + shared helpers; `handlers/` does axum
//! extraction + ACL; `service/` wraps the `nasiko-mcp-gateway` crate (all logic +
//! SQL live in the crate, so `ee/` reuses it via the same routers).

pub mod build;
mod handlers;
mod service;

use axum::{
    Json, Router,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde::de::DeserializeOwned;
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

/// MCP-server-upload MUTATION routes (build a container from user-supplied
/// source) — deployer+ only, gated the same way agent-build/upload mutations
/// are in `lib.rs::build_app_with_user_router` (building a container is the
/// same class of privileged, resource-consuming operation). Kept as its own
/// router, separate from [`router`], so that gate can be layered on
/// specifically these two routes without affecting the rest of the MCP
/// management surface.
///
/// `mcp_upload_max_bytes` sizes the body-limit layer scoped to the zip-upload
/// route only (mirrors `agents::upload::router()`'s own `DefaultBodyLimit`,
/// but this one's limit is config-driven, not a hardcoded constant, so it's a
/// parameter here rather than baked into the router).
pub fn upload_mutation_router(mcp_upload_max_bytes: u64) -> Router<AppState> {
    let upload_zip_route = Router::new()
        .route("/mcp/connectors/upload", post(handlers::upload::upload_zip))
        .layer(axum::extract::DefaultBodyLimit::max(
            mcp_upload_max_bytes as usize,
        ));

    Router::new().merge(upload_zip_route).route(
        "/mcp/connectors/upload-github",
        post(handlers::upload::upload_github),
    )
}

/// Authed MCP management routes (inherit `require_auth`).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/mcp/connectors/{id}/build-status",
            get(handlers::upload::build_status),
        )
        .route(
            "/mcp/connectors/{id}/build-logs",
            get(handlers::upload::build_logs),
        )
        // Catalog + platform Composio connector registration.
        .route("/mcp/catalog", get(handlers::catalog::get_catalog))
        .route(
            "/mcp/composio/toolkits",
            get(handlers::catalog::list_toolkits),
        )
        .route(
            "/mcp/auth-configs",
            get(handlers::catalog::list_auth_configs).post(handlers::catalog::create_auth_config),
        )
        .route(
            "/mcp/auth-configs/{connector_id}",
            patch(handlers::catalog::update_auth_config)
                .delete(handlers::catalog::delete_auth_config),
        )
        // Unified connect / disconnect / connection listing.
        .route("/mcp/connect", post(handlers::connect::connect_service))
        .route("/mcp/connections", get(handlers::connect::list_connections))
        .route(
            "/mcp/connections/{connector_id}",
            delete(handlers::connect::disconnect),
        )
        // Custom MCP connector registration + probe + sharing.
        .route(
            "/mcp/connectors",
            get(handlers::connectors::list).post(handlers::connectors::create),
        )
        .route("/mcp/connectors/probe", post(handlers::connectors::probe))
        .route(
            "/mcp/connectors/my-uploads",
            get(handlers::upload::list_my_uploads),
        )
        .route(
            "/mcp/connectors/{id}",
            get(handlers::connectors::get)
                .patch(handlers::connectors::update)
                .delete(handlers::connectors::delete),
        )
        .route("/mcp/connectors/{id}/grants", get(handlers::sharing::list))
        .route(
            "/mcp/connectors/{id}/grants/public",
            post(handlers::sharing::grant_public).delete(handlers::sharing::revoke_public),
        )
        .route(
            "/mcp/connectors/{id}/grants/users/{user_id}",
            post(handlers::sharing::grant_user).delete(handlers::sharing::revoke_user),
        )
        .route(
            "/mcp/connectors/{id}/grants/agents/{agent_id}",
            post(handlers::sharing::grant_agent).delete(handlers::sharing::revoke_agent),
        )
        .route("/mcp/share-targets", get(handlers::sharing::search_targets))
        .route(
            "/mcp/share-targets/resolve",
            get(handlers::sharing::resolve_target),
        )
        .route(
            "/mcp/connectors/{id}/consumers",
            get(handlers::sharing::consumers),
        )
        .route(
            "/mcp/connectors/{id}/pin",
            post(handlers::connectors::pin).delete(handlers::connectors::unpin),
        )
        .route("/mcp/connectors/pinned", get(handlers::connectors::pinned))
        .route("/mcp/connectors/recent", get(handlers::connectors::recent))
        // Per-user credentials (write-only).
        .route(
            "/mcp/connectors/{id}/credential",
            post(handlers::credentials::register).delete(handlers::credentials::delete),
        )
        .route(
            "/mcp/connectors/{id}/credential/status",
            get(handlers::credentials::status),
        )
        // MCP OAuth 2.1 per connector.
        .route(
            "/mcp/connectors/{id}/oauth/authorize",
            post(handlers::oauth::authorize),
        )
        .route(
            "/mcp/connectors/{id}/oauth/status",
            get(handlers::oauth::status),
        )
        .route(
            "/mcp/connectors/{id}/oauth/token",
            delete(handlers::oauth::revoke),
        )
        // Per-agent connector access + tool rules.
        .route(
            "/mcp/agents/{agent_id}/connectors",
            get(handlers::permissions::list_connectors),
        )
        .route(
            "/mcp/agents/{agent_id}/connectors/{connector_id}",
            put(handlers::permissions::set_connector_access),
        )
        .route(
            "/mcp/agents/{agent_id}/connectors/{connector_id}/tools",
            get(handlers::permissions::list_connector_tools),
        )
        .route(
            "/mcp/agents/{agent_id}/tools",
            get(handlers::permissions::list_tool_rules)
                .put(handlers::permissions::bulk_update_tools),
        )
        .route(
            "/mcp/agents/{agent_id}/permissions",
            delete(handlers::permissions::reset),
        )
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

/// Standard API envelope: `{"data": …, "status_code": N, "message": "…"}`.
/// `pub` (not `pub(crate)`) so `ee/server`'s own MCP-related handlers
/// (`mcp_sharing.rs`) can produce the same envelope shape.
pub struct ApiResponse {
    status: StatusCode,
    data: serde_json::Value,
    message: &'static str,
}

impl ApiResponse {
    pub fn ok(data: serde_json::Value, message: &'static str) -> Self {
        Self {
            status: StatusCode::OK,
            data,
            message,
        }
    }

    pub fn created(data: serde_json::Value, message: &'static str) -> Self {
        Self {
            status: StatusCode::CREATED,
            data,
            message,
        }
    }

    /// 202 — request accepted, processing continues asynchronously (queued
    /// build jobs; see `handlers::upload`).
    pub fn accepted(data: serde_json::Value, message: &'static str) -> Self {
        Self {
            status: StatusCode::ACCEPTED,
            data,
            message,
        }
    }
}

impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        let code = self.status.as_u16();
        (
            self.status,
            Json(json!({
                "data": self.data,
                "status_code": code,
                "message": self.message,
            })),
        )
            .into_response()
    }
}

/// Drop-in replacement for `axum::Json` in handler *arguments* that converts
/// deserialization failures into the standard `ApiError` envelope instead of
/// Axum's default plain-text 422.
pub(crate) struct AppJson<T>(pub T);

impl<T, S> FromRequest<S> for AppJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        use axum::extract::rejection::JsonRejection;
        match Json::<T>::from_request(req, state).await {
            Ok(Json(val)) => Ok(AppJson(val)),
            Err(e) => {
                let msg = match &e {
                    JsonRejection::JsonDataError(_) => {
                        format!("invalid request body: {}", e.body_text())
                    }
                    JsonRejection::JsonSyntaxError(_) => {
                        format!("invalid JSON syntax: {}", e.body_text())
                    }
                    JsonRejection::MissingJsonContentType(_) => {
                        "Content-Type must be application/json".to_string()
                    }
                    _ => e.body_text(),
                };
                Err(ApiError(McpError::BadRequest(msg)))
            }
        }
    }
}

/// Wraps [`McpError`] as an HTTP response for the management routes. `pub` so
/// `ee/server`'s MCP handlers can return it too (see [`ApiResponse`]).
pub struct ApiError(pub McpError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let code = status.as_u16();
        (
            status,
            Json(json!({
                "data": serde_json::Value::Null,
                "status_code": code,
                "message": self.0.client_message(),
            })),
        )
            .into_response()
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

/// Require that the caller can manage `agent_id` (owner / grant / superuser).
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

/// Require that the caller can manage `connector_id`'s state on `agent_id` —
/// full agent management, or ownership of `connector_id` plus an existing
/// agent-grant onto it (see [`crate::acl::can_manage_agent_connector`]).
pub(crate) async fn ensure_can_manage_agent_connector(
    state: &AppState,
    claims: &Claims,
    agent_id: Uuid,
    connector_id: Uuid,
) -> Result<(), ApiError> {
    if crate::acl::can_manage_agent_connector(state, claims, agent_id, connector_id).await {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden(
            "you do not have permission to manage this connector on this agent".into(),
        )))
    }
}

/// Drop-in for `axum::extract::Path` that converts path-param parse failures into
/// the standard `ApiError` envelope instead of Axum's plain-text 422.
pub(crate) struct AppPath<T>(pub T);

impl<T, S> FromRequestParts<S> for AppPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(val)) => Ok(AppPath(val)),
            Err(e) => Err(ApiError(McpError::BadRequest(format!(
                "invalid path parameter: {e}"
            )))),
        }
    }
}

/// Drop-in for `axum::extract::Query` that converts query-string parse failures into
/// the standard `ApiError` envelope instead of Axum's plain-text 400.
pub(crate) struct AppQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for AppQuery<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(val)) => Ok(AppQuery(val)),
            Err(e) => Err(ApiError(McpError::BadRequest(format!(
                "invalid query parameter: {e}"
            )))),
        }
    }
}

/// Require a superuser for platform-wide mutations (registering composio connectors).
pub(crate) fn ensure_admin(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_superuser {
        Ok(())
    } else {
        Err(ApiError(McpError::Forbidden(
            "admin privileges required for platform configuration".into(),
        )))
    }
}
