//! Typed gateway errors → `(StatusCode, Json{"detail": <msg>})`.
//!
//! The `detail` strings are a wire contract: ops tooling and the agent's OpenAI SDK
//! key off them, so they must match `RUST_PLAN_V1.md` §3.2 exactly. In particular the
//! expiry message MUST contain the word "expired".

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// All error conditions the gateway surfaces to the caller.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    // ── 401 — authentication ────────────────────────────────────────────────
    #[error("Missing Authorization header")]
    MissingAuthHeader,
    #[error("Gateway JWT secret not configured")]
    JwtSecretNotConfigured,
    #[error("Agent token expired")]
    TokenExpired,
    #[error("Invalid agent token: {0}")]
    InvalidToken(String),
    #[error("Token missing agent_id claim")]
    MissingAgentId,

    // ── 403 — authenticated but unattributable (strict flow enforcement) ────
    // The agent's credential is valid — what's missing is authorization
    // context (a live flow the agent participates in). Deliberately NOT 401:
    // that would send well-behaved clients into credential-refresh loops.
    #[error("{0}")]
    Forbidden(String),

    // ── 400 — client-actionable request/resolution errors ───────────────────
    #[error("{0}")]
    BadRequest(String),
    #[error("No registry entry for agent_id={0}")]
    NoRegistryEntry(String),
    #[error("Secret '{0}' not found for owner_id={1}")]
    SecretNotFound(String, String),
    #[error("No api_key_secret_name set and no platform fallback key configured")]
    NoApiKey,

    // ── 502 — upstream provider (steps 4+) ──────────────────────────────────
    #[error("Upstream LLM error: {0}")]
    Upstream(String),
    #[error("{message}")]
    Provider { status: StatusCode, message: String },

    #[error("request body is over the router's {} MiB limit", limit / (1024 * 1024))]
    PayloadTooLarge { limit: usize },
    #[error("file storage is unavailable; try again")]
    StorageUnavailable,
    #[error("the router is handling too much data right now; try again")]
    Busy,

    // ── 500 — server-side fault (DB/crypto/config integrity) ─────────────────
    // Not client-actionable; the detail is logged but not exposed in the body.
    #[error("{0}")]
    Internal(String),
}

impl GatewayError {
    /// HTTP status for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            GatewayError::MissingAuthHeader
            | GatewayError::JwtSecretNotConfigured
            | GatewayError::TokenExpired
            | GatewayError::InvalidToken(_)
            | GatewayError::MissingAgentId => StatusCode::UNAUTHORIZED,
            GatewayError::Forbidden(_) => StatusCode::FORBIDDEN,
            GatewayError::BadRequest(_)
            | GatewayError::NoRegistryEntry(_)
            | GatewayError::SecretNotFound(_, _)
            | GatewayError::NoApiKey => StatusCode::BAD_REQUEST,
            GatewayError::Upstream(_) => StatusCode::BAD_GATEWAY,
            GatewayError::Provider { status, .. } => *status,
            GatewayError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            GatewayError::StorageUnavailable | GatewayError::Busy => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            GatewayError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let status = self.status();
        let full = self.to_string();
        // Log all 5xx in full server-side (§3.2).
        if status.is_server_error() {
            tracing::error!(%status, error = %full, "LLM router server error");
        }
        // Body: 4xx messages and the Upstream string are the wire contract and are
        // exposed. `Internal` may carry DB/crypto specifics → return a generic body.
        let detail = match &self {
            GatewayError::Internal(_) => "Internal server error".to_string(),
            _ => full,
        };
        (status, Json(json!({ "detail": detail }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_new_refusal_answers_with_its_own_status_and_says_why() {
        let cases = [
            (
                GatewayError::PayloadTooLarge {
                    limit: 48 * 1024 * 1024,
                },
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is over the router's 48 MiB limit",
            ),
            (
                GatewayError::StorageUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "file storage is unavailable; try again",
            ),
            (
                GatewayError::Busy,
                StatusCode::SERVICE_UNAVAILABLE,
                "the router is handling too much data right now; try again",
            ),
            (
                GatewayError::Provider {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    message: "Upstream LLM error: provider returned 422: x".into(),
                },
                StatusCode::UNPROCESSABLE_ENTITY,
                "Upstream LLM error: provider returned 422: x",
            ),
        ];
        for (error, status, detail) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.to_string(), detail);
        }
    }
}
