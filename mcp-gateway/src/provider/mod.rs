//! Tool backends.
//!
//! Two concrete pieces, per plan §5 / §20.6:
//!
//! * [`GenericMcpProvider`] (`generic.rs`) — the shared streamable-HTTP MCP
//!   transport. Deterministic HTTP; used for every backend, including the
//!   Composio session URL. Not behind a trait — it has nothing provider-specific
//!   to swap.
//!
//! * [`ToolProvider`] (this module) + [`ComposioProvider`] (`composio.rs`) — the
//!   Composio **Tool Router** management surface (auth configs, connections,
//!   sessions, revoke). This is the hand-built v3/v3.1 HTTP client the plan flags
//!   as the largest/riskiest piece, so it sits behind the `ToolProvider` trait to
//!   be mocked in tests and swapped for tenant-scoped impls in EE.
//!
//! The [`Providers`] registry bundles both and is stored in `McpState`.

pub mod composio;
pub mod generic;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::McpConfig;
use crate::error::Result;

pub use composio::ComposioProvider;
pub use generic::GenericMcpProvider;

// ─── Result types ───────────────────────────────────────────────────────────

/// Map of `toolkit -> [connected_account_id]`, as Composio expects when scoping
/// a Tool Router session to a user's connected accounts.
pub type ConnectedAccounts = HashMap<String, Vec<String>>;

/// Result of registering a toolkit OAuth app.
#[derive(Debug, Clone)]
pub struct AuthConfigCreated {
    pub auth_config_id: String,
}

/// Result of initiating a user OAuth connection.
#[derive(Debug, Clone)]
pub struct ConnectionInitiated {
    /// URL the user opens in a browser to authorize. `None` if Composio returned
    /// an already-connected/no-redirect response.
    pub redirect_url: Option<String>,
    /// Composio's reported connection status (raw; not yet normalized).
    pub status: String,
}

/// Live status of a connection plus its resolved Composio account id (`ca_…`).
#[derive(Debug, Clone)]
pub struct ConnectionStatus {
    /// Raw Composio status, or the sentinels `NOT_FOUND` / `UNKNOWN`.
    pub status: String,
    pub account_id: Option<String>,
}

/// A resolved Composio Tool Router session: the durable id plus the MCP endpoint
/// (url + headers) the gateway treats as one backend.
#[derive(Debug, Clone)]
pub struct ComposioSession {
    pub session_id: String,
    pub mcp_url: String,
    pub mcp_headers: HashMap<String, String>,
}

/// A tool descriptor for the per-agent permission UI (name + optional description).
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: Option<String>,
}

// ─── ToolProvider trait (Composio management surface) ───────────────────────

#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Register a toolkit OAuth app. Returns the Composio `auth_config_id`.
    async fn create_auth_config(
        &self,
        toolkit: &str,
        use_composio_managed: bool,
        client_id: Option<&str>,
        client_secret: Option<&str>,
        scopes: Option<&[String]>,
    ) -> Result<AuthConfigCreated>;

    /// Initiate a user OAuth connection (Tool Router link). Returns the browser
    /// redirect URL + status.
    async fn initiate_connection(
        &self,
        user_id: &str,
        auth_config_id: &str,
        callback_url: Option<&str>,
    ) -> Result<ConnectionInitiated>;

    /// Look up a connection's live status and resolve its `ca_…` account id.
    async fn check_connection_status(
        &self,
        user_id: &str,
        auth_config_id: &str,
    ) -> Result<ConnectionStatus>;

    /// Create a fresh per-user Tool Router session, scoped to the given
    /// connected accounts. Deliberately created **without** a toolkit
    /// restriction (`manage_connections` omitted — the live v3.1 API 400s on a
    /// literal `false`, so implementations must NOT send it as a boolean) so
    /// adding toolkits later does not trigger `[Session Restriction]` — mirrors
    /// the PoC's intent, adapted to the real API's constraints.
    async fn create_session(
        &self,
        user_id: &str,
        connected_accounts: &ConnectedAccounts,
    ) -> Result<ComposioSession>;

    /// Re-attach to an existing session (`composio.use`). Returns `Ok(None)` when
    /// the session is dead/gone so the caller recreates it — never errors on a
    /// dead session.
    async fn reuse_session(&self, session_id: &str) -> Result<Option<ComposioSession>>;

    /// Update a session's connected accounts (`sess.update`). Returns `false`
    /// when the session is dead so the caller recreates it.
    async fn patch_session(
        &self,
        session_id: &str,
        connected_accounts: &ConnectedAccounts,
    ) -> Result<bool>;

    /// Revoke a Composio connected account, permanently killing its OAuth token.
    async fn revoke_connection(&self, connected_account_id: &str) -> Result<bool>;

    /// List the tools in a Composio toolkit (for the per-agent permission UI).
    async fn list_toolkit_tools(&self, toolkit: &str) -> Result<Vec<ToolDescriptor>>;
}

// ─── Registry ───────────────────────────────────────────────────────────────

/// Bundle of tool backends held in `McpState`.
#[derive(Clone)]
pub struct Providers {
    /// Composio Tool Router client — `None` when `COMPOSIO_API_KEY` is unset
    /// (generic MCP servers still work). Behind `Arc<dyn>` so EE / tests can swap it.
    pub composio: Option<Arc<dyn ToolProvider>>,
    /// Shared MCP transport for all backends.
    pub mcp: GenericMcpProvider,
}

impl Providers {
    pub fn new(http: reqwest::Client, config: &McpConfig) -> Self {
        let composio = config.composio_api_key.as_ref().map(|key| {
            Arc::new(ComposioProvider::new(
                http.clone(),
                key.clone(),
                config.composio_base_url.clone(),
            )) as Arc<dyn ToolProvider>
        });
        // The generic transport talks to user-registered backend URLs by default,
        // so it uses an SSRF/DNS-rebinding-guarded client (rejects private/internal
        // targets at resolution time) rather than the platform's shared client,
        // which must still reach internal hosts. The Composio session URL it also
        // calls is public, so the guard is transparent there. `http` (the
        // platform's own client, already passed in for the Composio provider
        // above) is reused as the `plain` client for `trusted == true` backends —
        // i.e. uploaded-build MCP-server connectors, whose address the platform
        // itself resolved, not a user.
        Self { composio, mcp: GenericMcpProvider::new(crate::net::guarded_http_client(), http) }
    }

    /// The Composio provider, or a `NotConfigured` error when no API key is set.
    pub fn require_composio(&self) -> Result<&Arc<dyn ToolProvider>> {
        self.composio.as_ref().ok_or_else(|| {
            crate::error::McpError::NotConfigured(
                "Composio integration is disabled (COMPOSIO_API_KEY not set)".to_string(),
            )
        })
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Read a string field from a JSON object.
pub(crate) fn v_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Read the first present string field from a list of candidate keys — mirrors
/// the PoC's tolerant `_extract_value`, so Composio field-name variants
/// (`id` / `nanoid` / `auth_config_id`, `redirect_url` / `redirectUrl`, …) all
/// resolve without brittle typed deserialization.
pub(crate) fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v_str(v, k))
}

/// Normalize a raw Composio connection status into the three states our
/// `mcp_connections.status` CHECK allows: `INITIATED` / `ACTIVE` / `EXPIRED`.
///
/// Composio reports INITIALIZING, INITIATED, ACTIVE, FAILED, EXPIRED, INACTIVE,
/// REVOKED. Anything terminal/broken collapses to `EXPIRED`; anything pending to
/// `INITIATED`.
pub fn normalize_connection_status(raw: &str) -> &'static str {
    match raw.to_ascii_uppercase().as_str() {
        "ACTIVE" => "ACTIVE",
        "INITIALIZING" | "INITIATED" => "INITIATED",
        _ => "EXPIRED",
    }
}
