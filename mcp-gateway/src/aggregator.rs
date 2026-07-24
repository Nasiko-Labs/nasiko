//! Tool aggregation — fan out `tools/list` to every backend, namespace generic
//! tools by connector id, filter by the agent's permissions, merge, Redis-cache.
//!
//! Composio meta-tools (`COMPOSIO_SEARCH_TOOLS`, …) keep their names and are
//! never filtered here (per-toolkit enforcement happens at `tools/call`).
//! Generic-server tools are namespaced `{connector_prefix}__{tool}`; a disabled
//! connector is dropped wholesale and individually-blocked tools are removed. A
//! backend that errors/times out is skipped for this cycle.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache;
use crate::error::Result;
use crate::permissions::{PermissionContext, ToolAccess, sha256_hex16};
use crate::provider::generic::LIST_TIMEOUT;
use crate::state::McpState;
use crate::types::{MCPServerConfig, ServerType, connector_prefix};

/// Fan out, namespace, filter, merge, cache. Returns the merged tool list.
pub async fn aggregate_tools(
    state: &McpState,
    user_id: Uuid,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Result<Vec<Value>> {
    let key = manifest_key(user_id, servers, connected_toolkits, &perms.hash);
    if let Some(cached) = cache::get_json::<Vec<Value>>(&state.redis, &key).await {
        tracing::debug!(%user_id, "manifest cache hit");
        return Ok(cached);
    }

    let active: Vec<&MCPServerConfig> = servers.iter().filter(|s| !s.url.is_empty()).collect();
    let provider = &state.providers.mcp;
    let results = futures::future::join_all(
        active
            .iter()
            .map(|s| async move { (*s, provider.list_tools(s, LIST_TIMEOUT, traceparent).await) }),
    )
    .await;

    let mut merged: Vec<Value> = Vec::new();
    for (server, result) in results {
        let tools = match result {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(server = %server.name, error = %e, "tools/list failed — skipping backend this cycle");
                continue;
            }
        };

        // Composio meta-tools pass through unchanged, unfiltered.
        if server.kind == ServerType::Composio {
            merged.extend(tools);
            continue;
        }

        // Generic server: connector-level toggle (fast skip of the whole backend),
        // then the SAME per-tool decision `tools/call` enforces — Denied tools are
        // dropped, Ask/Allowed stay listed. Going through `decide` keeps list and
        // call in lockstep.
        if !perms.is_connector_enabled(server.connector_id) {
            continue;
        }
        let prefix = connector_prefix(server.connector_id);
        for mut tool in tools {
            let Some(obj) = tool.as_object_mut() else { continue };
            let Some(original) = obj.get("name").and_then(|n| n.as_str()).map(str::to_string) else {
                continue;
            };
            if perms.decide(server.connector_id, &original) == ToolAccess::Denied {
                continue;
            }
            obj.insert("name".to_string(), json!(format!("{prefix}__{original}")));
            merged.push(tool);
        }
    }

    cache::set_json_ex(&state.redis, &key, &merged, state.config.manifest_ttl_seconds).await;
    tracing::info!(%user_id, tool_count = merged.len(), backends = active.len(), perms_hash = %perms.hash, "manifest built");
    Ok(merged)
}

/// `mcp:manifest:{user}:{backends_fp}:{perms_hash}` where `backends_fp` hashes,
/// per backend, `(connector_id, url, injected-headers fingerprint)` plus the
/// sorted connected toolkits (the Composio URL is stable across toolkit changes,
/// so the latter is needed).
///
/// The per-backend **headers fingerprint** is what closes finding #8: the
/// injected credential lives in `headers` (bearer/basic/oauth token) or in `url`
/// (url_param), so a credential/scope rotation that changes which tools a backend
/// exposes now changes the key and invalidates the stale manifest. The headers
/// are hashed (never placed in the key), so no secret is written to Redis.
fn manifest_key(
    user_id: Uuid,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms_hash: &str,
) -> String {
    let mut backends: Vec<(String, &str, String)> = servers
        .iter()
        .filter(|s| !s.url.is_empty())
        .map(|s| (s.connector_id.to_string(), s.url.as_str(), headers_fingerprint(&s.headers)))
        .collect();
    backends.sort();

    let mut toolkits: Vec<&str> = connected_toolkits.iter().map(String::as_str).collect();
    toolkits.sort();

    let raw = serde_json::to_string(&(backends, toolkits)).unwrap_or_default();
    let fp = sha256_hex16(raw.as_bytes());
    format!("mcp:manifest:{user_id}:{fp}:{perms_hash}")
}

/// Stable hash of a backend's injected headers (the credential lives here). Keys
/// sorted for determinism; the result is a hash, so the raw secret never appears
/// in the cache key. Empty headers → empty string (no fingerprint contribution).
fn headers_fingerprint(headers: &std::collections::HashMap<String, String>) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    pairs.sort();
    sha256_hex16(serde_json::to_string(&pairs).unwrap_or_default().as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::McpConfig;
    use crate::permissions::PermissionRule;
    use crate::provider::{GenericMcpProvider, Providers};
    use crate::types::Stance;

    fn srv(kind: ServerType, id: Uuid, url: &str) -> MCPServerConfig {
        MCPServerConfig {
            connector_id: id,
            kind,
            name: "test".into(),
            url: url.into(),
            headers: HashMap::new(),
            transport: "streamable_http".into(),
            trusted: false,
        }
    }

    fn empty_perms() -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            disabled_connectors: Default::default(),
            rules: vec![],
            hash: "h".into(),
        }
    }

    /// Build an `McpState` whose `db`/`redis` handles are *lazy* (no network
    /// I/O at construction) and deliberately point at a closed local port so
    /// any accidental use fails fast (connection refused) instead of hanging
    /// or silently touching real infra. `cache::get_json`/`set_json_ex`
    /// degrade a Redis failure to "miss"/no-op by design, so this is exactly
    /// what we want: manifest caching always misses, forcing a real fan-out
    /// on every call, without needing Redis or Postgres up for this crate's
    /// pure unit tests.
    fn test_state() -> McpState {
        let db = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network");
        let redis = redis::Client::open("redis://127.0.0.1:1/")
            .expect("lazy redis client construction must not touch the network");
        McpState {
            db,
            redis,
            http_client: reqwest::Client::new(),
            guarded_http_client: reqwest::Client::new(),
            config: McpConfig {
                composio_api_key: None,
                composio_base_url: "http://localhost".to_string(),
                composio_webhook_secret: None,
                gateway_public_url: None,
                oauth_redirect_base_url: None,
                composio_callback_base_url: None,
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                toolcount_ttl_seconds: 3600,
                oauth_state_signing_key: "test".to_string(),
            },
            providers: Providers {
                composio: None,
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer: std::sync::Arc::new(crate::authorizer::OssConnectorAuthorizer),
            endpoint_refresher: std::sync::Arc::new(crate::endpoint_refresh::NoopEndpointRefresher),
        }
    }

    // ─── manifest_key() — credential-aware cache key (review finding #8) ───

    #[test]
    fn manifest_key_changes_when_injected_credential_rotates() {
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut old = srv(ServerType::Mcp, id, "https://backend.example/mcp");
        old.headers.insert("authorization".into(), "Bearer OLD_TOKEN".into());
        let mut rotated = old.clone();
        rotated.headers.insert("authorization".into(), "Bearer ROTATED_TOKEN_DIFFERENT_SCOPES".into());

        let toolkits: Vec<String> = vec![];
        let k_old = manifest_key(user, &[old], &toolkits, "permshash1");
        let k_rotated = manifest_key(user, &[rotated], &toolkits, "permshash1");

        // Fix #8: the injected credential (here the bearer token in `headers`)
        // is folded into the key via a hash, so a rotation that could change
        // which tools the backend exposes invalidates the stale manifest instead
        // of serving it for up to `manifest_ttl_seconds`.
        assert_ne!(k_old, k_rotated, "a rotated credential must produce a different manifest cache key");
    }

    #[test]
    fn manifest_key_does_not_leak_the_raw_credential() {
        let user = Uuid::new_v4();
        let mut s = srv(ServerType::Mcp, Uuid::new_v4(), "https://backend.example/mcp");
        s.headers.insert("authorization".into(), "Bearer SUPER_SECRET_TOKEN".into());
        let key = manifest_key(user, &[s], &[], "permshash1");
        assert!(!key.contains("SUPER_SECRET_TOKEN"), "the raw credential must never appear in the cache key: {key}");
    }

    #[test]
    fn manifest_key_is_stable_regardless_of_backend_input_order() {
        let user = Uuid::new_v4();
        let a = srv(ServerType::Mcp, Uuid::new_v4(), "https://a.example/mcp");
        let b = srv(ServerType::Mcp, Uuid::new_v4(), "https://b.example/mcp");
        let toolkits: Vec<String> = vec![];
        let k1 = manifest_key(user, &[a.clone(), b.clone()], &toolkits, "h");
        let k2 = manifest_key(user, &[b, a], &toolkits, "h");
        assert_eq!(k1, k2, "backends are sorted before hashing, so input order must not matter");
    }

    // ─── aggregate_tools() — per-backend error isolation ───────────────────

    #[tokio::test]
    async fn multiple_failing_backends_dont_drop_the_healthy_ones() {
        let mut good = mockito::Server::new_async().await;
        good.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"good_tool"}]}}"#)
            .create_async()
            .await;

        let mut http500 = mockito::Server::new_async().await;
        http500.mock("POST", "/mcp").with_status(500).with_body("boom").create_async().await;

        let mut malformed = mockito::Server::new_async().await;
        malformed
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("this is not valid json")
            .create_async()
            .await;

        let good_id = Uuid::new_v4();
        let servers = vec![
            srv(ServerType::Mcp, good_id, &format!("{}/mcp", good.url())),
            srv(ServerType::Mcp, Uuid::new_v4(), &format!("{}/mcp", http500.url())),
            srv(ServerType::Mcp, Uuid::new_v4(), &format!("{}/mcp", malformed.url())),
            // Nothing listens here — a fast "connection refused" transport
            // error, exercising the same skip-and-continue path as a timeout.
            srv(ServerType::Mcp, Uuid::new_v4(), "http://127.0.0.1:1/mcp"),
        ];

        let state = test_state();
        let merged =
            aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &empty_perms(), None).await.unwrap();

        assert_eq!(merged.len(), 1, "only the healthy backend's tool should survive: {merged:?}");
        assert_eq!(merged[0]["name"], json!(format!("{}__good_tool", connector_prefix(good_id))));
    }

    #[tokio::test]
    async fn tool_entry_missing_name_field_is_skipped_not_panicking() {
        let mut m = mockito::Server::new_async().await;
        m.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"description":"no name here"},{"name":"good_tool"}]}}"#,
            )
            .create_async()
            .await;

        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, &format!("{}/mcp", m.url()))];
        let state = test_state();
        let merged =
            aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &empty_perms(), None).await.unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["name"], json!(format!("{}__good_tool", connector_prefix(id))));
    }

    #[tokio::test]
    async fn duplicate_tool_names_from_one_backend_are_kept_not_deduped() {
        let mut m = mockito::Server::new_async().await;
        m.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"dup"},{"name":"dup"}]}}"#)
            .create_async()
            .await;

        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, &format!("{}/mcp", m.url()))];
        let state = test_state();
        let merged =
            aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &empty_perms(), None).await.unwrap();

        // Current behavior: the aggregator does not dedupe by tool name —
        // both entries survive namespacing, producing two identically-named
        // merged tools. Documented here as the observed behavior (neither an
        // endorsement nor one of the 8 pre-identified bugs).
        assert_eq!(merged.len(), 2);
        let expected_name = json!(format!("{}__dup", connector_prefix(id)));
        assert!(merged.iter().all(|t| t["name"] == expected_name));
    }

    #[tokio::test]
    async fn disabled_connector_is_excluded_before_any_tool_rule_is_consulted() {
        let mut m = mockito::Server::new_async().await;
        m.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"any_tool"}]}}"#)
            .create_async()
            .await;

        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, &format!("{}/mcp", m.url()))];
        let state = test_state();
        // An explicit Allow-everything rule is present, but the connector is
        // disabled — `is_connector_enabled()` short-circuits before
        // `get_stance()` is ever consulted for this backend's tools, so the
        // rule is irrelevant: the whole connector's tools are dropped.
        let perms = PermissionContext {
            agent_id: Uuid::nil(),
            disabled_connectors: [id].into_iter().collect(),
            rules: vec![PermissionRule { connector_id: id, tool_pattern: "*".into(), stance: Stance::Allow }],
            hash: "h".into(),
        };
        let merged = aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &perms, None).await.unwrap();
        assert!(merged.is_empty());
    }

    #[tokio::test]
    async fn blocked_tool_is_filtered_but_sibling_tools_on_same_connector_survive() {
        let mut m = mockito::Server::new_async().await;
        m.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"send_email"},{"name":"read_email"}]}}"#)
            .create_async()
            .await;

        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, &format!("{}/mcp", m.url()))];
        let state = test_state();
        let perms = PermissionContext {
            agent_id: Uuid::nil(),
            disabled_connectors: Default::default(),
            rules: vec![PermissionRule { connector_id: id, tool_pattern: "send_*".into(), stance: Stance::Block }],
            hash: "h".into(),
        };
        let merged = aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &perms, None).await.unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["name"], json!(format!("{}__read_email", connector_prefix(id))));
    }
}
