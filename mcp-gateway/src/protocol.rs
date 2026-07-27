//! MCP JSON-RPC handlers for the aggregating gateway.
//!
//! Permission enforcement is two-layer, in order: Layer 1 — can the caller reach
//! this connector at all (owner/grant/composio); Layer 2 — is the tool allowed
//! for this agent. Composio meta-tools are never filtered at list; per-toolkit
//! enforcement happens here at `tools/call` via slug→connector resolution.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::aggregator;
use crate::error::McpError;
use crate::permissions::{self, PermissionContext, ToolAccess, toolkit_from_composio_slug};
use crate::provider::generic::DEFAULT_CALL_TIMEOUT;
use crate::router;
use crate::session::{self, ResolvedSession};
use crate::state::McpState;
use crate::types::{MCPServerConfig, PROTOCOL_VERSION, ServerType, codes};

fn ok(req_id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
}

fn err(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into() } })
}

/// Like [`err`] but carries a `data` object (e.g. the human-readable connector
/// name, so the route layer's approval flow event stays readable — id-based tool
/// prefixes are otherwise opaque).
fn err_data(req_id: &Value, code: i64, message: impl Into<String>, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": code, "message": message.into(), "data": data } })
}

/// Build a JSON-RPC error object (for the route layer's identity failures).
pub fn rpc_error(req_id: &Value, code: i64, message: impl Into<String>) -> Value {
    err(req_id, code, message)
}

/// Full JSON-RPC dispatch for the agent-facing gateway. Returns `None` for a
/// notification (a request with no `id`).
pub async fn handle_request(
    state: &McpState,
    user_id: Uuid,
    agent_id: Uuid,
    body: &Value,
    traceparent: Option<&str>,
) -> Option<Value> {
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");

    let Some(req_id) = body.get("id").cloned() else {
        tracing::debug!(method, "mcp notification");
        return None;
    };

    match method {
        "initialize" => return Some(handle_initialize(&req_id)),
        "ping" => return Some(json!({ "jsonrpc": "2.0", "id": req_id, "result": {} })),
        _ => {}
    }

    let perms = match permissions::load_permission_context(state, agent_id).await {
        Ok(p) => p,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };
    let resolved = match session::resolve_session(state, user_id).await {
        Ok(r) => r,
        Err(e) => return Some(err(&req_id, e.json_rpc_code(), e.to_json_rpc().message)),
    };

    let result = match method {
        "tools/list" => {
            handle_tools_list(
                state,
                user_id,
                &req_id,
                &resolved.servers,
                &resolved.connected_toolkits,
                &perms,
                traceparent,
            )
            .await
        }
        "tools/call" => {
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            handle_tools_call(
                state,
                user_id,
                &req_id,
                &params,
                &resolved,
                &perms,
                traceparent,
            )
            .await
        }
        other => err(
            &req_id,
            codes::METHOD_NOT_FOUND,
            format!("Method not found: {other}"),
        ),
    };
    Some(result)
}

/// `initialize` — gateway capability handshake.
pub fn handle_initialize(req_id: &Value) -> Value {
    ok(
        req_id,
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "MCP Gateway", "version": "1.0.0" },
        }),
    )
}

/// `tools/list` — aggregate, namespace, permission-filter, merge.
pub async fn handle_tools_list(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    servers: &[MCPServerConfig],
    connected_toolkits: &[String],
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Value {
    match aggregator::aggregate_tools(
        state,
        user_id,
        servers,
        connected_toolkits,
        perms,
        traceparent,
    )
    .await
    {
        Ok(tools) => ok(req_id, json!({ "tools": tools })),
        Err(e) => err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
    }
}

/// `tools/call` — route, enforce two-layer permissions, forward to the backend.
pub async fn handle_tools_call(
    state: &McpState,
    user_id: Uuid,
    req_id: &Value,
    params: &Value,
    resolved: &ResolvedSession,
    perms: &PermissionContext,
    traceparent: Option<&str>,
) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let (server, original) = match router::route_tool(tool_name, &resolved.servers) {
        Ok(pair) => pair,
        Err(e) => return err(req_id, codes::INVALID_PARAMS, e.to_string()),
    };

    // ── Generic MCP tool: Layer 1 (reachability) then Layer 2 (decide) ─────
    if server.kind == ServerType::Mcp {
        match state
            .authorizer
            .can_access_connector(&state.db, user_id, server.connector_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Connector for '{tool_name}' is not available."),
                );
            }
            Err(e) => return err(req_id, e.json_rpc_code(), e.to_json_rpc().message),
        }
        // Same decision `tools/list` filters on — a connector disabled for this
        // agent denies the call even though Layer 1 (owner/grant) still passes.
        match perms.decide(server.connector_id, &original) {
            ToolAccess::Denied => {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Tool '{tool_name}' is blocked or disabled for this agent."),
                );
            }
            ToolAccess::Ask => {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!(
                        "Tool '{tool_name}' requires user approval. Grant access in the agent settings."
                    ),
                    json!({ "server": server.name }),
                );
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio DIRECT tool call: enforce per-toolkit permission ───────────
    // A direct toolkit tool (e.g. GMAIL_SEND_EMAIL) resolves to its connector and
    // is subject to the same decide() as any other tool. Previously only the two
    // batch meta-tools below were checked, so a direct call bypassed enforcement
    // entirely (Round 3). Cross-toolkit meta-tools (COMPOSIO_SEARCH_TOOLS,
    // MANAGE_CONNECTIONS, MULTI_EXECUTE_TOOL) resolve to toolkit "composio", which
    // maps to no connector — they skip this block and are handled below / passed
    // through, exactly as before.
    if server.kind == ServerType::Composio
        && let Some(&cid) = resolved.toolkit_to_connector.get(&toolkit_from_composio_slug(tool_name))
    {
        match perms.decide(cid, tool_name) {
            ToolAccess::Denied => {
                return err(req_id, codes::TOOL_BLOCKED, format!("Tool '{tool_name}' is blocked or disabled for this agent."));
            }
            ToolAccess::Ask => {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool '{tool_name}' requires user approval. Grant access in the agent settings."),
                    json!({ "server": "composio" }),
                );
            }
            ToolAccess::Allowed => {}
        }
    }

    // ── Composio meta-tool interception (per-toolkit → connector) ───────────
    if tool_name == "COMPOSIO_MANAGE_CONNECTIONS"
        && let Some(requested) = arguments.get("toolkits").and_then(|v| v.as_array())
    {
        let requested: Vec<String> = requested.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let blocked: Vec<String> = requested
            .iter()
            .filter(|tk| connector_disabled(resolved, perms, tk))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            let allowed: Vec<String> = requested.iter().filter(|tk| !blocked.contains(tk)).cloned().collect();
            if allowed.is_empty() {
                return err(
                    req_id,
                    codes::TOOL_BLOCKED,
                    format!("Toolkit(s) are disabled for this agent: {blocked:?}."),
                );
            }
            arguments["toolkits"] = json!(allowed);
        }
    }

    if tool_name == "COMPOSIO_MULTI_EXECUTE_TOOL"
        && let Some(tools_arg) = arguments.get("tools").and_then(|v| v.as_array()).cloned()
        && !tools_arg.is_empty()
    {
        let mut allowed: Vec<Value> = Vec::new();
        let mut blocked_slugs: Vec<String> = Vec::new();
        let mut ask_slugs: Vec<String> = Vec::new();

        for t in &tools_arg {
            let slug = t.get("tool_slug").and_then(|v| v.as_str()).unwrap_or("");
            if slug.is_empty() {
                allowed.push(t.clone());
                continue;
            }
            let toolkit = toolkit_from_composio_slug(slug);
            match resolved.toolkit_to_connector.get(&toolkit) {
                Some(&cid) => match perms.decide(cid, slug) {
                    ToolAccess::Denied => blocked_slugs.push(slug.to_string()),
                    ToolAccess::Ask => ask_slugs.push(slug.to_string()),
                    ToolAccess::Allowed => allowed.push(t.clone()),
                },
                None => allowed.push(t.clone()),
            }
        }

        if allowed.is_empty() && (!blocked_slugs.is_empty() || !ask_slugs.is_empty()) {
            if !ask_slugs.is_empty() {
                return err_data(
                    req_id,
                    codes::TOOL_ASK,
                    format!("Tool(s) require user approval for this agent: {ask_slugs:?}."),
                    json!({ "server": "composio" }),
                );
            }
            return err(
                req_id,
                codes::TOOL_BLOCKED,
                format!("All requested tools are blocked for this agent: {blocked_slugs:?}."),
            );
        }
        if !blocked_slugs.is_empty() || !ask_slugs.is_empty() {
            tracing::info!(user = %user_id, agent = %perms.agent_id, ?blocked_slugs, ?ask_slugs, forwarding = allowed.len(), "partial composio multi-execute filter");
            arguments["tools"] = json!(allowed);
        }
    }

    tracing::info!(tool = %tool_name, forwarded_as = %original, kind = ?server.kind, "routing tool call");

    match state
        .providers
        .mcp
        .call_tool(server, req_id, &original, &arguments, DEFAULT_CALL_TIMEOUT, traceparent)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            // Self-heal: an uploaded_build connector's container can move
            // (restart/redeploy/reboot) between build time and this call. On
            // a connection-level failure (not an application-level MCP
            // error), ask the refresher for the container's current live
            // address and retry exactly once before giving up — mirrors this
            // gateway's own existing precedent for the structurally
            // identical Composio-connection staleness problem (refresh only
            // on-demand, never on every request).
            if server.trusted && is_connection_level_failure(&e)
                && let Some(new_url) = state.endpoint_refresher.refresh(server.connector_id).await
            {
                tracing::info!(server = %server.name, connector_id = %server.connector_id, "endpoint stale — retrying tool call against refreshed address");
                let mut refreshed = server.clone();
                refreshed.url = new_url;
                match state
                    .providers
                    .mcp
                    .call_tool(&refreshed, req_id, &original, &arguments, DEFAULT_CALL_TIMEOUT, traceparent)
                    .await
                {
                    Ok(response) => return response,
                    Err(e2) => {
                        tracing::warn!(server = %server.name, tool = %original, error = %e2, "backend tool call failed again after endpoint refresh");
                        return err(req_id, codes::INTERNAL_ERROR, format!("Backend '{}' failed to execute '{}'", server.name, original));
                    }
                }
            }
            tracing::warn!(server = %server.name, tool = %original, error = %e, "backend tool call failed");
            err(req_id, codes::INTERNAL_ERROR, format!("Backend '{}' failed to execute '{}'", server.name, original))
        }
    }
}

/// A connection-level failure (refused/timeout/DNS) — as opposed to an
/// application-level MCP error (a well-formed error response from a live
/// server) — is the only case worth refreshing the endpoint for; nothing else
/// indicates the address itself is stale.
fn is_connection_level_failure(e: &McpError) -> bool {
    matches!(e, McpError::Http(re) if re.is_connect() || re.is_timeout())
}

/// True when a Composio toolkit maps to a connector that is disabled for the agent.
fn connector_disabled(resolved: &ResolvedSession, perms: &PermissionContext, toolkit: &str) -> bool {
    resolved
        .toolkit_to_connector
        .get(&toolkit.to_ascii_lowercase())
        .map(|cid| !perms.is_connector_enabled(*cid))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::McpConfig;
    use crate::permissions::PermissionRule;
    use crate::provider::{GenericMcpProvider, Providers};
    use crate::types::Stance;

    fn test_state() -> McpState {
        let db = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("lazy pool construction must not touch the network");
        let redis = redis::Client::open("redis://127.0.0.1:1/").expect("lazy redis client");
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

    /// A resolved session with one Composio backend at `url` and a single
    /// connected `gmail` toolkit mapped to `cid`.
    fn gmail_session(url: &str, cid: Uuid) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: Uuid::nil(),
                kind: ServerType::Composio,
                name: "composio".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
                trusted: false,
            }],
            connected_toolkits: vec!["gmail".into()],
            toolkit_to_connector: HashMap::from([("gmail".to_string(), cid)]),
        }
    }

    /// `enabled` must be given explicitly — under the default-deny allowlist,
    /// a connector referenced only by `rules` (with no tool rule at all, e.g.
    /// a bare "this connector is on" case) can't be inferred from `rules`
    /// alone.
    fn perms(enabled: &[Uuid], rules: Vec<PermissionRule>) -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: enabled.iter().copied().collect(),
            rules,
            hash: "h".into(),
        }
    }

    fn rule(cid: Uuid, pat: &str, stance: Stance) -> PermissionRule {
        PermissionRule { connector_id: cid, tool_pattern: pat.into(), stance }
    }

    /// Layer-1 stub that always allows — the real `OssConnectorAuthorizer`
    /// hits `state.db`, which `test_state()`'s lazily-connected pool can't
    /// actually reach; tests exercising the generic-MCP (`ServerType::Mcp`)
    /// path need this instead.
    struct AllowAllAuthorizer;
    #[async_trait::async_trait]
    impl crate::authorizer::ConnectorAuthorizer for AllowAllAuthorizer {
        async fn can_access_connector(&self, _db: &sqlx::PgPool, _user_id: Uuid, _connector_id: Uuid) -> crate::error::Result<bool> {
            Ok(true)
        }
        async fn list_accessible_connectors(&self, _db: &sqlx::PgPool, _user_id: Uuid) -> crate::error::Result<Vec<crate::repo::McpConnector>> {
            Ok(vec![])
        }
        async fn list_accessible_mcp_connectors(
            &self,
            _db: &sqlx::PgPool,
            _user_id: Uuid,
        ) -> crate::error::Result<Vec<crate::repo::McpConnector>> {
            Ok(vec![])
        }
        async fn list_access_reasons(
            &self,
            _db: &sqlx::PgPool,
            _connector: &crate::repo::McpConnector,
        ) -> crate::error::Result<Vec<crate::types::AccessReason>> {
            Ok(vec![])
        }
        async fn list_org_grant_consumers(
            &self,
            _db: &sqlx::PgPool,
            _connector_id: Uuid,
        ) -> crate::error::Result<(Vec<crate::types::OrgGrantConsumer>, Vec<crate::types::OrgGrantConsumer>)> {
            Ok((vec![], vec![]))
        }
    }

    /// Always refreshes to a fixed URL — a fake
    /// [`crate::endpoint_refresh::EndpointRefresher`] standing in for the
    /// real `ContainerRuntime`-backed one (`oss/server`'s
    /// `RuntimeEndpointRefresher`, not constructible from this crate).
    struct FakeRefresher(String);
    #[async_trait::async_trait]
    impl crate::endpoint_refresh::EndpointRefresher for FakeRefresher {
        async fn refresh(&self, _connector_id: Uuid) -> Option<String> {
            Some(self.0.clone())
        }
    }

    /// A resolved session with one generic MCP backend (`ServerType::Mcp`,
    /// `trusted`) at `url`, namespaced under `cid`'s connector prefix.
    fn mcp_session(url: &str, cid: Uuid, trusted: bool) -> ResolvedSession {
        ResolvedSession {
            servers: vec![MCPServerConfig {
                connector_id: cid,
                kind: ServerType::Mcp,
                name: "uploaded-server".into(),
                url: url.into(),
                headers: HashMap::new(),
                transport: "streamable_http".into(),
                trusted,
            }],
            connected_toolkits: vec![],
            toolkit_to_connector: HashMap::new(),
        }
    }

    /// A mockito server answering `POST /mcp` with a successful JSON-RPC
    /// response — the "refreshed, now-reachable" address a retry lands on.
    /// Uses a `localhost`-hostname URL, not mockito's raw `127.0.0.1` form,
    /// per Step 7's own established gotcha (the first attempt to fail
    /// against a loopback URL): reqwest/hyper's normal request path handles
    /// both equally for a real request (unlike the SSRF guard's custom
    /// `Resolve` trait, which only fires for hostnames) — matching that
    /// convention here regardless, for consistency with this crate's other
    /// tests.
    async fn spawn_ok_backend() -> (mockito::ServerGuard, String) {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .create_async()
            .await;
        let url = format!("http://localhost:{}/mcp", server.socket_address().port());
        (server, url)
    }

    // ── Step 13: endpoint self-heal on connection failure ────────────────────

    #[tokio::test]
    async fn trusted_backend_connection_failure_retries_against_refreshed_endpoint() {
        let (_guard, fresh_url) = spawn_ok_backend().await;
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        state.endpoint_refresher = std::sync::Arc::new(FakeRefresher(fresh_url));

        let cid = Uuid::new_v4();
        // Port 1 is a well-known refused-connection target — this is a
        // genuine connection-level failure, not an application error.
        let resolved = mcp_session("http://127.0.0.1:1/mcp", cid, true);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(&state, Uuid::new_v4(), &json!(1), &json!({ "name": tool, "arguments": {} }), &resolved, &p, None).await;

        assert_eq!(res["result"]["ok"], json!(true), "must succeed after retrying against the refreshed endpoint: {res}");
    }

    #[tokio::test]
    async fn untrusted_backend_connection_failure_never_retries() {
        // An external_url connector (trusted=false) must never trigger a
        // refresh, even if the refresher would happily hand back a working
        // URL — refresh only ever applies to uploaded_build connectors.
        let (_guard, fresh_url) = spawn_ok_backend().await;
        let mut state = test_state();
        state.authorizer = std::sync::Arc::new(AllowAllAuthorizer);
        state.endpoint_refresher = std::sync::Arc::new(FakeRefresher(fresh_url));

        let cid = Uuid::new_v4();
        let resolved = mcp_session("http://127.0.0.1:1/mcp", cid, false);
        let p = perms(&[cid], vec![]);
        let tool = format!("{}__echo", crate::types::connector_prefix(cid));

        let res = handle_tools_call(&state, Uuid::new_v4(), &json!(1), &json!({ "name": tool, "arguments": {} }), &resolved, &p, None).await;

        assert_eq!(res["error"]["code"], json!(codes::INTERNAL_ERROR), "must surface the original failure, never retry: {res}");
    }

    // ── Round 3: direct Composio tool calls must be permission-enforced ──────

    #[tokio::test]
    async fn composio_direct_tool_with_block_rule_is_denied_before_backend() {
        // url points nowhere reachable — a Denied decision must return before any
        // backend call, so this must not hang or error on the network.
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[cid], vec![rule(cid, "GMAIL_SEND_*", Stance::Block)]);
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_on_disabled_connector_is_denied() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[], vec![]); // never enabled for the agent
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res}");
    }

    #[tokio::test]
    async fn composio_direct_tool_with_ask_rule_returns_tool_ask() {
        let cid = Uuid::new_v4();
        let resolved = gmail_session("http://127.0.0.1:9/mcp", cid);
        let p = perms(&[cid], vec![rule(cid, "*", Stance::Ask)]);
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res}");
    }

    #[tokio::test]
    async fn composio_allowed_direct_tool_reaches_backend() {
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(&[cid], vec![]); // explicitly enabled, no tool rules
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "GMAIL_SEND_EMAIL", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert_eq!(res["result"]["ok"], json!(true), "{res}");
        hit.assert_async().await;
    }

    #[tokio::test]
    async fn composio_cross_toolkit_metatool_is_not_caught_by_per_toolkit_check() {
        // COMPOSIO_SEARCH_TOOLS resolves to toolkit "composio" (no connector), so
        // even with the gmail connector disabled it must NOT be denied by the
        // per-toolkit check — it proceeds to the backend (meta-tools are not
        // connector-scoped; MANAGE_CONNECTIONS/MULTI_EXECUTE do their own filtering).
        let mut backend = mockito::Server::new_async().await;
        let hit = backend
            .mock("POST", "/mcp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
            .expect(1)
            .create_async()
            .await;

        let cid = Uuid::new_v4();
        let resolved = gmail_session(&format!("{}/mcp", backend.url()), cid);
        let p = perms(&[], vec![]); // gmail not enabled — irrelevant to a meta-tool
        let res = handle_tools_call(
            &test_state(),
            Uuid::new_v4(),
            &json!(1),
            &json!({ "name": "COMPOSIO_SEARCH_TOOLS", "arguments": {} }),
            &resolved,
            &p,
            None,
        )
        .await;
        assert!(res.get("error").is_none(), "a cross-toolkit meta-tool must not be blocked by the per-toolkit check: {res}");
        hit.assert_async().await;
    }
}
