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
    #[error("Agent token expired")]
    TokenExpired,
    #[error("Invalid agent token: {0}")]
    InvalidToken(String),
    #[error("Token missing agent_id claim")]
    MissingAgentId,
    /// The owner revoked the agent's LLM routing tokens after this one was issued — what
    /// `nasiko disconnect <coding-agent>` does. 401 so a client refreshes its credential: a
    /// still-connected one gets a fresh token, a disconnected one learns it was disconnected.
    #[error(
        "Nasiko LLM routing was disconnected for this agent; restart the coding agent to use its own provider, or reconnect it"
    )]
    TokenRevoked,

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
    /// The LLM config names a user secret the owner never stored. Worded for the end user: an
    /// agent relays it into the chat. The owner id stays out of the message.
    #[error(
        "Your LLM config uses the secret '{0}', which doesn't exist. Add it in Settings → \
         Secrets or edit the config."
    )]
    SecretNotFound(String),
    /// The owner's stored secret exists but this server can't decrypt it (the server's
    /// encryption key changed since it was saved). Saving it again is the fix.
    #[error(
        "Your stored API key '{0}' can't be decrypted on this server. Save it again in \
         Settings → Secrets."
    )]
    SecretUnreadable(String),
    /// No per-user key and no platform key for the provider the call resolved to.
    #[error("{}", no_api_key_message(.provider, .model))]
    NoApiKey { provider: String, model: String },
    /// The agent is shared with the calling user, so it runs on their own LLM config, and they
    /// have none. Worded for the end user: an agent relays it into the chat.
    #[error(
        "Connect a model in LLM router to chat with this agent. It is shared with you, so it \
         runs on your own default LLM config, and you have not set one."
    )]
    NoSharedUserConfig,

    // ── 429 - the owner's allowance for this caller is spent ────────────────
    /// A call the deployment would cover (the agent's owner paying for a shared caller, or the
    /// platform for an owner's own call that found no key) was refused until the allowance renews,
    /// and the caller has no config of their own to fall back on. Worded for the end user, like
    /// [`NoSharedUserConfig`](Self::NoSharedUserConfig); `Retry-After` carries the seconds.
    #[error(
        "Today's free model calls are used up. Connect your own model in LLM router to keep \
         going, or try again after 00:00 UTC."
    )]
    OwnerAllowanceSpent { retry_after_secs: u64 },

    // ── 502 — upstream provider (steps 4+) ──────────────────────────────────
    #[error("Upstream LLM error: {0}")]
    Upstream(String),
    #[error("{message}")]
    Provider { status: StatusCode, message: String },
    /// No model answered in time: 504, so the caller can tell a slow provider from a broken one.
    #[error("Upstream LLM error: {0}")]
    UpstreamTimeout(String),

    #[error("request body is over the router's {} MiB limit", limit / (1024 * 1024))]
    PayloadTooLarge { limit: usize },
    #[error("file storage is unavailable; try again")]
    StorageUnavailable,
    #[error("the router is handling too much data right now; try again")]
    Busy,
    /// Fail closed: without the secret no agent token can be verified. A 503, not a 401, so
    /// agent SDKs don't read a server misconfiguration as a bad API key.
    #[error("LLM router misconfigured: AGENT_JWT_SECRET is not set on the server.")]
    JwtSecretNotConfigured,

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
            | GatewayError::TokenExpired
            | GatewayError::InvalidToken(_)
            | GatewayError::MissingAgentId
            | GatewayError::TokenRevoked => StatusCode::UNAUTHORIZED,
            GatewayError::Forbidden(_) => StatusCode::FORBIDDEN,
            GatewayError::BadRequest(_)
            | GatewayError::NoRegistryEntry(_)
            | GatewayError::SecretNotFound(_)
            | GatewayError::SecretUnreadable(_)
            | GatewayError::NoApiKey { .. }
            | GatewayError::NoSharedUserConfig => StatusCode::BAD_REQUEST,
            GatewayError::OwnerAllowanceSpent { .. } => StatusCode::TOO_MANY_REQUESTS,
            GatewayError::Upstream(_) => StatusCode::BAD_GATEWAY,
            GatewayError::Provider { status, .. } => *status,
            GatewayError::UpstreamTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
            GatewayError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            GatewayError::StorageUnavailable
            | GatewayError::Busy
            | GatewayError::JwtSecretNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
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
        let mut response = (status, error_body(status, &detail)).into_response();
        if let GatewayError::OwnerAllowanceSpent { retry_after_secs } = self {
            mark_allowance_spent(&mut response, retry_after_secs);
        }
        response
    }
}

/// On a response that refused a call with [`GatewayError::OwnerAllowanceSpent`]: lets a layer
/// around the router tell that refusal from a provider's own `429` without reading the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowanceSpent {
    pub retry_after_secs: u64,
}

/// `Retry-After` for the caller and [`AllowanceSpent`] for the layers around the router.
pub(crate) fn mark_allowance_spent(response: &mut Response, retry_after_secs: u64) {
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
    response
        .extensions_mut()
        .insert(AllowanceSpent { retry_after_secs });
}

/// The environment variable that holds the platform key for a built-in provider, or `None`
/// for a provider the platform has no key setting for.
pub(crate) fn platform_key_env(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("PLATFORM_OPENAI_API_KEY"),
        "anthropic" => Some("PLATFORM_ANTHROPIC_API_KEY"),
        "gemini" => Some("PLATFORM_GEMINI_API_KEY"),
        "openrouter" => Some("PLATFORM_OPENROUTER_API_KEY"),
        _ => None,
    }
}

fn no_api_key_message(provider: &str, model: &str) -> String {
    let admin_fix = match platform_key_env(provider) {
        Some(env) => format!(", or ask your admin to set {env}"),
        None => String::new(),
    };
    format!(
        "No API key for provider '{provider}' (model {model}). Connect a model with your own \
         key in LLM router{admin_fix}."
    )
}

pub(crate) fn error_body(status: StatusCode, message: &str) -> Json<serde_json::Value> {
    let kind = match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        s if s.is_server_error() => "api_error",
        _ => "invalid_request_error",
    };
    Json(json!({ "detail": message, "error": { "message": message, "type": kind } }))
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
            (
                GatewayError::UpstreamTimeout("model m did not answer within 5.0s".into()),
                StatusCode::GATEWAY_TIMEOUT,
                "Upstream LLM error: model m did not answer within 5.0s",
            ),
        ];
        for (error, status, detail) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.to_string(), detail);
        }
    }

    async fn body_of(error: GatewayError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn a_refusal_reaches_openai_sdks_as_error_message_and_keeps_detail() {
        let reason = "Anthropic does not accept this input: 'application/zip' documents (it reads PDF and text documents)";
        let (status, body) = body_of(GatewayError::BadRequest(reason.into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({
                "detail": reason,
                "error": {"message": reason, "type": "invalid_request_error"}
            })
        );
    }

    #[tokio::test]
    async fn each_status_class_names_its_sdk_error_type() {
        let cases = [
            (GatewayError::TokenExpired, "authentication_error"),
            (
                GatewayError::Forbidden("no flow".into()),
                "permission_error",
            ),
            (
                GatewayError::PayloadTooLarge { limit: 1024 * 1024 },
                "request_too_large",
            ),
            (
                GatewayError::Provider {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    message: "slow down".into(),
                },
                "rate_limit_error",
            ),
            (
                GatewayError::Provider {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    message: "bad".into(),
                },
                "invalid_request_error",
            ),
            (GatewayError::Upstream("down".into()), "api_error"),
            (GatewayError::StorageUnavailable, "api_error"),
        ];
        for (error, kind) in cases {
            let expected = error.to_string();
            let (_, body) = body_of(error).await;
            assert_eq!(body["error"]["type"], kind, "{expected}");
            assert_eq!(body["error"]["message"], expected);
            assert_eq!(body["detail"], expected);
        }
    }

    #[test]
    fn a_key_refusal_names_the_provider_the_model_and_the_fix() {
        let error = GatewayError::NoApiKey {
            provider: "anthropic".into(),
            model: "claude-sonnet".into(),
        };
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            "No API key for provider 'anthropic' (model claude-sonnet). Connect a model with your \
             own key in LLM router, or ask your admin to set PLATFORM_ANTHROPIC_API_KEY."
        );
        let no_platform_setting = GatewayError::NoApiKey {
            provider: "deepseek".into(),
            model: "r1".into(),
        };
        assert_eq!(
            no_platform_setting.to_string(),
            "No API key for provider 'deepseek' (model r1). Connect a model with your own key in \
             LLM router."
        );
    }

    #[test]
    fn a_missing_or_unreadable_secret_says_which_and_never_names_the_owner() {
        let missing = GatewayError::SecretNotFound("ANTHROPIC_API_KEY".into());
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            missing.to_string(),
            "Your LLM config uses the secret 'ANTHROPIC_API_KEY', which doesn't exist. Add it in \
             Settings → Secrets or edit the config."
        );
        let unreadable = GatewayError::SecretUnreadable("ANTHROPIC_API_KEY".into());
        assert_eq!(unreadable.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            unreadable.to_string(),
            "Your stored API key 'ANTHROPIC_API_KEY' can't be decrypted on this server. Save it \
             again in Settings → Secrets."
        );
    }

    #[tokio::test]
    async fn a_missing_jwt_secret_is_a_503_that_names_the_setting() {
        let (status, body) = body_of(GatewayError::JwtSecretNotConfigured).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body["detail"],
            "LLM router misconfigured: AGENT_JWT_SECRET is not set on the server."
        );
        assert_eq!(body["error"]["type"], "api_error");
    }

    #[tokio::test]
    async fn an_internal_error_hides_its_cause_in_both_fields() {
        let (status, body) = body_of(GatewayError::Internal("db password=x".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["detail"], "Internal server error");
        assert_eq!(body["error"]["message"], "Internal server error");
        assert!(!body.to_string().contains("password"));
    }

    #[test]
    fn only_a_spent_allowance_is_marked_as_one() {
        let spent = GatewayError::OwnerAllowanceSpent {
            retry_after_secs: 90,
        }
        .into_response();
        assert_eq!(spent.headers()[RETRY_AFTER], "90");
        assert_eq!(
            spent.extensions().get::<AllowanceSpent>(),
            Some(&AllowanceSpent {
                retry_after_secs: 90
            })
        );

        // A provider's own 429 is a rate limit, not the allowance.
        let provider = GatewayError::Provider {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "slow down".into(),
        }
        .into_response();
        assert!(provider.extensions().get::<AllowanceSpent>().is_none());
    }
}
