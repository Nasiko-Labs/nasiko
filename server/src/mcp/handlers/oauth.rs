//! MCP OAuth 2.1 per-connector flow: authorize / callback / status / revoke.

use axum::{
    Json,
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use nasiko_mcp_gateway::oauth::CallbackOutcome;

use super::super::openapi::McpEnvelope;
use super::super::{ApiError, ApiResponse, AppPath, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default, ToSchema)]
pub struct AuthorizeRequest {
    pub client_id: Option<String>,
    pub redirect_url: Option<String>,
}

/// `POST /api/mcp/connectors/{id}/oauth/authorize` — start the OAuth 2.1 flow.
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/{id}/oauth/authorize",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    request_body(content = Option<AuthorizeRequest>),
    responses(
        (status = 200, description = "Authorization URL generated — `data` is `{connector_id, name, authorization_url}`", body = McpEnvelope),
        (status = 404, description = "No such OAuth connector", body = McpEnvelope),
    ),
)]
pub async fn authorize(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
    body: Option<Json<AuthorizeRequest>>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    Ok(ApiResponse::ok(
        service::oauth::authorize(
            &state,
            user_id,
            connector_id,
            body.client_id,
            body.redirect_url,
        )
        .await?,
        "OAuth authorization URL generated successfully",
    ))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// `GET /api/mcp/oauth/callback` — public browser redirect target (HTML, not JSON).
#[utoipa::path(
    get,
    path = "/api/mcp/oauth/callback",
    tag = "mcp",
    params(
        ("code" = Option<String>, Query, description = "OAuth authorization code"),
        ("state" = Option<String>, Query, description = "Opaque state minted by the authorize step"),
        ("error" = Option<String>, Query, description = "OAuth error code, if the provider denied"),
        ("error_description" = Option<String>, Query, description = "Human-readable OAuth error"),
    ),
    responses(
        (status = 200, description = "HTML page closing the popup, or an HTML error page", content_type = "text/html", body = String),
        (status = 302, description = "Redirect back to the requesting app"),
    ),
)]
pub async fn callback(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> Response {
    match service::oauth::callback(&state, q.code, q.state, q.error, q.error_description).await {
        CallbackOutcome::Redirect(dest) => Redirect::to(&dest).into_response(),
        CallbackOutcome::ClosePopup(fallback_url) => {
            Html(close_popup_page(&fallback_url)).into_response()
        }
        CallbackOutcome::Message(msg) => Html(error_page(&msg)).into_response(),
    }
}

/// `GET /api/mcp/connectors/{id}/oauth/status` — token presence + expiry.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/{id}/oauth/status",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "OAuth status — `data` is `{connector_id, name, authorized, expires_at, scope}`", body = McpEnvelope),
        (status = 404, description = "No such OAuth connector", body = McpEnvelope),
    ),
)]
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::oauth::status(&state, user_id, connector_id).await?,
        "OAuth status retrieved successfully",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/oauth/token` — remove the caller's token.
#[utoipa::path(
    delete,
    path = "/api/mcp/connectors/{id}/oauth/token",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "OAuth token revoked", body = McpEnvelope),
        (status = 404, description = "No token to revoke", body = McpEnvelope),
    ),
)]
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::oauth::revoke(&state, user_id, connector_id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "OAuth token revoked successfully",
    ))
}

/// HTML page that closes the popup window. Falls back to a redirect if
/// `window.close()` is blocked (e.g. flow ran in a top-level tab).
fn close_popup_page(fallback_url: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Connected</title></head>\
         <body style=\"font-family:sans-serif;text-align:center;padding:48px\">\
         <p style=\"color:#666\">Authorization complete. This window will close automatically.</p>\
         <script>window.close();setTimeout(function(){{location.href=\"{}\"}},1000);</script>\
         </body></html>",
        html_escape(fallback_url)
    )
}

fn error_page(message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Connection Error</title></head>\
         <body style=\"font-family:sans-serif;text-align:center;padding:48px\">\
         <h2 style=\"color:#dc2626\">Connection failed</h2><p style=\"color:#666\">{}</p></body></html>",
        html_escape(message)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
