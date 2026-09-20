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
    /// This is the in-cluster URL agents use to reach the gateway.
    pub gateway_public_url: Option<String>,
    /// Base URL for the generic-connector OAuth 2.1 browser redirect —
    /// distinct from `gateway_public_url` on purpose, see
    /// `Config::mcp_oauth_redirect_base_url`'s doc comment for why. Falls
    /// back to `gateway_public_url` when unset.
    pub oauth_redirect_base_url: Option<String>,
    /// Browser-reachable base URL for Composio OAuth callbacks specifically
    /// (`oss/mcp-gateway/src/connect.rs`). Falls back to `gateway_public_url`
    /// when unset.
    pub composio_callback_base_url: Option<String>,
    /// TTL for the Redis-cached resolved backend/session list.
    pub session_ttl_seconds: u64,
    /// TTL for the Redis-cached per-agent permission context.
    pub perm_cache_ttl_seconds: u64,
    /// TTL for the Redis-cached aggregated tool manifest.
    pub manifest_ttl_seconds: u64,
    /// TTL for the Redis-cached Composio toolkit tool count.
    pub toolcount_ttl_seconds: u64,
    /// HMAC key for signing MCP OAuth 2.1 `state`. Reuses `OAUTH_STATE_SIGNING_KEY`
    /// (the same signer GitHub OAuth uses); falls back to a dev constant.
    pub oauth_state_signing_key: String,
    /// Model for the description-backfill LLM fallback (`description_backfill.rs`).
    pub description_model: String,
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
            oauth_redirect_base_url: config.mcp_oauth_redirect_base_url.clone(),
            composio_callback_base_url: config.composio_callback_base_url.clone(),
            session_ttl_seconds: config.mcp_session_ttl_seconds,
            perm_cache_ttl_seconds: config.mcp_perm_cache_ttl_seconds,
            manifest_ttl_seconds: config.mcp_manifest_ttl_seconds,
            toolcount_ttl_seconds: config.mcp_toolcount_ttl_seconds,
            // Never fall back to a shipped constant (this file syncs to the public
            // repo). Use the dedicated key if set, else derive from the already-
            // required JWT_SECRET with domain separation. Panics only if BOTH are
            // unset — impossible in a valid deployment (JWT_SECRET is required).
            oauth_state_signing_key: std::env::var("OAUTH_STATE_SIGNING_KEY")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    std::env::var("JWT_SECRET")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .map(|j| format!("mcp-oauth-state::{j}"))
                })
                .expect(
                    "OAUTH_STATE_SIGNING_KEY or JWT_SECRET must be set for MCP OAuth state signing",
                ),
            description_model: config.mcp_description_model.clone(),
        }
    }

    /// True when Composio integration is configured (an API key is present).
    pub fn composio_enabled(&self) -> bool {
        self.composio_api_key.is_some()
    }

    /// The browser-facing OAuth 2.1 callback URL: `{base}/oauth/callback`,
    /// where `base` is `MCP_OAUTH_REDIRECT_BASE_URL` if set, else
    /// `MCP_GATEWAY_PUBLIC_URL`. `None` when neither is set (MCP OAuth cannot
    /// run without a reachable redirect).
    ///
    /// These two are kept separate on purpose: `gateway_public_url` is told to
    /// agent *containers* (may be a Docker-internal address like
    /// `host.docker.internal`), but this URL is opened in the *user's own
    /// browser* and sent to real OAuth providers' Dynamic Client Registration
    /// endpoints, which commonly reject anything that isn't HTTPS or a genuine
    /// loopback address — confirmed live against Notion's DCR endpoint
    /// (`"Redirect URI must use HTTPS unless it is a loopback HTTP URI"`).
    pub fn oauth_redirect_uri(&self) -> Option<String> {
        self.oauth_redirect_base_url
            .as_ref()
            .or(self.gateway_public_url.as_ref())
            .map(|base| format!("{}/oauth/callback", base.trim_end_matches('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> McpConfig {
        McpConfig {
            composio_api_key: None,
            composio_base_url: "https://backend.composio.dev".to_string(),
            composio_webhook_secret: None,
            gateway_public_url: None,
            oauth_redirect_base_url: None,
            composio_callback_base_url: None,
            session_ttl_seconds: 60,
            perm_cache_ttl_seconds: 30,
            manifest_ttl_seconds: 300,
            toolcount_ttl_seconds: 3600,
            oauth_state_signing_key: "test".to_string(),
            description_model: "gpt-4o-mini".to_string(),
        }
    }

    #[test]
    fn oauth_redirect_uri_is_none_when_neither_url_is_set() {
        assert_eq!(base_config().oauth_redirect_uri(), None);
    }

    #[test]
    fn oauth_redirect_uri_falls_back_to_gateway_public_url_when_unset() {
        let cfg = McpConfig {
            gateway_public_url: Some("http://host.docker.internal:8080/api/mcp".into()),
            ..base_config()
        };
        assert_eq!(
            cfg.oauth_redirect_uri().as_deref(),
            Some("http://host.docker.internal:8080/api/mcp/oauth/callback")
        );
    }

    #[test]
    fn oauth_redirect_uri_prefers_dedicated_base_over_gateway_public_url() {
        // The exact scenario this field exists for: MCP_GATEWAY_PUBLIC_URL is a
        // Docker-internal address agent containers need, but real OAuth
        // providers reject it as a redirect_uri — the dedicated base must win.
        let cfg = McpConfig {
            gateway_public_url: Some("http://host.docker.internal:8080/api/mcp".into()),
            oauth_redirect_base_url: Some("http://localhost:8080/api/mcp".into()),
            ..base_config()
        };
        assert_eq!(
            cfg.oauth_redirect_uri().as_deref(),
            Some("http://localhost:8080/api/mcp/oauth/callback")
        );
    }

    #[test]
    fn oauth_redirect_uri_trims_trailing_slash_on_either_source() {
        let cfg = McpConfig {
            oauth_redirect_base_url: Some("http://localhost:8080/".into()),
            ..base_config()
        };
        assert_eq!(
            cfg.oauth_redirect_uri().as_deref(),
            Some("http://localhost:8080/oauth/callback")
        );
    }
}
