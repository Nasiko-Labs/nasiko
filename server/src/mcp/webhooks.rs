//! Composio webhook route (`POST /api/mcp/webhooks/composio`, public).
//!
//! Verifies the HMAC signature (when a secret is configured) against the raw
//! body, then delegates the effect to the crate's `webhooks::process_event`.

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use nasiko_mcp_gateway::webhooks;

use crate::state::AppState;

pub async fn composio(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let raw = String::from_utf8_lossy(&body).to_string();

    // Fail CLOSED, not open: this is a public, unauthenticated-by-default route
    // whose effect is "mark a connection EXPIRED" for whatever account_id the
    // caller names. Without a configured secret there is no way to distinguish
    // Composio from anyone else on the internet, so the endpoint must refuse to
    // process events rather than silently trust unauthenticated input.
    let Some(secret) = &state.mcp.config.composio_webhook_secret else {
        tracing::error!("COMPOSIO_WEBHOOK_SECRET not set — refusing to process unauthenticated webhook");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "webhook processing disabled: COMPOSIO_WEBHOOK_SECRET is not configured",
        )
            .into_response();
    };

    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
    let (id, ts, sig) = (
        get("webhook-id"),
        get("webhook-timestamp"),
        get("webhook-signature"),
    );
    if id.is_empty() || ts.is_empty() || sig.is_empty() {
        return (StatusCode::UNAUTHORIZED, "missing webhook signature headers").into_response();
    }
    if !webhooks::verify_signature(id, ts, &raw, sig, secret) {
        return (StatusCode::UNAUTHORIZED, "invalid webhook signature").into_response();
    }

    let payload: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON payload").into_response(),
    };

    match webhooks::process_event(&state.mcp, &payload).await {
        Ok(outcome) => {
            tracing::debug!(?outcome, "processed composio webhook");
            (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
        }
        Err(e) => {
            // 5xx so Composio retries the delivery.
            tracing::error!(error = %e, "composio webhook processing failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "status": "error" }))).into_response()
        }
    }
}
