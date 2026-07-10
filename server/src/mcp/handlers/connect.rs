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

use nasiko_mcp_gateway::oauth::CallbackOutcome;

use super::super::{ApiError, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    pub connector_id: Option<Uuid>,
    pub service: Option<String>,
    pub toolkit: Option<String>,
    pub url: Option<String>,
    pub credentials: Option<Credentials>,
    pub redirect_url: Option<String>,
}

/// `POST /api/mcp/connect` — connect any connector type.
pub async fn connect_service(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ConnectRequest>,
) -> Result<Response, ApiError> {
    use service::connect::{ConnectInput, ConnectOutcome};
    let user_id = parse_user(&claims)?;
    let outcome = service::connect::connect(
        &state,
        user_id,
        ConnectInput {
            connector_id: body.connector_id,
            service: body.service.or(body.toolkit),
            url: body.url,
            credential_value: body.credentials.map(|c| c.value),
            redirect_url: body.redirect_url,
        },
    )
    .await?;

    Ok(match outcome {
        ConnectOutcome::Connected { connector_id, name } => (
            StatusCode::OK,
            Json(json!({ "status": "connected", "connector_id": connector_id, "name": name })),
        )
            .into_response(),
        ConnectOutcome::Initiated { connector_id, name, oauth_url } => (
            StatusCode::CREATED,
            Json(json!({ "status": "initiated", "connector_id": connector_id, "name": name, "oauth_url": oauth_url })),
        )
            .into_response(),
        ConnectOutcome::OAuthRequired { connector_id, name, authorization_url } => Json(json!({
            "status": "oauth_required",
            "connector_id": connector_id,
            "name": name,
            "authorization_url": authorization_url,
        }))
        .into_response(),
    })
}

/// `GET /api/mcp/connections` — the caller's connections.
pub async fn list_connections(State(state): State<AppState>, claims: Claims) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(Json(service::connect::list_connections(&state, user_id).await?))
}

/// `DELETE /api/mcp/connections/{connector_id}` — disconnect the caller's connection.
pub async fn disconnect(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let outcome = service::connect::disconnect(&state, user_id, connector_id).await?;
    Ok(Json(json!({
        "message": outcome.message,
        "connector_id": outcome.connector_id,
        "composio_revoked": outcome.composio_revoked,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ComposioCallbackQuery {
    pub user_id: Option<Uuid>,
    pub connector_id: Option<Uuid>,
    pub success_url: Option<String>,
}

/// `GET /oauth/callback` — public Composio redirect target.
pub async fn oauth_callback(State(state): State<AppState>, Query(q): Query<ComposioCallbackQuery>) -> Response {
    match service::connect::composio_callback(&state, q.user_id, q.connector_id, q.success_url).await {
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
