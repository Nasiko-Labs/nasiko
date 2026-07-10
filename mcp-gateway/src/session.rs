//! Session resolver — the collapsed, Redis-cached backend resolution.
//!
//! Resolves, per user, the ordered backend list `[composio, generic_1, …]`. The
//! Composio Tool Router session (a network call) is Redis-cached at
//! `mcp:session:{user}`; generic servers are rebuilt from the DB each call
//! (cheap: batch queries + in-process decrypt), so a credential change takes
//! effect immediately and no plaintext secret is cached beyond the request.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cache;
use crate::credentials;
use crate::error::Result;
use crate::provider::{ComposioSession, ConnectedAccounts, ToolProvider};
use crate::repo;
use crate::state::McpState;
use crate::types::{MCPServerConfig, ServerType};

/// The resolved backend set for a user, plus the Composio toolkit fingerprint
/// and the toolkit→connector map used to resolve Composio tool permissions.
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub servers: Vec<MCPServerConfig>,
    pub connected_toolkits: Vec<String>,
    /// Composio toolkit slug → its connector id (for per-toolkit permission checks).
    pub toolkit_to_connector: HashMap<String, Uuid>,
}

/// Carries the Composio master API key; never written to Redis.
const COMPOSIO_API_KEY_HEADER: &str = "x-api-key";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedComposioSession {
    session_id: String,
    mcp_url: String,
    mcp_headers: HashMap<String, String>,
    toolkits: Vec<String>,
}

fn session_cache_key(user_id: Uuid) -> String {
    format!("mcp:session:{user_id}")
}

/// Remove the Composio API-key header (case-insensitively) before caching.
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

    let (accounts, toolkits, toolkit_to_connector) = current_connected_accounts(state, user_id).await?;

    if let Some(composio) = resolve_composio_backend(state, user_id, &accounts, &toolkits).await? {
        servers.push(composio);
        connected_toolkits = toolkits;
    }

    let generic = credentials::build_generic_servers(state, user_id).await?;
    servers.extend(generic);

    Ok(ResolvedSession { servers, connected_toolkits, toolkit_to_connector })
}

/// Invalidate the cached Composio session for a user.
pub async fn invalidate_session_cache(state: &McpState, user_id: Uuid) {
    cache::delete(&state.redis, &session_cache_key(user_id)).await;
}

/// Resolve the Composio Tool Router backend. `None` when Composio is disabled or
/// the user has no active Composio connections; failures degrade to `None`.
async fn resolve_composio_backend(
    state: &McpState,
    user_id: Uuid,
    accounts: &ConnectedAccounts,
    toolkits: &[String],
) -> Result<Option<MCPServerConfig>> {
    let Some(provider) = &state.providers.composio else {
        return Ok(None);
    };
    if accounts.is_empty() {
        return Ok(None);
    }

    // Fast path: cached session with matching toolkits, no Composio call.
    let key = session_cache_key(user_id);
    if let Some(cached) = cache::get_json::<CachedComposioSession>(&state.redis, &key).await
        && cached.toolkits == toolkits
    {
        return Ok(Some(composio_config(cached.mcp_url, cached.mcp_headers, &state.config.composio_api_key)));
    }

    // Slow/cold path: reuse / patch / create the upstream session.
    let session = match resolve_upstream(state, provider, user_id, accounts).await {
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
            mcp_headers: strip_cached_secret(&session.mcp_headers),
            toolkits: toolkits.to_vec(),
        },
        state.config.session_ttl_seconds,
    )
    .await;

    Ok(Some(composio_config(session.mcp_url, session.mcp_headers, &state.config.composio_api_key)))
}

/// Reuse the stored session (patching connected accounts), or create a fresh one.
async fn resolve_upstream(
    state: &McpState,
    provider: &std::sync::Arc<dyn ToolProvider>,
    user_id: Uuid,
    accounts: &ConnectedAccounts,
) -> Result<ComposioSession> {
    if let Some(row) = repo::get_composio_session(&state.db, user_id).await?
        && let Some(session) = provider.reuse_session(&row.composio_session_id).await?
    {
        // Session alive — ensure connected accounts are current (idempotent).
        if provider.patch_session(&row.composio_session_id, accounts).await? {
            return Ok(session);
        }
        // Session died during patch → recreate.
    }
    create_and_store(state, provider, user_id, accounts).await
}

async fn create_and_store(
    state: &McpState,
    provider: &std::sync::Arc<dyn ToolProvider>,
    user_id: Uuid,
    accounts: &ConnectedAccounts,
) -> Result<ComposioSession> {
    let session = provider.create_session(&user_id.to_string(), accounts).await?;
    repo::upsert_composio_session(&state.db, user_id, &session.session_id).await?;
    tracing::info!(%user_id, session_id = %session.session_id, "created composio tool router session");
    Ok(session)
}

/// Build `{toolkit: [account_id]}`, the sorted toolkit list, and the
/// toolkit→connector map from the user's ACTIVE Composio connections.
async fn current_connected_accounts(
    state: &McpState,
    user_id: Uuid,
) -> Result<(ConnectedAccounts, Vec<String>, HashMap<String, Uuid>)> {
    let active = repo::list_active_composio_connections(&state.db, user_id).await?;
    let mut accounts: ConnectedAccounts = HashMap::new();
    let mut toolkit_to_connector = HashMap::new();
    for conn in active {
        accounts.entry(conn.toolkit.clone()).or_default().push(conn.connected_account_id);
        toolkit_to_connector.insert(conn.toolkit.to_ascii_lowercase(), conn.connector_id);
    }
    let mut toolkits: Vec<String> = accounts.keys().cloned().collect();
    toolkits.sort();
    Ok((accounts, toolkits, toolkit_to_connector))
}

/// Build the Composio backend config, (re-)injecting the master key from config.
fn composio_config(
    url: String,
    mut headers: HashMap<String, String>,
    api_key: &Option<String>,
) -> MCPServerConfig {
    if let Some(key) = api_key {
        headers.insert(COMPOSIO_API_KEY_HEADER.to_string(), key.clone());
    }
    MCPServerConfig {
        connector_id: Uuid::nil(),
        kind: ServerType::Composio,
        name: "composio".to_string(),
        url,
        headers,
        transport: "streamable_http".to_string(),
    }
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
        assert!(!stripped.contains_key("x-api-key"));
        assert_eq!(stripped.get("x-other").map(String::as_str), Some("keep-me"));
    }

    #[test]
    fn strip_is_case_insensitive() {
        let mut headers = HashMap::new();
        headers.insert("X-API-Key".to_string(), "ak_master_secret".to_string());
        assert!(strip_cached_secret(&headers).is_empty());
    }

    #[test]
    fn config_reinjects_key_from_config_not_cache() {
        let cfg = composio_config("https://mcp/x".into(), HashMap::new(), &Some("ak_from_config".to_string()));
        assert_eq!(cfg.headers.get("x-api-key").map(String::as_str), Some("ak_from_config"));
        assert_eq!(cfg.kind, ServerType::Composio);
        assert_eq!(cfg.connector_id, Uuid::nil());
    }
}
