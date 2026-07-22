//! MCP OAuth 2.1 per-connector flow: authorize / callback / status / revoke.

use axum::{
    Json,
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use nasiko_mcp_gateway::oauth::CallbackOutcome;

use super::super::{ApiError, ApiResponse, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct AuthorizeRequest {
    pub client_id: Option<String>,
    pub redirect_url: Option<String>,
}

/// `POST /api/mcp/connectors/{id}/oauth/authorize` — start the OAuth 2.1 flow.
pub async fn authorize(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
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
pub async fn callback(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> Response {
    match service::oauth::callback(&state, q.code, q.state, q.error, q.error_description).await {
        CallbackOutcome::Redirect(dest) => Redirect::to(&dest).into_response(),
        CallbackOutcome::Message(msg) => Html(error_page(&msg)).into_response(),
    }
}

/// `GET /api/mcp/connectors/{id}/oauth/status` — token presence + expiry.
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::oauth::status(&state, user_id, connector_id).await?,
        "OAuth status retrieved successfully",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/oauth/token` — remove the caller's token.
pub async fn revoke(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::oauth::revoke(&state, user_id, connector_id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "OAuth token revoked successfully"))
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
