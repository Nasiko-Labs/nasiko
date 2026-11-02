//! MCP gateway configuration.
//!
//! Derived from the platform's central [`nasiko_config::Config`] so there is a
//! single source of env parsing. The gateway crate deliberately depends on
//! `nasiko-config` (a leaf crate) rather than the server crate, keeping the
//! dependency direction acyclic (`server -> mcp-gateway`, never the reverse).

use nasiko_config::Config;

/// Default `initialize.instructions` sentence when `MCP_GATEWAY_INSTRUCTIONS`
/// is unset or blank — keeps the field non-empty on an unconfigured deployment.
pub(crate) const DEFAULT_GATEWAY_INSTRUCTIONS: &str = "You are connected to the Nasiko MCP gateway. \
    Tools listed here are the platform's connectors; call them directly by name. To find a tool \
    that is not listed, use nasiko_search_tools, then call the found tool by name — or through \
    nasiko_call_tool if your client can only call listed tools.";

/// Tool search mode — selects the `ToolSearchIndex` implementation wired at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSearchMode {
    /// Flat embedding-based cosine similarity (default). Falls back to `Keyword`
    /// at startup when `OPENAI_API_KEY` is not set.
    Semantic,
    /// Flat BM25 keyword search — no external dependencies.
    Keyword,
    /// Disabled: eager fan-out, return all tools (rollback path).
    None,
}

impl ToolSearchMode {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "semantic" => Self::Semantic,
            "keyword" => Self::Keyword,
            "none" => Self::None,
            _ => {
                tracing::warn!(value = %s, "unknown MCP_TOOL_SEARCH_MODE, defaulting to semantic");
                Self::Semantic
            }
        }
    }
}

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
    /// How long a pending `mcp_tool`-origin `hitl_requests` row (an `auth_required`/
    /// `tool_approval` pause this gateway creates) stays answerable before the dispatcher expires
    /// it — `Config::hitl_request_ttl_days` (env: `HITL_REQUEST_TTL_DAYS`), the same knob every
    /// other `HitlKind` already honors via `PgHitlStore::with_ttl_days`. Passed to
    /// `nasiko_hitl::repo::create_pending_auth_required_with_ttl`/
    /// `create_pending_tool_approval_with_ttl` rather than their fixed-7-day counterparts (found
    /// in review: setting the env var used to have no effect on anything this gateway created).
    pub hitl_request_ttl_days: i64,
    /// Tool search mode — selects the search index implementation.
    pub tool_search_mode: ToolSearchMode,
    /// Max tools returned by query-aware `tools/list`.
    pub tool_search_tool_limit: usize,
    /// Max tools returned by `nasiko_search_tools` meta-tool.
    pub tool_search_meta_limit: usize,
    /// OpenAI API key for embedding queries (semantic mode).
    pub openai_api_key: Option<String>,
    /// Embedding model name (e.g. `text-embedding-3-small`).
    pub embedding_model: String,
    /// The gateway's own sentence advertised in `initialize.instructions`,
    /// ahead of any per-connector instructions. MCP_GATEWAY_INSTRUCTIONS,
    /// defaults to a generic orientation sentence so the field is never
    /// blank on an unconfigured deployment.
    pub gateway_instructions: String,
    /// HMAC key `identity::SignedIdentity::sign`/`verify` use for the
    /// `x-nasiko-identity` header forwarded to every system backend — see
    /// `identity.rs`'s module doc. Derived from `Config::mcp_identity_signing_key`
    /// (itself `MCP_IDENTITY_SIGNING_KEY`, falling back to `JWT_SECRET`); never
    /// empty past `from_config`, which panics rather than sign with no key.
    pub identity_signing_key: Vec<u8>,
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
            hitl_request_ttl_days: config.hitl_request_ttl_days,
            tool_search_mode: ToolSearchMode::parse(&config.mcp_tool_search_mode),
            tool_search_tool_limit: config.mcp_tool_search_tool_limit,
            tool_search_meta_limit: config.mcp_tool_search_meta_limit,
            openai_api_key: config.openai_api_key.clone(),
            embedding_model: config.embedding_model.clone(),
            // Central `Config` carries the raw (possibly blank) value; this
            // crate owns the default text (it syncs to the public repo, so the
            // default sentence can't live in `nasiko-config`).
            gateway_instructions: if config.mcp_gateway_instructions.trim().is_empty() {
                DEFAULT_GATEWAY_INSTRUCTIONS.to_string()
            } else {
                config.mcp_gateway_instructions.clone()
            },
            // Fails loudly (not silently signing with an empty key) — an empty
            // key here means both `MCP_IDENTITY_SIGNING_KEY` and the required
            // `JWT_SECRET` were unset, which central `Config::from_env` should
            // never actually produce in a valid deployment. Trimmed already by
            // `Config::from_env`'s own derivation, so no `.trim()` here.
            identity_signing_key: {
                let key = Some(config.mcp_identity_signing_key.as_str())
                    .filter(|s| !s.is_empty())
                    .expect(
                        "MCP_IDENTITY_SIGNING_KEY or JWT_SECRET must be set for MCP identity signing",
                    );
                // A short key weakens HMAC-SHA256 far less than it would a
                // block cipher, but a short, guessable value (a placeholder
                // left over from a quickstart, say) is still worth flagging —
                // a warning, not a hard failure, since a short-but-genuinely-
                // random key is still valid. Measure the actual secret
                // material, not the domain-separation prefix the JWT_SECRET
                // fallback path prepends (`nasiko_config::IDENTITY_KEY_PREFIX`
                // — the single source both crates read, so this strip can
                // never drift from what `Config::from_env` actually prepends):
                // that prefix is constant across every deployment and
                // contributes no entropy, so counting it would let a short
                // JWT_SECRET hide under the threshold.
                let secret_len = key
                    .strip_prefix(nasiko_config::IDENTITY_KEY_PREFIX)
                    .unwrap_or(key)
                    .len();
                if secret_len < 32 {
                    tracing::warn!(
                        key_bytes = secret_len,
                        "MCP_IDENTITY_SIGNING_KEY (or its JWT_SECRET-derived fallback) is shorter \
                         than 32 bytes — consider a longer value"
                    );
                }
                key.as_bytes().to_vec()
            },
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
            hitl_request_ttl_days: 7,
            tool_search_mode: ToolSearchMode::None,
            tool_search_tool_limit: 15,
            tool_search_meta_limit: 10,
            openai_api_key: None,
            embedding_model: "text-embedding-3-small".to_string(),
            gateway_instructions: String::new(),
            identity_signing_key: b"test-identity-signing-key".to_vec(),
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
