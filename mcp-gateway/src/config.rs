//! MCP gateway configuration.
//!
//! Derived from the platform's central [`nasiko_config::Config`] so there is a
//! single source of env parsing. The gateway crate deliberately depends on
//! `nasiko-config` (a leaf crate) rather than the server crate, keeping the
//! dependency direction acyclic (`server -> mcp-gateway`, never the reverse).

use nasiko_config::Config;

/// Runtime settings for the MCP gateway, copied out of the central `Config`.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Composio platform API key. `None` disables all Composio integration;
    /// generic MCP servers keep working.
    pub composio_api_key: Option<String>,
    /// Composio v3 HTTP API base URL (no trailing slash).
    pub composio_base_url: String,
    /// HMAC secret for verifying inbound Composio webhooks. `None` skips
    /// verification (dev only).
    pub composio_webhook_secret: Option<String>,
    /// Public URL of this gateway, injected into agents as `MCP_GATEWAY_URL`.
    pub gateway_public_url: Option<String>,
    /// TTL for the Redis-cached resolved backend/session list.
    pub session_ttl_seconds: u64,
    /// TTL for the Redis-cached per-agent permission context.
    pub perm_cache_ttl_seconds: u64,
    /// TTL for the Redis-cached aggregated tool manifest.
    pub manifest_ttl_seconds: u64,
    /// HMAC key for signing MCP OAuth 2.1 `state`. Reuses `OAUTH_STATE_SIGNING_KEY`
    /// (the same signer GitHub OAuth uses); falls back to a dev constant.
    pub oauth_state_signing_key: String,
}

impl McpConfig {
    /// Build the MCP config from the platform config.
    pub fn from_config(config: &Config) -> Self {
        Self {
            composio_api_key: config.composio_api_key.clone(),
            // Normalize away any trailing slash so URL joins are predictable.
            composio_base_url: config.composio_base_url.trim_end_matches('/').to_string(),
            composio_webhook_secret: config.composio_webhook_secret.clone(),
            gateway_public_url: config.mcp_gateway_public_url.clone(),
            session_ttl_seconds: config.mcp_session_ttl_seconds,
            perm_cache_ttl_seconds: config.mcp_perm_cache_ttl_seconds,
            manifest_ttl_seconds: config.mcp_manifest_ttl_seconds,
            // Never fall back to a shipped constant (this file syncs to the public
            // repo). Use the dedicated key if set, else derive from the already-
            // required JWT_SECRET with domain separation. Panics only if BOTH are
            // unset — impossible in a valid deployment (JWT_SECRET is required).
            oauth_state_signing_key: std::env::var("OAUTH_STATE_SIGNING_KEY")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    std::env::var("JWT_SECRET").ok().filter(|s| !s.is_empty()).map(|j| format!("mcp-oauth-state::{j}"))
                })
                .expect("OAUTH_STATE_SIGNING_KEY or JWT_SECRET must be set for MCP OAuth state signing"),
        }
    }

    /// True when Composio integration is configured (an API key is present).
    pub fn composio_enabled(&self) -> bool {
        self.composio_api_key.is_some()
    }

    /// The browser-facing OAuth 2.1 callback URL, derived from the gateway's
    /// public URL: `{MCP_GATEWAY_PUBLIC_URL}/oauth/callback`. `None` when the
    /// public URL is unset (MCP OAuth cannot run without a reachable redirect).
    pub fn oauth_redirect_uri(&self) -> Option<String> {
        self.gateway_public_url
            .as_ref()
            .map(|base| format!("{}/oauth/callback", base.trim_end_matches('/')))
    }
}
