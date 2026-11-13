//! Typed gateway errors → `(StatusCode, Json{"detail": <msg>})`.
//!
//! The `detail` strings are a wire contract: ops tooling and the agent's OpenAI SDK
//! key off them, so they must match `RUST_PLAN_V1.md` §3.2 exactly. In particular the
//! expiry message MUST contain the word "expired".

use axum::Json;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
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
    /// The agent is shared with the calling user, so it runs on their own LLM config, and they
    /// have none. Worded for the end user: an agent relays it into the chat.
    #[error(
        "Connect a model in LLM router to chat with this agent. It is shared with you, so it \
         runs on your own default LLM config, and you have not set one."
    )]
    NoSharedUserConfig,

    // ── 429 - the owner's allowance for this caller is spent ────────────────
    /// A call the agent's owner would pay for was refused until the allowance renews, and the
    /// caller has no config of their own to fall back on. Worded for the end user, like
    /// [`NoSharedUserConfig`](Self::NoSharedUserConfig); `Retry-After` carries the seconds.
    #[error(
        "Today's calls on this agent's own model are used up. Connect your own model in LLM \
         router to keep chatting with it, or try again after 00:00 UTC."
    )]
    OwnerAllowanceSpent { retry_after_secs: u64 },

    // ── 502 — upstream provider (steps 4+) ──────────────────────────────────
    #[error("Upstream LLM error: {0}")]
    Upstream(String),

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
            | GatewayError::NoApiKey
            | GatewayError::NoSharedUserConfig => StatusCode::BAD_REQUEST,
            GatewayError::OwnerAllowanceSpent { .. } => StatusCode::TOO_MANY_REQUESTS,
            GatewayError::Upstream(_) => StatusCode::BAD_GATEWAY,
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
        let mut response = (status, Json(json!({ "detail": detail }))).into_response();
        if let GatewayError::OwnerAllowanceSpent { retry_after_secs } = self {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
        }
        response
    }
}
