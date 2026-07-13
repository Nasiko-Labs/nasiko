//! Gateway error type.
//!
//! This crate is axum-free (route handlers live in `oss/server/src/mcp/`), so
//! `McpError` maps itself to **both** a JSON-RPC error object (for the agent
//! `/api/mcp` path) and an HTTP status code (for the management routes). The
//! server module turns these into concrete `axum` responses.

use thiserror::Error;

use crate::types::{JsonRpcError, codes};

pub type Result<T> = std::result::Result<T, McpError>;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("conflict: {0}")]
    Conflict(String),

    /// A tool is blocked for this agent (permission stance = block).
    #[error("{0}")]
    ToolBlocked(String),

    /// A tool requires user approval (permission stance = ask).
    #[error("{0}")]
    ToolApprovalRequired(String),

    /// A backend (Composio or generic MCP server) failed or was unreachable.
    #[error("backend error: {0}")]
    Backend(String),

    /// Composio-specific upstream failure.
    #[error("composio error: {0}")]
    Composio(String),

    /// OAuth 2.1 discovery / token exchange failure.
    #[error("oauth error: {0}")]
    Oauth(String),

    /// Feature disabled by config (e.g. Composio API key not set).
    #[error("not configured: {0}")]
    NotConfigured(String),

    /// Secret encryption/decryption failure.
    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("cache error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl McpError {
    /// The JSON-RPC error code for this error, used on the agent `/api/mcp` path.
    pub fn json_rpc_code(&self) -> i64 {
        match self {
            McpError::BadRequest(_) | McpError::Conflict(_) => codes::INVALID_PARAMS,
            McpError::NotFound(_) => codes::METHOD_NOT_FOUND,
            McpError::ToolBlocked(_) | McpError::Forbidden(_) => codes::TOOL_BLOCKED,
            McpError::ToolApprovalRequired(_) => codes::TOOL_ASK,
            _ => codes::INTERNAL_ERROR,
        }
    }

    /// The HTTP status code for this error, used on the management routes.
    pub fn http_status(&self) -> u16 {
        match self {
            McpError::BadRequest(_) => 400,
            McpError::Unauthorized(_) => 401,
            McpError::Forbidden(_) | McpError::ToolBlocked(_) => 403,
            McpError::NotFound(_) => 404,
            McpError::Conflict(_) => 409,
            McpError::ToolApprovalRequired(_) => 428, // Precondition Required
            McpError::NotConfigured(_) => 503,
            McpError::Backend(_) | McpError::Composio(_) | McpError::Oauth(_) => 502,
            _ => 500,
        }
    }

    /// Render as a JSON-RPC error object. Server-side (5xx) causes are logged
    /// and the client-facing message is kept generic to avoid leaking internals.
    pub fn to_json_rpc(&self) -> JsonRpcError {
        let code = self.json_rpc_code();
        let message = match self {
            McpError::Database(e) => {
                tracing::error!(error = %e, "mcp database error");
                "internal error".to_string()
            }
            McpError::Redis(e) => {
                tracing::error!(error = %e, "mcp cache error");
                "internal error".to_string()
            }
            McpError::Http(e) => {
                tracing::error!(error = %e, "mcp http error");
                "backend request failed".to_string()
            }
            McpError::Serde(e) => {
                tracing::error!(error = %e, "mcp serialization error");
                "internal error".to_string()
            }
            McpError::Crypto(m) => {
                tracing::error!(message = %m, "mcp crypto error");
                "internal error".to_string()
            }
            McpError::Internal(m) => {
                tracing::error!(message = %m, "mcp internal error");
                "internal error".to_string()
            }
            // Backend detail may name internal endpoints — keep generic on the
            // agent-facing JSON-RPC path (detail is preserved for management via
            // `client_message`).
            McpError::Backend(m) | McpError::Composio(m) => {
                tracing::warn!(message = %m, "mcp backend error");
                "backend request failed".to_string()
            }
            McpError::Oauth(m) => {
                tracing::warn!(message = %m, "mcp oauth error");
                "authorization error".to_string()
            }
            other => other.to_string(),
        };
        JsonRpcError::new(code, message)
    }

    /// The client-facing message for HTTP management responses. Server-side
    /// causes are logged and reduced to a generic message.
    pub fn client_message(&self) -> String {
        match self {
            McpError::Database(e) => {
                tracing::error!(error = %e, "mcp database error");
                "internal error".to_string()
            }
            McpError::Redis(e) => {
                tracing::error!(error = %e, "mcp cache error");
                "internal error".to_string()
            }
            McpError::Serde(e) => {
                tracing::error!(error = %e, "mcp serialization error");
                "internal error".to_string()
            }
            McpError::Crypto(m) => {
                tracing::error!(message = %m, "mcp crypto error");
                "internal error".to_string()
            }
            // OAuth discovery/exchange can reflect attacker-influenced endpoint
            // URLs or token-endpoint response bodies — redact on every surface.
            McpError::Oauth(m) => {
                tracing::warn!(message = %m, "mcp oauth error");
                "authorization error".to_string()
            }
            McpError::Internal(m) => {
                tracing::error!(message = %m, "mcp internal error");
                "internal error".to_string()
            }
            // Backend/Composio detail is kept for management responses (e.g.
            // "could not reach MCP server") — safe, user-facing diagnostics.
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_mapping() {
        assert_eq!(McpError::BadRequest("x".into()).http_status(), 400);
        assert_eq!(McpError::Unauthorized("x".into()).http_status(), 401);
        assert_eq!(McpError::Forbidden("x".into()).http_status(), 403);
        assert_eq!(McpError::ToolBlocked("x".into()).http_status(), 403);
        assert_eq!(McpError::NotFound("x".into()).http_status(), 404);
        assert_eq!(McpError::Conflict("x".into()).http_status(), 409);
        assert_eq!(McpError::ToolApprovalRequired("x".into()).http_status(), 428);
        assert_eq!(McpError::NotConfigured("x".into()).http_status(), 503);
        assert_eq!(McpError::Backend("x".into()).http_status(), 502);
        assert_eq!(McpError::Internal("x".into()).http_status(), 500);
    }

    #[test]
    fn json_rpc_code_mapping() {
        assert_eq!(McpError::BadRequest("x".into()).json_rpc_code(), codes::INVALID_PARAMS);
        assert_eq!(McpError::NotFound("x".into()).json_rpc_code(), codes::METHOD_NOT_FOUND);
        assert_eq!(McpError::ToolBlocked("x".into()).json_rpc_code(), codes::TOOL_BLOCKED);
        assert_eq!(McpError::ToolApprovalRequired("x".into()).json_rpc_code(), codes::TOOL_ASK);
        assert_eq!(McpError::Forbidden("x".into()).json_rpc_code(), codes::TOOL_BLOCKED);
        assert_eq!(McpError::Conflict("x".into()).json_rpc_code(), codes::INVALID_PARAMS);
        assert_eq!(McpError::Internal("x".into()).json_rpc_code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn internal_causes_are_redacted_client_facing() {
        // Server-side error detail must not leak to the JSON-RPC client.
        let msg = McpError::Internal("secret db dsn".into()).to_json_rpc().message;
        assert_eq!(msg, "internal error");
        // But client-safe errors keep their message.
        assert_eq!(McpError::ToolBlocked("blocked!".into()).to_json_rpc().message, "blocked!");
    }

    #[test]
    fn sensitive_variants_are_redacted_on_agent_path() {
        // Crypto detail must never reach the agent, on either surface.
        assert_eq!(McpError::Crypto("decrypt failed for key X".into()).to_json_rpc().message, "internal error");
        assert_eq!(McpError::Crypto("decrypt failed for key X".into()).client_message(), "internal error");
        // Backend/Composio/Oauth detail is redacted on the agent JSON-RPC path.
        assert_eq!(McpError::Backend("upstream at 10.0.1.5 refused".into()).to_json_rpc().message, "backend request failed");
        assert_eq!(McpError::Composio("composio 500".into()).to_json_rpc().message, "backend request failed");
        assert_eq!(McpError::Oauth("bad token endpoint".into()).to_json_rpc().message, "authorization error");
        // Oauth is redacted on the management surface too (can reflect discovered URLs / bodies).
        assert_eq!(McpError::Oauth("token exchange failed (HTTP 500): <body>".into()).client_message(), "authorization error");
        // …but Backend detail is preserved for management responses (safe diagnostics).
        assert_eq!(McpError::Backend("could not reach MCP server".into()).client_message(), "backend error: could not reach MCP server");
    }

    /// Table-driven sweep of `client_message()` across every `McpError` variant:
    /// which ones redact server-side detail vs which pass their message through.
    #[test]
    fn client_message_redaction_table_for_every_variant() {
        // Pass-through variants (Display detail is safe for the management surface).
        assert_eq!(McpError::NotFound("conn x".into()).client_message(), "not found: conn x");
        assert_eq!(McpError::BadRequest("bad url".into()).client_message(), "bad request: bad url");
        assert_eq!(McpError::Unauthorized("no token".into()).client_message(), "unauthorized: no token");
        assert_eq!(McpError::Forbidden("no access".into()).client_message(), "forbidden: no access");
        assert_eq!(McpError::Conflict("dup".into()).client_message(), "conflict: dup");
        assert_eq!(McpError::ToolBlocked("blocked!".into()).client_message(), "blocked!");
        assert_eq!(McpError::ToolApprovalRequired("ask!".into()).client_message(), "ask!");
        assert_eq!(McpError::NotConfigured("no key".into()).client_message(), "not configured: no key");
        assert_eq!(
            McpError::Backend("upstream at 10.0.1.5 refused".into()).client_message(),
            "backend error: upstream at 10.0.1.5 refused"
        );
        assert_eq!(McpError::Composio("composio 500".into()).client_message(), "composio error: composio 500");

        // Fix #7: `McpError::Oauth` is now redacted on the management surface too —
        // its message is built from attacker-influenced discovered URLs / token-
        // endpoint response bodies, so it must not pass through.
        assert_eq!(
            McpError::Oauth("token exchange failed (HTTP 502): <internal-host-detail>".into()).client_message(),
            "authorization error"
        );

        // Redacted variants — server-side cause logged, client gets a generic message.
        assert_eq!(McpError::Database(sqlx::Error::RowNotFound).client_message(), "internal error");
        let io_err = std::io::Error::other("boom");
        let redis_err: redis::RedisError = io_err.into();
        assert_eq!(McpError::Redis(redis_err).client_message(), "internal error");
        let serde_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        assert_eq!(McpError::Serde(serde_err).client_message(), "internal error");
        assert_eq!(McpError::Crypto("decrypt failed for key X".into()).client_message(), "internal error");
        assert_eq!(McpError::Internal("secret db dsn".into()).client_message(), "internal error");
    }

    /// `client_message()` has no arm for `McpError::Http` — it falls into the
    /// generic `other => other.to_string()` branch, so a raw `reqwest::Error`
    /// reaches management callers verbatim, unlike `to_json_rpc()` which redacts
    /// it. Characterizes current behavior (candidate follow-up hardening).
    #[tokio::test]
    async fn http_variant_is_not_redacted_by_client_message_unlike_json_rpc() {
        let err1 = reqwest::Client::new().get("http://127.0.0.1:1/").send().await.unwrap_err();
        let json_rpc_message = McpError::Http(err1).to_json_rpc().message;
        // reqwest::Error isn't Clone — fetch a fresh error for the second check.
        let err2 = reqwest::Client::new().get("http://127.0.0.1:1/").send().await.unwrap_err();
        let client_message = McpError::Http(err2).client_message();

        assert_eq!(json_rpc_message, "backend request failed");
        assert_ne!(client_message, "backend request failed");
        assert_ne!(client_message, "internal error");
    }
}
