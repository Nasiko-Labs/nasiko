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
use std::sync::{Arc, RwLock};

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
    /// The tool's input parameter schema, when the backend provides one —
    /// used only as LLM-fallback signal (`description_backfill`) when
    /// `description` is `None`; never persisted.
    pub input_schema: Option<Value>,
}

/// A toolkit's own catalog description (distinct from any one tool's
/// description) — Composio's `GET /toolkits/{slug}` `meta.description`.
/// Deliberately carries no logo field — logos are the platform's own images,
/// never fetched from Composio.
#[derive(Debug, Clone, Default)]
pub struct ToolkitMetadata {
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

    /// Whether `auth_config_id` exists in the project this provider's key belongs to.
    /// `false` after a key from a different project is saved.
    async fn auth_config_exists(&self, auth_config_id: &str) -> Result<bool>;

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

    /// Downcast support for accessing provider-specific methods (e.g.
    /// `ComposioProvider::list_tools_for_toolkits`).
    fn as_any(&self) -> &dyn std::any::Any;
}

// ─── Registry ───────────────────────────────────────────────────────────────

/// The live Composio backend and the key it authenticates with.
#[derive(Clone)]
pub struct ComposioBackend {
    pub provider: Arc<dyn ToolProvider>,
    pub api_key: Option<String>,
}

/// A swappable handle to the Composio backend, shared by every clone of
/// `McpState`, so a key saved in the admin UI takes effect without a restart.
#[derive(Clone, Default)]
pub struct ComposioSlot(Arc<RwLock<Option<ComposioBackend>>>);

impl ComposioSlot {
    pub fn new(provider: Arc<dyn ToolProvider>, api_key: Option<String>) -> Self {
        Self(Arc::new(RwLock::new(Some(ComposioBackend {
            provider,
            api_key,
        }))))
    }

    /// The current provider, or `None` while no key is configured.
    pub fn get(&self) -> Option<Arc<dyn ToolProvider>> {
        self.read().as_ref().map(|b| Arc::clone(&b.provider))
    }

    /// The current provider and its key, read together so a concurrent swap can't pair
    /// one key's provider with the other key.
    pub fn backend(&self) -> Option<ComposioBackend> {
        self.read().clone()
    }

    /// The key the current provider authenticates with.
    pub fn api_key(&self) -> Option<String> {
        self.read().as_ref().and_then(|b| b.api_key.clone())
    }

    pub fn replace(&self, backend: Option<ComposioBackend>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = backend;
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Option<ComposioBackend>> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }
}

/// Bundle of tool backends held in `McpState`.
#[derive(Clone)]
pub struct Providers {
    /// Composio Tool Router client — empty when no Composio key is configured
    /// (generic MCP servers still work). Behind `Arc<dyn>` so EE / tests can swap it.
    pub composio: ComposioSlot,
    /// Shared MCP transport for all backends.
    pub mcp: GenericMcpProvider,
}

impl Providers {
    pub fn new(http: reqwest::Client, config: &McpConfig) -> Self {
        let composio = match config.composio_api_key.as_ref() {
            Some(key) => ComposioSlot::new(
                Arc::new(ComposioProvider::new(
                    http.clone(),
                    key.clone(),
                    config.composio_base_url.clone(),
                )),
                Some(key.clone()),
            ),
            None => ComposioSlot::default(),
        };
        // The generic transport talks to user-registered backend URLs by default,
        // so it uses an SSRF/DNS-rebinding-guarded client (rejects private/internal
        // targets at resolution time) rather than the platform's shared client,
        // which must still reach internal hosts. The Composio session URL it also
        // calls is public, so the guard is transparent there. `http` (the
        // platform's own client, already passed in for the Composio provider
        // above) is reused as the `plain` client for `trusted == true` backends —
        // i.e. uploaded-build MCP-server connectors, whose address the platform
        // itself resolved, not a user. Only Composio keeps a connection pool.
        Self {
            composio,
            mcp: GenericMcpProvider::new(crate::net::guarded_http_client_no_reuse(), http)
                .with_composio_client(crate::net::guarded_http_client()),
        }
    }

    /// The Composio provider, or a `NotConfigured` error when no API key is set.
    pub fn require_composio(&self) -> Result<Arc<dyn ToolProvider>> {
        self.composio.get().ok_or_else(|| {
            crate::error::McpError::NotConfigured(
                "Composio integration is disabled (no Composio API key configured)".to_string(),
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

/// Same tolerance as [`first_str`], for a field whose value is itself a JSON
/// object (e.g. a tool's input parameter schema) rather than a string.
pub(crate) fn first_value(v: &Value, keys: &[&str]) -> Option<Value> {
    keys.iter()
        .find_map(|k| v.get(k).filter(|val| !val.is_null()).cloned())
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
