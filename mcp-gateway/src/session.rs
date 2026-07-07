//! Session resolver — the collapsed, Redis-cached backend resolution.
//!
//! Unlike the PoC (per-(user,agent) upstream sessions, `/warm` endpoints,
//! `last_verified_at`), the agent always talks to *our* stable `/api/mcp`. Here
//! we resolve, on demand and **per user**, the ordered list of backends:
//!
//!   `[ composio(Tool Router session) , generic_1 , generic_2 , … ]`
//!
//! The Composio entry is always index 0 (when the user has active Composio
//! connections). Its resolution (a network call to the Tool Router API) is
//! Redis-cached at `mcp:session:{user}` with a TTL: within the window we return
//! the cached url+headers with **no** Composio call (the fast path); on a miss or
//! a toolkit change we reuse/patch/create the upstream session and re-cache.
//!
//! Generic servers are rebuilt from the DB every call (cheap: two batch queries
//! plus in-process AES decrypt), so a credential change takes effect immediately
//! and no plaintext secret is cached beyond the request.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache;
use crate::credentials;
use crate::error::Result;
use crate::provider::{ComposioSession, ConnectedAccounts, ToolProvider};
use crate::repo;
use crate::state::McpState;
use crate::types::MCPServerConfig;

/// The resolved backend set for a user, plus the Composio toolkit fingerprint
/// (used by the aggregator's manifest cache key so a toolkit change invalidates
/// the merged tool list even though the Composio URL is stable).
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub servers: Vec<MCPServerConfig>,
    pub connected_toolkits: Vec<String>,
}

/// The header the Composio Tool Router MCP url authenticates with. It carries
/// the Composio **master API key**, so it is deliberately never written to Redis
/// (see [`strip_cached_secret`]) — it is re-injected from config at read time by
/// [`composio_config`].
const COMPOSIO_API_KEY_HEADER: &str = "x-api-key";

/// Redis-cached shape of the resolved Composio backend.
///
/// `mcp_headers` holds only non-secret headers: the Composio `x-api-key` is
/// stripped before caching and re-injected from config on read, so the master
/// key is never persisted to Redis (which is a shared, if internal, store).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedComposioSession {
    session_id: String,
    mcp_url: String,
    mcp_headers: HashMap<String, String>,
    /// Sorted toolkit list this cache entry was built for — a mismatch forces
    /// a re-resolve so the upstream session gets patched.
    toolkits: Vec<String>,
}

fn session_cache_key(user_id: Uuid) -> String {
    format!("mcp:session:{user_id}")
}

/// Remove the Composio API-key header (case-insensitively) from a header set
/// before it is persisted to Redis. Returns a copy without the secret.
fn strip_cached_secret(headers: &HashMap<String, String>) -> HashMap<String, String> {
    headers
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(COMPOSIO_API_KEY_HEADER))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Resolve all backends for a user: Composio (if any active connections) first,
/// then credential-injected generic servers.
pub async fn resolve_session(state: &McpState, user_id: Uuid) -> Result<ResolvedSession> {
    let mut servers = Vec::new();
    let mut connected_toolkits = Vec::new();

    if let Some((composio, toolkits)) = resolve_composio_backend(state, user_id).await? {
        servers.push(composio);
        connected_toolkits = toolkits;
    }

    let generic = credentials::build_generic_servers(state, user_id).await?;
    servers.extend(generic);

    Ok(ResolvedSession { servers, connected_toolkits })
}

/// Invalidate the cached Composio session for a user. Call after connect,
/// disconnect, or a token-expiry webhook so the next resolve re-syncs.
pub async fn invalidate_session_cache(state: &McpState, user_id: Uuid) {
    cache::delete(&state.redis, &session_cache_key(user_id)).await;
}

/// Resolve the Composio Tool Router backend for a user, returning the backend
/// config + the sorted toolkit list. `None` when Composio is disabled or the
/// user has no active Composio connections.
///
/// If Composio resolution fails (network/upstream), this logs and returns `None`
/// — the user's generic tools still work; the Composio tools reappear when it
/// recovers (same "skip a failing backend" philosophy as aggregation).
async fn resolve_composio_backend(
    state: &McpState,
    user_id: Uuid,
) -> Result<Option<(MCPServerConfig, Vec<String>)>> {
    let Some(provider) = &state.providers.composio else {
        return Ok(None); // Composio disabled (no API key)
    };

    let (accounts, toolkits) = current_connected_accounts(state, user_id).await?;
    if accounts.is_empty() {
        return Ok(None); // no active Composio connections → no Composio backend
    }

    // ── Fast path: cached session with matching toolkits, no Composio call ──
    let key = session_cache_key(user_id);
    if let Some(cached) = cache::get_json::<CachedComposioSession>(&state.redis, &key).await
        && cached.toolkits == toolkits
    {
        // Re-inject the API key (stripped before caching) from config.
        let cfg = composio_config(cached.mcp_url, cached.mcp_headers, &state.config.composio_api_key);
        return Ok(Some((cfg, toolkits)));
    }

    // ── Slow/cold path: reuse / patch / create the upstream session ─────────
    let session = match resolve_upstream(state, provider, user_id, &accounts, &toolkits).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%user_id, error = %e, "composio session resolution failed — omitting composio backend this cycle");
            return Ok(None);
        }
    };

    cache::set_json_ex(
        &state.redis,
        &key,
        &CachedComposioSession {
            session_id: session.session_id.clone(),
            mcp_url: session.mcp_url.clone(),
            // Strip the Composio master key before it ever reaches Redis.
            mcp_headers: strip_cached_secret(&session.mcp_headers),
            toolkits: toolkits.clone(),
        },
        state.config.session_ttl_seconds,
    )
    .await;

    // `session.mcp_headers` already carries the key on the fresh path; passing it
    // through `composio_config` keeps injection in one place (idempotent).
    let cfg = composio_config(session.mcp_url, session.mcp_headers, &state.config.composio_api_key);
    Ok(Some((cfg, toolkits)))
}

/// Reuse the stored session (patching connected accounts if toolkits changed),
/// or create a fresh one — persisting the durable `session_id`.
async fn resolve_upstream(
    state: &McpState,
    provider: &std::sync::Arc<dyn ToolProvider>,
    user_id: Uuid,
    accounts: &ConnectedAccounts,
    toolkits: &[String],
) -> Result<ComposioSession> {
    let stored = repo::get_composio_session(&state.db, user_id).await?;

    if let Some(row) = stored
        && let Some(session) = provider.reuse_session(&row.session_id).await?
    {
        // Session alive. Patch connected accounts only if the toolkit set drifted.
        let mut stored_toolkits: Vec<String> = row
            .connected_toolkits
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        stored_toolkits.sort();

        if stored_toolkits != toolkits {
            if provider.patch_session(&row.session_id, accounts).await? {
                persist_session(state, user_id, &row.session_id, accounts, toolkits).await?;
            } else {
                // Session died during patch — recreate.
                return create_and_store(state, provider, user_id, accounts, toolkits).await;
            }
        }
        return Ok(session);
    }

    // No stored session, or it was dead → create fresh.
    create_and_store(state, provider, user_id, accounts, toolkits).await
}

async fn create_and_store(
    state: &McpState,
    provider: &std::sync::Arc<dyn ToolProvider>,
    user_id: Uuid,
    accounts: &ConnectedAccounts,
    toolkits: &[String],
) -> Result<ComposioSession> {
    let session = provider.create_session(&user_id.to_string(), accounts).await?;
    persist_session(state, user_id, &session.session_id, accounts, toolkits).await?;
    tracing::info!(%user_id, session_id = %session.session_id, ?toolkits, "created composio tool router session");
    Ok(session)
}

async fn persist_session(
    state: &McpState,
    user_id: Uuid,
    session_id: &str,
    accounts: &ConnectedAccounts,
    toolkits: &[String],
) -> Result<()> {
    let accounts_json = serde_json::to_value(accounts)?;
    let toolkits_json = serde_json::to_value(toolkits)?;
    repo::upsert_composio_session(
        &state.db,
        user_id,
        session_id,
        Some(&accounts_json),
        Some(&toolkits_json),
    )
    .await?;
    Ok(())
}

/// Build the `{toolkit: [connected_account_id]}` map + sorted toolkit list from
/// the user's ACTIVE Composio connections that have a resolved account id.
async fn current_connected_accounts(
    state: &McpState,
    user_id: Uuid,
) -> Result<(ConnectedAccounts, Vec<String>)> {
    let active = repo::list_connections_by_user(&state.db, user_id, Some("ACTIVE")).await?;
    let mut accounts: ConnectedAccounts = HashMap::new();
    for conn in active {
        if let Some(account_id) = conn.connected_account_id {
            accounts.entry(conn.toolkit).or_default().push(account_id);
        }
    }
    let mut toolkits: Vec<String> = accounts.keys().cloned().collect();
    toolkits.sort();
    Ok((accounts, toolkits))
}

/// Build the Composio backend config, (re-)injecting the Composio `x-api-key`
/// from config. The key is authoritative from config, never from cached headers —
/// so the master secret lives only in process config/memory, never in Redis.
fn composio_config(
    url: String,
    mut headers: HashMap<String, String>,
    api_key: &Option<String>,
) -> MCPServerConfig {
    if let Some(key) = api_key {
        headers.insert(COMPOSIO_API_KEY_HEADER.to_string(), key.clone());
    }
    MCPServerConfig { name: "composio".to_string(), url, headers, transport: "streamable_http".to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_stripped_before_caching() {
        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), "ak_master_secret".to_string());
        headers.insert("x-other".to_string(), "keep-me".to_string());
        let stripped = strip_cached_secret(&headers);
        assert!(!stripped.contains_key("x-api-key"), "master key must not be cached");
        assert_eq!(stripped.get("x-other").map(String::as_str), Some("keep-me"));
    }

    #[test]
    fn strip_is_case_insensitive() {
        let mut headers = HashMap::new();
        headers.insert("X-API-Key".to_string(), "ak_master_secret".to_string());
        assert!(strip_cached_secret(&headers).is_empty(), "differently-cased key must also be stripped");
    }

    #[test]
    fn config_reinjects_key_from_config_not_cache() {
        // Cached headers have no key (stripped). composio_config re-adds it from config.
        let cfg = composio_config(
            "https://mcp/x".into(),
            HashMap::new(),
            &Some("ak_from_config".to_string()),
        );
        assert_eq!(cfg.headers.get("x-api-key").map(String::as_str), Some("ak_from_config"));
    }
}
