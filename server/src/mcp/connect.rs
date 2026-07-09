//! Unified connect / disconnect + connection listing, and the Composio OAuth
//! browser callback.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use nasiko_mcp_gateway::connect::{self, ConnectInput, ConnectOutcome};
use nasiko_mcp_gateway::oauth::CallbackOutcome;

use super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    pub service: Option<String>,
    pub toolkit: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub url: Option<String>,
    pub credentials: Option<Credentials>,
    pub redirect_url: Option<String>,
}

/// `POST /api/mcp/connect` — connect any service type.
pub async fn connect_service(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ConnectRequest>,
) -> Result<Response, ApiError> {
    let user_id = parse_user(&claims)?;
    let service =
        body.service.clone().or_else(|| body.toolkit.clone()).map(|s| s.to_lowercase()).unwrap_or_default();

    let outcome = connect::connect_service(
        &state.mcp,
        user_id,
        ConnectInput {
            service,
            kind: body.kind,
            url: body.url,
            credential_value: body.credentials.map(|c| c.value),
            redirect_url: body.redirect_url,
        },
    )
    .await?;

    Ok(match outcome {
        ConnectOutcome::Connected { service } => {
            (StatusCode::OK, Json(json!({ "status": "connected", "service": service }))).into_response()
        }
        ConnectOutcome::Initiated { service, oauth_url } => (
            StatusCode::CREATED,
            Json(json!({ "status": "initiated", "service": service, "oauth_url": oauth_url })),
        )
            .into_response(),
        ConnectOutcome::OAuthRequired { service, authorization_url } => Json(json!({
            "status": "oauth_required",
            "service": service,
            "authorization_url": authorization_url,
        }))
        .into_response(),
    })
}

/// `GET /api/mcp/connections` — list the caller's connections, syncing any
/// pending ones with Composio.
pub async fn list_connections(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(Json(connect::list_connections_view(&state.mcp, user_id).await?))
}

/// `DELETE /api/mcp/connections/{toolkit}` — revoke a Composio connection.
pub async fn disconnect_toolkit(
    State(state): State<AppState>,
    claims: Claims,
    Path(toolkit): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let outcome = connect::disconnect_toolkit(&state.mcp, user_id, &toolkit).await?;
    Ok(Json(json!({
        "message": outcome.message,
        "toolkit": outcome.toolkit,
        "composio_revoked": outcome.composio_revoked,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ComposioCallbackQuery {
    pub user_id: Option<Uuid>,
    pub toolkit: Option<String>,
    pub success_url: Option<String>,
}

/// `GET /oauth/callback` — public Composio redirect target. Verifies the
/// connection became ACTIVE, records the account id, invalidates the session
/// cache, and redirects.
pub async fn oauth_callback(State(state): State<AppState>, Query(q): Query<ComposioCallbackQuery>) -> Response {
    match connect::handle_composio_callback(&state.mcp, q.user_id, q.toolkit, q.success_url).await {
        CallbackOutcome::Redirect(dest) => Redirect::to(&dest).into_response(),
        CallbackOutcome::Message(msg) => Html(callback_page(&msg)).into_response(),
    }
}

fn callback_page(message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Connecting…</title></head>\
         <body style=\"font-family:sans-serif;text-align:center;padding:48px\">\
         <p style=\"color:#666\">{}</p></body></html>",
        message.replace('<', "&lt;")
    )
}
