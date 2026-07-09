//! MCP OAuth 2.1 per-server flow: authorize / callback / status / revoke.
//!
//! Building blocks (discovery, PKCE, signed state, token exchange) and the
//! authorize/callback orchestration live in the crate's `oauth` module; these
//! handlers extract identity and shape the HTTP response.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::oauth::{self, CallbackOutcome};
use nasiko_mcp_gateway::{McpError, repo, session};

use super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct AuthorizeRequest {
    /// Pre-registered client_id (skips dynamic client registration).
    pub client_id: Option<String>,
    /// Where to send the user after OAuth completes.
    pub redirect_url: Option<String>,
}

/// `POST /api/mcp/servers/{id}/oauth/authorize` — start the OAuth 2.1 flow.
pub async fn authorize(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
    body: Option<Json<AuthorizeRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let server = oauth::load_owned_oauth_server(&state.mcp, user_id, server_id).await?;
    let (server_id, server_name) = (server.id, server.name.clone());
    let authorization_url =
        oauth::begin_authorization(&state.mcp, user_id, server, body.redirect_url, body.client_id).await?;
    Ok(Json(json!({
        "server_id": server_id,
        "server_name": server_name,
        "authorization_url": authorization_url,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// `GET /api/mcp/oauth/callback` — public browser redirect target. Exchanges the
/// code for tokens (encrypted), then redirects to the caller's success URL.
pub async fn callback(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> Response {
    match oauth::handle_callback(&state.mcp, q.code, q.state, q.error, q.error_description).await {
        CallbackOutcome::Redirect(dest) => Redirect::to(&dest).into_response(),
        CallbackOutcome::Message(msg) => Html(error_page(&msg)).into_response(),
    }
}

/// `GET /api/mcp/servers/{id}/oauth/status` — token presence + expiry.
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = oauth::load_owned_oauth_server(&state.mcp, user_id, server_id).await?;
    let token = repo::get_mcp_oauth_token(&state.mcp.db, server_id, user_id).await?;
    Ok(Json(json!({
        "server_id": server.id,
        "server_name": server.name,
        "authorized": token.is_some(),
        "expires_at": token.as_ref().and_then(|t| t.expires_at),
        "scope": token.and_then(|t| t.scope),
    })))
}

/// `DELETE /api/mcp/servers/{id}/oauth/token` — remove the caller's token.
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    oauth::load_owned_oauth_server(&state.mcp, user_id, server_id).await?;
    if !repo::delete_mcp_oauth_token(&state.mcp.db, server_id, user_id).await? {
        return Err(ApiError(McpError::NotFound("no token to revoke".into())));
    }
    session::invalidate_session_cache(&state.mcp, user_id).await;
    Ok(StatusCode::NO_CONTENT)
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
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
