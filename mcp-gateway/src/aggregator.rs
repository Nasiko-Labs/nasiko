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

        // Composio tools: filter out meta-tools (COMPOSIO_SEARCH_TOOLS,
        // COMPOSIO_MULTI_EXECUTE_TOOL, COMPOSIO_MANAGE_CONNECTIONS, etc.)
        // — agents now discover tools via the search index and call them by
        // their real names; the gateway wraps in MULTI_EXECUTE transparently.
        if server.kind == ServerType::Composio {
            merged.extend(tools.into_iter().filter(|t| {
                t.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| !n.starts_with("COMPOSIO_"))
                    .unwrap_or(true)
            }));
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
            let Some(obj) = tool.as_object_mut() else {
                continue;
            };
            let Some(original) = obj.get("name").and_then(|n| n.as_str()).map(str::to_string)
            else {
                continue;
            };
            // Listing and routing must agree: `router::route_tool` only ever
            // hands a bare name to a system backend when its synced
            // `tool_names` catalog claims it exactly — a tool the LIVE
            // backend advertises but the catalog hasn't synced yet would
            // otherwise be listed here, then have `tools/call` misroute it
            // (with its arguments) to Composio instead of erroring. Hide it
            // rather than list something that can't actually be called.
            if server.system && !server.tool_names.iter().any(|t| t == &original) {
                continue;
            }
            if perms.decide(server.connector_id, &original) == ToolAccess::Denied {
                continue;
            }
            // System backends are the only generic servers exposed un-prefixed
            // (`router::route_tool` routes their bare names first) — the tool
            // list is the model's prompt, and `{prefix}__save_file` is worse
            // than `save_file` for a platform-owned backend the model already
            // trusts implicitly.
            if !server.system {
                obj.insert("name".to_string(), json!(format!("{prefix}__{original}")));
            }
            merged.push(tool);
        }
    }

    cache::set_json_ex(
        &state.redis,
        &key,
        &merged,
        state.config.manifest_ttl_seconds,
    )
    .await;
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
        .map(|s| {
            (
                s.connector_id.to_string(),
                s.url.as_str(),
                headers_fingerprint(&s.headers),
            )
        })
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
///
/// Excludes [`crate::identity::IDENTITY_HEADER`] (case-insensitively): unlike a
/// real injected credential, that header carries a per-request `exp`
/// (`identity::SignedIdentity::sign`), so folding it in here would mean the
/// manifest cache never hits at all for any user with a system connector —
/// every `tools/list` would mint a fresh signature and bust the key.
fn headers_fingerprint(headers: &std::collections::HashMap<String, String>) -> String {
    let mut pairs: Vec<(&str, &str)> = headers
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case(crate::identity::IDENTITY_HEADER))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if pairs.is_empty() {
        return String::new();
    }
    pairs.sort();
    sha256_hex16(serde_json::to_string(&pairs).unwrap_or_default().as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

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
            system: false,
            tool_names: vec![],
            instructions: None,
        }
    }

    /// A permissive baseline for tests that aren't exercising the enable/rule
    /// model itself — under the default-deny allowlist, a connector must be
    /// named here to be reachable at all.
    fn perms_enabling(ids: &[Uuid]) -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: ids.iter().copied().collect(),
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
                description_model: "gpt-4o-mini".to_string(),
                hitl_request_ttl_days: 7,
                tool_search_mode: crate::config::ToolSearchMode::None,
                tool_search_tool_limit: 15,
                tool_search_meta_limit: 10,
                openai_api_key: None,
                embedding_model: "text-embedding-3-small".to_string(),
                gateway_instructions: String::new(),
                identity_signing_key: b"test-identity-signing-key".to_vec(),
            },
            providers: Providers {
                composio: None,
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer: std::sync::Arc::new(crate::authorizer::OssConnectorAuthorizer),
            endpoint_refresher: std::sync::Arc::new(crate::endpoint_refresh::NoopEndpointRefresher),
            llm: nasiko_orchestrator::providers::LLMProvider::from_env(reqwest::Client::new()),
            search_index: std::sync::Arc::new(crate::search::NoopSearchIndex),
        }
    }

    // ─── manifest_key() — credential-aware cache key (review finding #8) ───

    #[test]
    fn manifest_key_changes_when_injected_credential_rotates() {
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut old = srv(ServerType::Mcp, id, "https://backend.example/mcp");
        old.headers
            .insert("authorization".into(), "Bearer OLD_TOKEN".into());
        let mut rotated = old.clone();
        rotated.headers.insert(
            "authorization".into(),
            "Bearer ROTATED_TOKEN_DIFFERENT_SCOPES".into(),
        );

        let toolkits: Vec<String> = vec![];
        let k_old = manifest_key(user, &[old], &toolkits, "permshash1");
        let k_rotated = manifest_key(user, &[rotated], &toolkits, "permshash1");

        // Fix #8: the injected credential (here the bearer token in `headers`)
        // is folded into the key via a hash, so a rotation that could change
        // which tools the backend exposes invalidates the stale manifest instead
        // of serving it for up to `manifest_ttl_seconds`.
        assert_ne!(
            k_old, k_rotated,
            "a rotated credential must produce a different manifest cache key"
        );
    }

    #[test]
    fn manifest_key_does_not_leak_the_raw_credential() {
        let user = Uuid::new_v4();
        let mut s = srv(
            ServerType::Mcp,
            Uuid::new_v4(),
            "https://backend.example/mcp",
        );
        s.headers
            .insert("authorization".into(), "Bearer SUPER_SECRET_TOKEN".into());
        let key = manifest_key(user, &[s], &[], "permshash1");
        assert!(
            !key.contains("SUPER_SECRET_TOKEN"),
            "the raw credential must never appear in the cache key: {key}"
        );
    }

    #[test]
    fn manifest_key_is_stable_regardless_of_backend_input_order() {
        let user = Uuid::new_v4();
        let a = srv(ServerType::Mcp, Uuid::new_v4(), "https://a.example/mcp");
        let b = srv(ServerType::Mcp, Uuid::new_v4(), "https://b.example/mcp");
        let toolkits: Vec<String> = vec![];
        let k1 = manifest_key(user, &[a.clone(), b.clone()], &toolkits, "h");
        let k2 = manifest_key(user, &[b, a], &toolkits, "h");
        assert_eq!(
            k1, k2,
            "backends are sorted before hashing, so input order must not matter"
        );
    }

    #[test]
    fn manifest_key_ignores_the_identity_header_but_not_a_real_credential() {
        let user = Uuid::new_v4();
        let id = Uuid::new_v4();
        let mut a = srv(ServerType::Mcp, id, "https://backend.example/mcp");
        a.headers.insert(
            crate::identity::IDENTITY_HEADER.to_string(),
            "sig.one".into(),
        );
        // Same identity header key, different value (a later request's fresh
        // `exp`/signature) — must NOT change the manifest key.
        let mut b = a.clone();
        b.headers.insert(
            crate::identity::IDENTITY_HEADER.to_string(),
            "sig.two-different-exp".into(),
        );
        let toolkits: Vec<String> = vec![];
        let k_a = manifest_key(user, &[a.clone()], &toolkits, "h");
        let k_b = manifest_key(user, &[b], &toolkits, "h");
        assert_eq!(
            k_a, k_b,
            "the identity header carries a per-request exp and must not bust the manifest cache"
        );

        // A real header change (an actual injected credential) must still bust it.
        let mut c = a.clone();
        c.headers
            .insert("authorization".into(), "Bearer other-token".into());
        let k_c = manifest_key(user, &[c], &toolkits, "h");
        assert_ne!(
            k_a, k_c,
            "a real header change must still change the manifest key"
        );
    }

    /// `headers_fingerprint` directly (not through `manifest_key`, which adds
    /// unrelated connector-id/url noise to the hashed tuple): a mixed-case
    /// spelling of the identity header name must be excluded exactly like the
    /// canonical lowercase [`crate::identity::IDENTITY_HEADER`] constant
    /// (HTTP header names are case-insensitive on the wire — `axum`'s
    /// `HeaderMap` normalizes them, but this fn takes a plain `HashMap` a
    /// caller could populate with any casing), and a header map whose ONLY
    /// entry is the identity header must fingerprint identically to a map
    /// with no headers at all — not merely "not contribute a *different*
    /// value", which the empty-string early-return already guarantees but
    /// this makes an explicit, named property.
    #[test]
    fn headers_fingerprint_excludes_identity_header_case_insensitively() {
        let empty: HashMap<String, String> = HashMap::new();
        let mut lower = HashMap::new();
        lower.insert(
            crate::identity::IDENTITY_HEADER.to_string(),
            "sig.one".into(),
        );
        let mut mixed = HashMap::new();
        mixed.insert(
            "X-Nasiko-Identity".to_string(),
            "sig.two-different-value".into(),
        );

        let fp_empty = headers_fingerprint(&empty);
        assert_eq!(
            fp_empty, "",
            "no headers must fingerprint as the empty-string sentinel"
        );
        assert_eq!(
            headers_fingerprint(&lower),
            fp_empty,
            "a header map containing ONLY the identity header must fingerprint \
             the same as an empty map"
        );
        assert_eq!(
            headers_fingerprint(&mixed),
            fp_empty,
            "a mixed-case identity header name (X-Nasiko-Identity) must be \
             excluded just as the canonical lowercase name is"
        );
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
        http500
            .mock("POST", "/mcp")
            .with_status(500)
            .with_body("boom")
            .create_async()
            .await;

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
            srv(
                ServerType::Mcp,
                Uuid::new_v4(),
                &format!("{}/mcp", http500.url()),
            ),
            srv(
                ServerType::Mcp,
                Uuid::new_v4(),
                &format!("{}/mcp", malformed.url()),
            ),
            // Nothing listens here — a fast "connection refused" transport
            // error, exercising the same skip-and-continue path as a timeout.
            srv(ServerType::Mcp, Uuid::new_v4(), "http://127.0.0.1:1/mcp"),
        ];

        let state = test_state();
        let merged = aggregate_tools(
            &state,
            Uuid::new_v4(),
            &servers,
            &[],
            &perms_enabling(&[good_id]),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            merged.len(),
            1,
            "only the healthy backend's tool should survive: {merged:?}"
        );
        assert_eq!(
            merged[0]["name"],
            json!(format!("{}__good_tool", connector_prefix(good_id)))
        );
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
        let merged = aggregate_tools(
            &state,
            Uuid::new_v4(),
            &servers,
            &[],
            &perms_enabling(&[id]),
            None,
        )
        .await
        .unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0]["name"],
            json!(format!("{}__good_tool", connector_prefix(id)))
        );
    }

    #[tokio::test]
    async fn duplicate_tool_names_from_one_backend_are_kept_not_deduped() {
        let mut m = mockito::Server::new_async().await;
        m.mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"dup"},{"name":"dup"}]}}"#,
            )
            .create_async()
            .await;

        let id = Uuid::new_v4();
        let servers = vec![srv(ServerType::Mcp, id, &format!("{}/mcp", m.url()))];
        let state = test_state();
        let merged = aggregate_tools(
            &state,
            Uuid::new_v4(),
            &servers,
            &[],
            &perms_enabling(&[id]),
            None,
        )
        .await
        .unwrap();

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
            enabled_connectors: HashSet::new(), // connector not enabled → tools dropped
            rules: vec![PermissionRule {
                connector_id: id,
                tool_pattern: "*".into(),
                stance: Stance::Allow,
            }],
            hash: "h".into(),
        };
        let merged = aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &perms, None)
            .await
            .unwrap();
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
            enabled_connectors: [id].into_iter().collect(),
            rules: vec![PermissionRule {
                connector_id: id,
                tool_pattern: "send_*".into(),
                stance: Stance::Block,
            }],
            hash: "h".into(),
        };
        let merged = aggregate_tools(&state, Uuid::new_v4(), &servers, &[], &perms, None)
            .await
            .unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0]["name"],
            json!(format!("{}__read_email", connector_prefix(id)))
        );
    }

    #[tokio::test]
    async fn system_server_tool_keeps_its_bare_name_but_non_system_stays_prefixed() {
        let mut sys_backend = mockito::Server::new_async().await;
        sys_backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"save_file"}]}}"#)
            .create_async()
            .await;
        let mut generic_backend = mockito::Server::new_async().await;
        generic_backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"send_email"}]}}"#)
            .create_async()
            .await;

        let sys_id = Uuid::new_v4();
        let generic_id = Uuid::new_v4();
        let mut sys = srv(
            ServerType::Mcp,
            sys_id,
            &format!("{}/mcp", sys_backend.url()),
        );
        sys.system = true;
        sys.tool_names = vec!["save_file".to_string()];
        let generic = srv(
            ServerType::Mcp,
            generic_id,
            &format!("{}/mcp", generic_backend.url()),
        );

        let state = test_state();
        let merged = aggregate_tools(
            &state,
            Uuid::new_v4(),
            &[sys, generic],
            &[],
            &perms_enabling(&[sys_id, generic_id]),
            None,
        )
        .await
        .unwrap();

        assert_eq!(merged.len(), 2);
        assert!(
            merged.iter().any(|t| t["name"] == json!("save_file")),
            "a system server's tool must keep its bare name: {merged:?}"
        );
        assert!(
            merged.iter().any(
                |t| t["name"] == json!(format!("{}__send_email", connector_prefix(generic_id)))
            ),
            "a non-system generic server's tool must still be prefixed: {merged:?}"
        );
    }

    /// Listing and routing must agree (review finding): a system backend can
    /// advertise a tool the synced `tool_names` catalog hasn't caught up to
    /// yet — `router::route_tool` would never hand a bare name for it to
    /// this backend, so listing it here would produce a tool the agent can
    /// call but that silently misroutes (to Composio, with its arguments) at
    /// `tools/call` time. Only catalog-listed tools may appear.
    #[tokio::test]
    async fn system_server_only_lists_tools_present_in_its_synced_catalog() {
        let mut backend = mockito::Server::new_async().await;
        backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"save_file"},{"name":"secret_tool"}]}}"#,
            )
            .create_async()
            .await;

        let sys_id = Uuid::new_v4();
        let mut sys = srv(ServerType::Mcp, sys_id, &format!("{}/mcp", backend.url()));
        sys.system = true;
        sys.tool_names = vec!["save_file".to_string()]; // catalog hasn't synced secret_tool

        let state = test_state();
        let merged = aggregate_tools(
            &state,
            Uuid::new_v4(),
            &[sys],
            &[],
            &perms_enabling(&[sys_id]),
            None,
        )
        .await
        .unwrap();

        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(merged[0]["name"], json!("save_file"));
    }
}
