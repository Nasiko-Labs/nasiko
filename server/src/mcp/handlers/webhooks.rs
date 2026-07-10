//! Composio webhook route (`POST /api/mcp/webhooks/composio`, public).
//!
//! Verifies the HMAC signature (fail-closed when no secret) against the raw body,
//! then delegates the effect to the service layer.

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use super::super::service;
use crate::state::AppState;

pub async fn composio(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let raw = String::from_utf8_lossy(&body).to_string();

    // Fail CLOSED: without a secret we can't distinguish Composio from anyone.
    let Some(secret) = &state.mcp.config.composio_webhook_secret else {
        tracing::error!("COMPOSIO_WEBHOOK_SECRET not set — refusing to process unauthenticated webhook");
        return (StatusCode::SERVICE_UNAVAILABLE, "webhook processing disabled").into_response();
    };

    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
    let (id, ts, sig) = (get("webhook-id"), get("webhook-timestamp"), get("webhook-signature"));
    if id.is_empty() || ts.is_empty() || sig.is_empty() {
        return (StatusCode::UNAUTHORIZED, "missing webhook signature headers").into_response();
    }
    if !service::webhooks::verify_signature(id, ts, &raw, sig, secret) {
        return (StatusCode::UNAUTHORIZED, "invalid webhook signature").into_response();
    }

    let payload: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON payload").into_response(),
    };

    match service::webhooks::process(&state, &payload).await {
        Ok(outcome) => {
            tracing::debug!(?outcome, "processed composio webhook");
            (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "composio webhook processing failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "status": "error" }))).into_response()
        }
    }
}
