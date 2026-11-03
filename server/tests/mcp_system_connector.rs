//! `provider_type='system'` connector rows — schema-only coverage for
//! `0054_workspace.sql`, plus an end-to-end proof that a system connector
//! actually works through the real gateway.
//!
//! `chk_connectors_provider_fields` (0003_mcp.sql) originally allowed only the
//! `composio`/`mcp_server` field combinations; a `provider_type='system'` row
//! (however `source_kind`/`auth_type` were set) was rejected by that CHECK
//! even after `mcp_connectors_provider_type_check` was widened to permit the
//! value. The three tests below prove the migration's follow-up fix —
//! dropping and re-adding `chk_connectors_provider_fields` with a `system`
//! clause — lets a minimal system-connector row actually insert, and that a
//! NULL `url` / non-`none` `auth_type` are still rejected by the same clause.
//!
//! `system_connector_end_to_end_through_the_real_gateway` below is the
//! acceptance test for a system connector: it inserts the rows a real deployment
//! would use (public grant, synced tool catalog, per-agent access) against a
//! real stub MCP backend and drives `/api/mcp` exactly as a deployed agent
//! would — no route or application code is exercised in the three
//! schema-only tests above.

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use common::TestServer;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// SAFETY: every test in this file is `#[serial]`, so no other test observes
/// the env var mid-mutation. Same convention as `mcp_e2e_agent_flow.rs`,
/// `mcp_connectors.rs`, `mcp_credentials.rs`, `mcp_oauth.rs`,
/// `mcp_permissions_v2.rs` — each keeps its own copy rather than sharing one
/// through `common`.
fn allow_private_urls() {
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

#[tokio::test]
#[serial]
async fn a_minimal_system_connector_row_can_be_inserted() {
    let server = TestServer::start().await;

    // Confirms the server actually came up (migrations included) before the
    // direct-SQL assertion below — this test's whole point is the migration.
    let health = server
        .client
        .get(server.url("/health"))
        .send()
        .await
        .expect("health check must be reachable");
    assert!(health.status().is_success());

    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'system', 'workspace-test-connector', 'http://127.0.0.1:1/mcp', 'none') \
         RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .expect("a minimal provider_type='system' row must satisfy every CHECK on mcp_connectors");

    let (provider_type, source_kind): (String, String) =
        sqlx::query_as("SELECT provider_type, source_kind::text FROM mcp_connectors WHERE id = $1")
            .bind(id)
            .fetch_one(&server.db)
            .await
            .expect("row must be readable back");
    assert_eq!(provider_type, "system");
    assert_eq!(source_kind, "system");

    server.cleanup().await;
}

/// `chk_connectors_provider_fields`'s `system` clause requires `url IS NOT
/// NULL` — a system connector's whole point is a real loopback address, never
/// the "not yet built" NULL a `source_kind='uploaded_build'` row can have.
#[tokio::test]
#[serial]
async fn a_system_connector_row_with_null_url_is_rejected() {
    let server = TestServer::start().await;

    let result = sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'system', 'workspace-null-url', NULL, 'none')",
    )
    .execute(&server.db)
    .await;

    assert!(
        result.is_err(),
        "a system row with NULL url must violate chk_connectors_provider_fields"
    );

    server.cleanup().await;
}

/// The same clause requires `auth_type IS NOT DISTINCT FROM 'none'` — a
/// system connector is served by the control plane itself with no per-user
/// credential (`credentials::build_server_config` never reaches any other
/// auth_type arm for one), so any other value must be rejected.
#[tokio::test]
#[serial]
async fn a_system_connector_row_with_bearer_auth_type_is_rejected() {
    let server = TestServer::start().await;

    let result = sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'system', 'workspace-bearer-auth', 'http://127.0.0.1:1/mcp', 'bearer')",
    )
    .execute(&server.db)
    .await;

    let err = result.expect_err(
        "a system row with auth_type='bearer' must violate chk_connectors_provider_fields",
    );
    assert!(
        err.to_string().contains("chk_connectors_provider_fields"),
        "error must name the violated constraint: {err}"
    );

    server.cleanup().await;
}

/// `auth_type IS NOT DISTINCT FROM 'none'` (not a bare `=`) is exactly what
/// makes a NULL `auth_type` fail too: `NULL IS NOT DISTINCT FROM 'none'`
/// evaluates to FALSE (unlike `NULL = 'none'`, which evaluates to NULL and
/// would let the row pass a CHECK). This is the migration's whole point — the
/// bearer test above only proves *some* non-'none' value is rejected; this
/// proves the NULL case the `IS NOT DISTINCT FROM` rewrite specifically exists
/// for is rejected too.
#[tokio::test]
#[serial]
async fn a_system_connector_row_with_null_auth_type_is_rejected() {
    let server = TestServer::start().await;

    let result = sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'system', 'workspace-null-auth-type', 'http://127.0.0.1:1/mcp', NULL)",
    )
    .execute(&server.db)
    .await;

    let err = result
        .expect_err("a system row with NULL auth_type must violate chk_connectors_provider_fields");
    assert!(
        err.to_string().contains("chk_connectors_provider_fields"),
        "error must name the violated constraint: {err}"
    );

    server.cleanup().await;
}

/// The `system` clause also requires `source_kind::text = 'system'` — ties the
/// two columns together so a `provider_type='system'` row can't pair with, say,
/// `source_kind='external_url'` (a combination nothing else in the schema rules
/// out on its own, since `source_kind='external_url'` is otherwise valid for
/// `provider_type='mcp_server'`).
#[tokio::test]
#[serial]
async fn a_system_connector_row_with_external_url_source_kind_is_rejected() {
    let server = TestServer::start().await;

    let result = sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('system', 'external_url', 'workspace-wrong-source-kind', 'http://127.0.0.1:1/mcp', 'none')",
    )
    .execute(&server.db)
    .await;

    let err = result.expect_err(
        "a system row with source_kind='external_url' must violate chk_connectors_provider_fields",
    );
    assert!(
        err.to_string().contains("chk_connectors_provider_fields"),
        "error must name the violated constraint: {err}"
    );

    server.cleanup().await;
}

// ─── End-to-end: a system connector through the real gateway ──────────────

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

async fn seed_agent(server: &TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

/// Names of the two tools the stub system backend advertises.
const SAVE_FILE: &str = "save_file";
const LIST_FILES: &str = "list_files";

/// The `mcp_connectors.instructions` column's value — the source of truth the
/// gateway must forward. Deliberately different from
/// [`STUB_LIVE_INITIALIZE_INSTRUCTIONS`] below so the assertion can't pass by
/// coincidence.
const DB_INSTRUCTIONS: &str = "WS-E2E-INSTR-FROM-DB-COLUMN";

/// What the stub backend's own live `initialize` response advertises — never
/// what the gateway is supposed to forward (`connector.instructions`, harvested
/// once at registration/probe time, is the source of truth; the gateway never
/// re-probes a system backend's `initialize` per request).
const STUB_LIVE_INITIALIZE_INSTRUCTIONS: &str = "WS-E2E-INSTR-FROM-STUB-LIVE-INITIALIZE";

/// Tracks every `tools/call` the stub backend actually received, by tool name.
type CallLog = Arc<Mutex<Vec<String>>>;

/// Every request the stub backend received, in arrival order, as `(method,
/// x-nasiko-identity header value)` — lets a test single out, say, "the
/// identity header the stub saw on the `tools/call` for save_file", not just
/// "some header was present somewhere", and separately prove a given method
/// (or a given, differently-configured backend) received none at all.
type HeaderLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A real MCP JSON-RPC backend standing in for a platform-owned system
/// connector: answers `initialize` with a fixed `instructions` string,
/// `tools/list` with `save_file`/`list_files`, and records every `tools/call`
/// plus every request's `x-nasiko-identity` header (present or not — this
/// same stub also stands in for a NON-system backend later in this file,
/// where the absence is exactly what's being proved). Same shape as
/// `mcp_e2e_agent_flow.rs::start_stub_mcp_backend` — a `provider_type='system'`
/// connector is `trusted` (see `credentials::build_server_config`), so its
/// calls bypass the SSRF guard entirely regardless of URL; unlike that file's
/// test, no `MCP_ALLOW_PRIVATE_URLS` / `allow_private_urls()` dance is needed
/// for the *system* connector below, because this test never goes through the
/// guarded connector-registration HTTP route (`POST /api/mcp/connectors`) for
/// it — the row is inserted directly by SQL, exactly as the platform itself
/// would create one. The non-system connector added later in this file DOES
/// need that dance, same as `mcp_e2e_agent_flow.rs`, since it's an ordinary
/// (untrusted) generic connector.
async fn start_stub_system_backend() -> (String, CallLog, HeaderLog) {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));
    let header_log: HeaderLog = Arc::new(Mutex::new(Vec::new()));

    async fn handle(
        State((calls, header_log)): State<(CallLog, HeaderLog)>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let identity = headers
            .get(nasiko_mcp_gateway::identity::IDENTITY_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        header_log.lock().unwrap().push((method.clone(), identity));

        match method.as_str() {
            "initialize" => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "serverInfo": {"name": "workspace-stub", "version": "1.0"},
                    "instructions": STUB_LIVE_INITIALIZE_INSTRUCTIONS,
                },
            })),
            "tools/list" => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "tools": [
                        {"name": SAVE_FILE, "description": "save a file", "inputSchema": {"type": "object"}},
                        {"name": LIST_FILES, "description": "list files", "inputSchema": {"type": "object"}},
                    ]
                },
            })),
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or("").to_string();
                // Echoed back so a test can see which arguments actually
                // arrived, not just which tool.
                let arguments = body["params"]["arguments"].clone();
                calls.lock().unwrap().push(name.clone());
                Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {"content": [{"type": "text", "text": format!("stub executed '{name}' with {arguments}")}]},
                }))
            }
            other => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("stub: method not found: {other}")},
            })),
        }
    }

    let app = Router::new()
        .route("/mcp", post(handle))
        .with_state((calls.clone(), header_log.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{port}/mcp"), calls, header_log)
}

/// The rows a deployment carries for one system connector — the connector
/// itself, a public grant, the synced two-tool catalog, and `agent_id`'s
/// access row with `tool_rules` — inserted by SQL exactly as the platform
/// would. Returns the connector id.
async fn seed_system_connector(
    server: &TestServer,
    backend_url: &str,
    name: &str,
    agent_id: Uuid,
    tool_rules: Value,
) -> Uuid {
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', $1, $2, 'none', 'instr') \
         RETURNING id",
    )
    .bind(name)
    .bind(backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled, tool_rules) \
         VALUES ($1, $2, true, $3)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .bind(tool_rules)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    connector_id
}

/// The acceptance test for a `provider_type='system'`
/// connector — the synced catalog (`mcp_connector_tools`), a public grant,
/// and a per-agent access row, all inserted exactly as the platform itself
/// would (via SQL, not the connector-registration API, which rejects any
/// `provider_type` other than `mcp_server` — see `net.rs`'s and
/// `types.rs`'s `trusted` doc comments) — driven through the real
/// `/api/mcp` gateway exactly as a deployed agent would: its gateway
/// bearer token plus the traceparent of a live flow it participates in.
#[tokio::test]
#[serial]
async fn system_connector_end_to_end_through_the_real_gateway() {
    // The default test config's `mcp_tool_search_mode` (`""`) parses to
    // `ToolSearchMode::Semantic` (`config::ToolSearchMode::parse`'s fallback
    // for an unrecognized value) — under which `tools/list` returns only
    // search-index matches plus the `nasiko_search_tools` meta-tool, never
    // the full manifest (`protocol::handle_tools_list`). This test asserts
    // the literal bare tool-name set the manifest carries, so it needs the
    // legacy eager-fan-out path; disable search explicitly, the same
    // `TestServer::start_with` override mechanism
    // `mcp_session_grant_stability.rs` uses for its own `McpConfig`.
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-e2e-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-e2e-agent").await;

    let (backend_url, backend_calls, backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'workspace-e2e-connector', $1, 'none', $2) \
         RETURNING id",
    )
    .bind(&backend_url)
    .bind(DB_INSTRUCTIONS)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (flow_id, traceparent) = common::open_flow(&server.db, owner, agent_id).await;

    let mcp = |body: Value| {
        server
            .client
            .post(server.url("/api/mcp"))
            .bearer_auth(&token)
            .header("traceparent", &traceparent)
            .json(&body)
    };

    // ── initialize: the DB column's instructions ride along, never the stub's
    //    own live `initialize` response — the gateway forwards what was
    //    harvested into `mcp_connectors.instructions` at registration/probe
    //    time, not a live re-probe of the backend.
    let res = mcp(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let instructions = body["result"]["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains(DB_INSTRUCTIONS),
        "initialize must forward the connector row's DB instructions: {body:?}"
    );
    assert!(
        !instructions.contains(STUB_LIVE_INITIALIZE_INSTRUCTIONS),
        "initialize must never forward the stub's own live `initialize` instructions \
         — the DB column is the source of truth: {body:?}"
    );

    // ── tools/list: exactly the two synced tools, bare (no connector prefix) ──
    let res = mcp(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![LIST_FILES, SAVE_FILE],
        "tools/list must carry exactly the synced catalog's tools, un-namespaced: {body:?}"
    );

    // ── tools/call save_file: reaches the stub, by its bare name ────────────
    let res = mcp(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": SAVE_FILE, "arguments": {}},
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "save_file call must not error: {body:?}"
    );
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &[SAVE_FILE.to_string()],
        "the stub must have recorded exactly one tools/call, for save_file"
    );

    // ── prove the identity header on the wire: the save_file `tools/call`
    //    the stub just logged must carry a header this test can verify with
    //    the same key `Config` derives (`test_config`'s `Config` literal sets
    //    `mcp_identity_signing_key` directly to `TEST_JWT_SECRET`, bypassing
    //    `Config::from_env`'s own JWT_SECRET-derivation — see that field's
    //    doc comment in `oss/server/tests/common/mod.rs`), and it must name
    //    exactly this call's agent, this flow's user, and this flow's trace id.
    let identity_header = {
        let logged = backend_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("stub must have logged the tools/call request");
        header
            .clone()
            .expect("a system backend must receive x-nasiko-identity on tools/call")
    };
    let verified = nasiko_mcp_gateway::identity::SignedIdentity::verify(
        &identity_header,
        common::TEST_JWT_SECRET.as_bytes(),
    )
    .expect("the gateway's own signature must verify with the test signing key");
    assert_eq!(verified.agent_id, agent_id);
    assert_eq!(verified.user_id, owner);
    assert_eq!(
        verified.flow_id.as_deref(),
        Some(flow_id.as_str()),
        "the signed identity must carry this flow's own trace id"
    );

    // ── tools/call COMPOSIO_SEARCH_TOOLS: never reaches this stub ───────────
    let res = mcp(json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "COMPOSIO_SEARCH_TOOLS", "arguments": {}},
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "JSON-RPC errors are still HTTP 200");
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &[SAVE_FILE.to_string()],
        "the stub's call log must be unchanged — a Composio meta-tool name \
         must never reach the system backend, whatever error the gateway \
         itself returns for it"
    );

    // ── the identity header must never reach a NON-system backend ───────────
    // Same stub implementation, registered as an ordinary (untrusted)
    // `provider_type='mcp_server'` connector this time — proves the header is
    // bound to `server.system`, not to "any backend this test happens to
    // control". An ordinary connector isn't `trusted` (see
    // `credentials::build_server_config`), so its loopback URL needs the same
    // `MCP_ALLOW_PRIVATE_URLS` escape hatch `mcp_e2e_agent_flow.rs` uses.
    //
    // A second agent (not `agent_id`) drives this call, on its own gateway
    // token and its own live flow — deliberately, not a reuse of `agent_id`'s:
    // `load_permission_context` is Redis-cached per agent
    // (`permissions::load_permission_context`), and `agent_id`'s context was
    // already cached by the calls above, before this connector's access row
    // existed. A fresh agent means a fresh cache key, so this assertion can't
    // pass or fail on cache staleness either way.
    allow_private_urls();
    let (non_system_url, non_system_calls, non_system_headers) = start_stub_system_backend().await;

    let non_system_connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type) \
         VALUES ('mcp_server', 'external_url', 'ws-e2e-non-system-connector', $1, 'none') \
         RETURNING id",
    )
    .bind(&non_system_url)
    .fetch_one(&server.db)
    .await
    .expect("insert non-system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(non_system_connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant for non-system connector");

    let non_system_agent_id = seed_agent(&server, owner, "ws-e2e-non-system-agent").await;
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(non_system_agent_id)
    .bind(non_system_connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access for non-system connector");

    let non_system_token = common::mint_gateway_token(&server.db, non_system_agent_id).await;
    let (_, non_system_traceparent) =
        common::open_flow(&server.db, owner, non_system_agent_id).await;
    let non_system_prefix = nasiko_mcp_gateway::types::connector_prefix(non_system_connector_id);
    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&non_system_token)
        .header("traceparent", &non_system_traceparent)
        .json(&json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": format!("{non_system_prefix}__{SAVE_FILE}"), "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "the non-system connector's namespaced tools/call must not error: {body:?}"
    );
    assert_eq!(
        non_system_calls.lock().unwrap().as_slice(),
        &[SAVE_FILE.to_string()],
        "the non-system stub must have recorded the call"
    );
    let non_system_identity = {
        let logged = non_system_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("non-system stub must have logged the tools/call request");
        header.clone()
    };
    assert!(
        non_system_identity.is_none(),
        "a non-system backend must never receive x-nasiko-identity: {non_system_identity:?}"
    );
    disallow_private_urls();

    server.cleanup().await;
}

/// On the gateway's owner-fallback path (`oss/server/src/mcp/handlers/gateway.rs::dispatch`) —
/// reached here by a `tools/list` whose `traceparent` is syntactically well-formed (passes
/// `FlowContext::from_traceparent`'s shape check) but names no row in `flows` at all — the
/// identity signed for the system backend must carry `flow_id: None`, never the bogus trace id
/// lifted from that unverified header: `handle_request` must not re-parse the raw `traceparent`
/// itself and sign that trace id as if it had been verified.
///
/// `tools/list` (unlike `tools/call`) is exempt from the flow requirement —
/// rule 2 in `mcp_gateway_auth.rs`'s doc comment — so this must return 200
/// via the owner-fallback, not 403.
#[tokio::test]
#[serial]
async fn owner_fallback_tools_list_with_a_bogus_traceparent_signs_flow_id_none() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-owner-fallback-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-owner-fallback-agent").await;

    let (backend_url, _backend_calls, backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'ws-owner-fallback-connector', $1, 'none', 'instr') \
         RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let token = common::mint_gateway_token(&server.db, agent_id).await;

    // Well-formed (32 hex trace id, 16 hex parent id — passes
    // `FlowContext::from_traceparent`'s shape check) but names no row in
    // `flows` at all: `flow_user` must reject it, forcing the owner-fallback
    // path for this read-only method.
    let bogus_traceparent = format!("00-{}-{}-01", "b".repeat(32), "c".repeat(16));

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &bogus_traceparent)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "tools/list must succeed via the owner-fallback path even though the \
         traceparent doesn't resolve to a live flow"
    );

    let identity_header = {
        let logged = backend_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/list")
            .expect("stub must have logged the tools/list request");
        header
            .clone()
            .expect("system backend must receive x-nasiko-identity on tools/list too")
    };
    let verified = nasiko_mcp_gateway::identity::SignedIdentity::verify(
        &identity_header,
        common::TEST_JWT_SECRET.as_bytes(),
    )
    .expect("the gateway's own signature must verify with the test signing key");
    assert_eq!(verified.agent_id, agent_id);
    assert_eq!(
        verified.user_id, owner,
        "the owner-fallback path's user is the agent's owner"
    );
    assert_eq!(
        verified.flow_id, None,
        "the owner-fallback path must sign flow_id: None, never the bogus \
         trace id lifted from an unverified traceparent"
    );

    server.cleanup().await;
}

/// A local coding agent's row (`coding_agent_integration_id` set — no CLI ever dispatches it
/// through a flow) makes a flow-less `tools/call` and the identity signed for a system backend
/// carries the agent's *owner* as `user_id` and `flow_id: None` — the same policy
/// `owner_fallback_tools_list_with_a_bogus_traceparent_signs_flow_id_none` proves for the
/// read-only owner-fallback, but here for `tools/call` specifically, which every OTHER flow-less
/// agent is still refused for
/// (`mcp_gateway_auth.rs::coding_agent_row_without_flow_resolves_to_owner_for_tools_call`
/// proves that half at the HTTP layer).
#[tokio::test]
#[serial]
async fn coding_agent_flowless_tools_call_to_system_connector_signs_owner_identity() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-coding-agent-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-coding-agent").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("stamp coding_agent_integration_id");

    let (backend_url, _backend_calls, backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'ws-coding-agent-connector', $1, 'none', 'instr') \
         RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let token = common::mint_gateway_token(&server.db, agent_id).await;

    // No traceparent at all — a local coding agent, never dispatched through
    // the proxy, never has one.
    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": SAVE_FILE, "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "a coding-agent row's flow-less tools/call must be admitted, not 403'd"
    );
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "save_file call must not error: {body:?}"
    );

    let identity_header = {
        let logged = backend_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("stub must have logged the tools/call request");
        header
            .clone()
            .expect("system backend must receive x-nasiko-identity on tools/call")
    };
    let verified = nasiko_mcp_gateway::identity::SignedIdentity::verify(
        &identity_header,
        common::TEST_JWT_SECRET.as_bytes(),
    )
    .expect("the gateway's own signature must verify with the test signing key");
    assert_eq!(verified.agent_id, agent_id);
    assert_eq!(
        verified.user_id, owner,
        "the coding-agent owner policy's user must be the agent's owner"
    );
    assert_eq!(
        verified.flow_id, None,
        "a coding-agent row has no flow — flow_id must be None, never a \
         fabricated value"
    );

    server.cleanup().await;
}

/// The owner policy is a fallback, never an override. When a coding-agent row DOES have a live
/// flow, that flow's own
/// user and flow id are used — seeded here for a user other than the agent's
/// owner, so the assertion is meaningful (the owner policy would give the
/// wrong answer if it ever won this race).
#[tokio::test]
#[serial]
async fn coding_agent_row_with_a_live_flow_uses_the_flow_not_the_owner() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-coding-agent-flow-owner").await;
    let flow_user = seed_user(&server, "ws-coding-agent-flow-user").await;
    let agent_id = seed_agent(&server, owner, "ws-coding-agent-flow").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("stamp coding_agent_integration_id");

    let (backend_url, _backend_calls, backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'ws-coding-agent-flow-connector', $1, 'none', 'instr') \
         RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (flow_id, traceparent) = common::open_flow(&server.db, flow_user, agent_id).await;

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &traceparent)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": SAVE_FILE, "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "save_file call must not error: {body:?}"
    );

    let identity_header = {
        let logged = backend_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("stub must have logged the tools/call request");
        header
            .clone()
            .expect("system backend must receive x-nasiko-identity on tools/call")
    };
    let verified = nasiko_mcp_gateway::identity::SignedIdentity::verify(
        &identity_header,
        common::TEST_JWT_SECRET.as_bytes(),
    )
    .expect("the gateway's own signature must verify with the test signing key");
    assert_eq!(verified.agent_id, agent_id);
    assert_eq!(
        verified.user_id, flow_user,
        "a live flow's own user must win over the coding-agent owner policy \
         — the policy is a fallback, never an override"
    );
    assert_ne!(
        verified.user_id, owner,
        "sanity: the flow's user and the agent's owner must differ, or this \
         assertion couldn't distinguish the two"
    );
    assert_eq!(
        verified.flow_id.as_deref(),
        Some(flow_id.as_str()),
        "the signed identity must carry the live flow's own trace id"
    );

    server.cleanup().await;
}

/// The owner policy's widening covers not just an *unknown* trace id but a REAL, live flow the
/// coding-agent row simply isn't a participant of — a different user's,
/// dispatched to a different agent entirely. Naming it must not let the
/// coding-agent row ride along on it: `flow_user` still denies (rule 4, not a
/// participant), exactly as it would for a plain deployed agent, and only
/// then does the coding-agent owner policy take over — resolving to its OWN
/// owner, never the foreign flow's user or trace id.
#[tokio::test]
#[serial]
async fn coding_agent_row_with_a_foreign_live_flow_still_resolves_to_owner() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-coding-agent-foreign-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-coding-agent-foreign").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("stamp coding_agent_integration_id");

    let (backend_url, _backend_calls, backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'ws-coding-agent-foreign-connector', $1, 'none', 'instr') \
         RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) \
         VALUES ($1, $2, true)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert agent connector access");

    let token = common::mint_gateway_token(&server.db, agent_id).await;

    // A REAL, live flow — belonging to a different user, dispatched to a
    // different agent. Deliberately not `agent_id`.
    let victim = seed_user(&server, "ws-coding-agent-foreign-victim").await;
    let victim_agent = seed_agent(&server, victim, "ws-coding-agent-foreign-victim-agent").await;
    let (_victim_flow_id, foreign_traceparent) =
        common::open_flow(&server.db, victim, victim_agent).await;

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &foreign_traceparent)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": SAVE_FILE, "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "a coding-agent row must be admitted even when the traceparent names \
         a real, live flow it isn't a participant of"
    );
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "save_file call must not error: {body:?}"
    );

    let identity_header = {
        let logged = backend_headers.lock().unwrap();
        let (_, header) = logged
            .iter()
            .find(|(method, _)| method == "tools/call")
            .expect("stub must have logged the tools/call request");
        header
            .clone()
            .expect("system backend must receive x-nasiko-identity on tools/call")
    };
    let verified = nasiko_mcp_gateway::identity::SignedIdentity::verify(
        &identity_header,
        common::TEST_JWT_SECRET.as_bytes(),
    )
    .expect("the gateway's own signature must verify with the test signing key");
    assert_eq!(verified.agent_id, agent_id);
    assert_eq!(
        verified.user_id, owner,
        "a foreign live flow must never leak in as the caller's identity — \
         the coding-agent row still acts only as its own owner"
    );
    assert_eq!(
        verified.flow_id, None,
        "the coding-agent row is not a participant of the foreign flow, so \
         flow_id must be None, never that flow's own id"
    );

    server.cleanup().await;
}

/// The cross-user event-injection regression a code-quality review of Task
/// 1.5 caught: on the same foreign-live-flow setup as the test above, an
/// `ask`-stance tool call must not publish a `ToolApprovalRequired` event
/// onto the FOREIGN flow's `FlowEventBus` channel. `a2a_dispatch.rs` forwards
/// that channel straight into the flow owner's SSE stream, so before the fix
/// a coding-agent row could name any live flow's trace id and land an
/// attacker-chosen server/tool "needs approval" card in a completely
/// unrelated user's chat — despite never being a participant of that flow.
/// The fix keys the publish on `verified_flow_id` (`None` here, since this
/// agent isn't a participant), never a re-parse of the raw `traceparent`.
#[tokio::test]
#[serial]
async fn coding_agent_row_with_foreign_flow_does_not_leak_an_approval_event() {
    let server = TestServer::start_with(|cfg| {
        cfg.mcp_tool_search_mode = "none".to_string();
    })
    .await;

    let owner = seed_user(&server, "ws-coding-agent-ask-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-coding-agent-ask").await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("stamp coding_agent_integration_id");

    let (backend_url, _backend_calls, _backend_headers) = start_stub_system_backend().await;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, source_kind, name, url, auth_type, instructions) \
         VALUES ('system', 'system', 'ws-coding-agent-ask-connector', $1, 'none', 'instr') \
         RETURNING id",
    )
    .bind(&backend_url)
    .fetch_one(&server.db)
    .await
    .expect("insert system connector");

    sqlx::query(
        "INSERT INTO mcp_connector_grants (connector_id, grant_type, grantee_id) \
         VALUES ($1, 'public', '*')",
    )
    .bind(connector_id)
    .execute(&server.db)
    .await
    .expect("insert public grant");

    for tool in [SAVE_FILE, LIST_FILES] {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, $2)")
            .bind(connector_id)
            .bind(tool)
            .execute(&server.db)
            .await
            .expect("insert synced connector tool");
    }

    // Ask-stance for save_file specifically — this is what makes the call
    // answer TOOL_ASK instead of actually executing.
    sqlx::query(
        "INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled, tool_rules) \
         VALUES ($1, $2, true, $3)",
    )
    .bind(agent_id)
    .bind(connector_id)
    .bind(json!([{"pattern": SAVE_FILE, "stance": "ask"}]))
    .execute(&server.db)
    .await
    .expect("insert agent connector access with ask-stance rule");

    let token = common::mint_gateway_token(&server.db, agent_id).await;

    let victim = seed_user(&server, "ws-coding-agent-ask-victim").await;
    let victim_agent = seed_agent(&server, victim, "ws-coding-agent-ask-victim-agent").await;
    let (victim_flow_id, foreign_traceparent) =
        common::open_flow(&server.db, victim, victim_agent).await;

    // Simulate the victim's live SSE subscription to their own flow — exactly
    // what `a2a_dispatch.rs` sets up for a real chat session.
    let mut victim_rx = server.flow_events.subscribe(&victim_flow_id).await;

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &foreign_traceparent)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": SAVE_FILE, "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "an ask-stance decision is a normal 200 JSON-RPC error, not an HTTP error"
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        json!(nasiko_mcp_gateway::types::codes::TOOL_ASK),
        "must be the normal ask decision: {body:?}"
    );
    // Second regression (participant laundering through HITL context, caught
    // in a follow-up review): a flow-less `tools/call` has no `verified_flow_id`,
    // so `create_tool_approval_id` must refuse to persist anything — the
    // response must not claim a `hitl_request_id` that resolves against the
    // victim's flow.
    assert!(
        body["error"]["data"].get("hitl_request_id").is_none(),
        "no verified flow means no context_id to persist against — must not \
         claim a hitl_request_id that doesn't exist: {body:?}"
    );

    // The regression: nothing must have been published onto the VICTIM's own
    // flow, despite their trace id being the one carried on the wire.
    match victim_rx.try_recv() {
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
        other => panic!(
            "a coding-agent row naming a foreign live flow must publish \
             NOTHING to it, got {other:?}"
        ),
    }

    // And no `hitl_requests` row was ever persisted naming the victim's flow
    // id as its `context_id` — the actual data this whole class of bug is
    // about not leaking into (`oss/hitl/src/notifier.rs`'s resume path is
    // what would otherwise treat that context as a flow to rejoin).
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hitl_requests WHERE agent_id = $1 AND context_id = $2",
    )
    .bind(agent_id)
    .bind(&victim_flow_id)
    .fetch_one(&server.db)
    .await
    .expect("count query");
    assert_eq!(
        leaked, 0,
        "no hitl_requests row for this coding-agent row may ever carry the \
         victim's flow id as its context_id"
    );

    server.cleanup().await;
}

// ─── nasiko_call_tool: the executor for fixed-menu clients ──────────────────
//
// Under the default search mode `tools/list` carries only matched tools plus
// the gateway's meta-tools, never the full manifest. An MCP client that can
// only invoke tools present in that list (Claude Code, Codex, OpenCode) can
// therefore find `save_file` with `nasiko_search_tools` yet has no way to
// call it — `nasiko_call_tool` is that way. These tests drive it through the
// real gateway against the real stub backend and hold it to one standard: the
// result must be exactly what the direct call would have produced.

/// The `tools/call` body a fixed-menu client sends to run `tool` through the
/// executor — `tool`'s own arguments nested under the executor's.
fn via_call_tool(id: u64, tool: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {
            "name": "nasiko_call_tool",
            "arguments": { "name": tool, "arguments": arguments },
        },
    })
}

/// The same call, named directly — the body the executor must be
/// indistinguishable from.
fn direct_call(id: u64, tool: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

#[tokio::test]
#[serial]
async fn nasiko_call_tool_runs_a_found_system_tool_identically_to_the_direct_call() {
    // Default search mode on purpose — this is the fixed-menu scenario itself,
    // not the eager-fan-out rollback path the other tests in this file need.
    let server = TestServer::start().await;

    let owner = seed_user(&server, "ws-call-tool-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-call-tool-agent").await;
    let (backend_url, backend_calls, _) = start_stub_system_backend().await;
    seed_system_connector(
        &server,
        &backend_url,
        "ws-call-tool-connector",
        agent_id,
        json!([]),
    )
    .await;

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent_id).await;
    let mcp = |body: Value| {
        server
            .client
            .post(server.url("/api/mcp"))
            .bearer_auth(&token)
            .header("traceparent", &traceparent)
            .json(&body)
    };

    // The menu a fixed-menu client is confined to: the meta-tools, not save_file.
    let res = mcp(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"nasiko_search_tools") && names.contains(&"nasiko_call_tool"),
        "a search mode must list the search meta-tool and the executor side by side: {names:?}"
    );
    assert!(
        !names.contains(&SAVE_FILE),
        "sanity: a search mode must not list the real tool itself, or the \
         executor would be pointless here: {names:?}"
    );

    // The search result must be readable the way an MCP client reads any
    // `tools/call` result — through `content` for a text-only client, through
    // `structuredContent` for a schema-aware one — and carry NOTHING else: a
    // strict client (Codex) deserializes `CallToolResult` with unknown fields
    // denied, so a bare top-level `tools`/`search_mode` made every direct
    // search call fail there with "unexpected response type".
    let res = mcp(json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {"name": "nasiko_search_tools", "arguments": {"query": "save file", "limit": 5}},
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let search: Value = res.json().await.unwrap();
    let result = &search["result"];
    assert_eq!(
        result["content"][0]["type"], "text",
        "search result must carry an MCP text content block: {search:?}"
    );
    let from_content: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().expect("content text"))
            .expect("content text is the JSON payload");
    assert_eq!(
        from_content["tools"], result["structuredContent"]["tools"],
        "content and structuredContent views must agree: {search:?}"
    );
    assert!(
        result["structuredContent"]["search_mode"].is_string()
            && from_content["search_mode"] == result["structuredContent"]["search_mode"],
        "search_mode must be present in both views: {search:?}"
    );
    assert!(
        result.get("tools").is_none() && result.get("search_mode").is_none(),
        "no non-spec top-level fields — a strict MCP client refuses unknown keys: {search:?}"
    );
    assert_eq!(
        result["isError"],
        Value::Bool(false),
        "a successful search is not an error: {search:?}"
    );

    let arguments = json!({ "path": "notes.md" });
    let res = mcp(direct_call(2, SAVE_FILE, arguments.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let direct: Value = res.json().await.unwrap();
    assert!(
        direct.get("error").is_none(),
        "direct save_file call must not error: {direct:?}"
    );

    let res = mcp(via_call_tool(2, SAVE_FILE, arguments))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let via_executor: Value = res.json().await.unwrap();
    assert_eq!(
        via_executor, direct,
        "the executor's response must be byte-identical to the direct call's"
    );
    let text = via_executor["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("notes.md"),
        "the inner arguments, not the executor's own, must reach the backend: {via_executor:?}"
    );
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &[SAVE_FILE.to_string(), SAVE_FILE.to_string()],
        "the stub must have seen save_file twice — never nasiko_call_tool"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn nasiko_call_tool_is_blocked_by_a_tool_rule_identically_to_the_direct_call() {
    let server = TestServer::start().await;

    let owner = seed_user(&server, "ws-call-tool-block-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-call-tool-block-agent").await;
    let (backend_url, backend_calls, _) = start_stub_system_backend().await;
    seed_system_connector(
        &server,
        &backend_url,
        "ws-call-tool-block-connector",
        agent_id,
        json!([{"pattern": SAVE_FILE, "stance": "block"}]),
    )
    .await;

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, agent_id).await;
    let mcp = |body: Value| {
        server
            .client
            .post(server.url("/api/mcp"))
            .bearer_auth(&token)
            .header("traceparent", &traceparent)
            .json(&body)
    };

    let res = mcp(direct_call(1, SAVE_FILE, json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "a block decision is a JSON-RPC error");
    let direct: Value = res.json().await.unwrap();
    assert_eq!(
        direct["error"]["code"],
        json!(nasiko_mcp_gateway::types::codes::TOOL_BLOCKED),
        "{direct:?}"
    );

    let res = mcp(via_call_tool(1, SAVE_FILE, json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let via_executor: Value = res.json().await.unwrap();
    assert_eq!(
        via_executor, direct,
        "a rule that blocks the inner tool must block the executor identically"
    );
    assert!(
        backend_calls.lock().unwrap().is_empty(),
        "a blocked call must never reach the backend by either route"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn nasiko_call_tool_ask_rule_records_the_inner_tool_not_the_executor() {
    // A live flow this agent participates in: that is what makes the ask
    // decision persist a `hitl_requests` row and publish an approval event at
    // all (`coding_agent_row_with_foreign_flow_does_not_leak_an_approval_event`
    // above covers the flow-less half). Both the record and the event must
    // name the inner tool — the human approves `save_file`, and the resumed
    // retry's own routing resolves to `save_file`, so anything else would
    // leave the approval unmatchable.
    let server = TestServer::start().await;

    let owner = seed_user(&server, "ws-call-tool-ask-owner").await;
    let agent_id = seed_agent(&server, owner, "ws-call-tool-ask-agent").await;
    let (backend_url, backend_calls, _) = start_stub_system_backend().await;
    let connector_id = seed_system_connector(
        &server,
        &backend_url,
        "ws-call-tool-ask-connector",
        agent_id,
        json!([{"pattern": SAVE_FILE, "stance": "ask"}]),
    )
    .await;

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (flow_id, traceparent) = common::open_flow(&server.db, owner, agent_id).await;
    let mut events = server.flow_events.subscribe(&flow_id).await;

    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &traceparent)
        .json(&via_call_tool(1, SAVE_FILE, json!({ "path": "notes.md" })))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        json!(nasiko_mcp_gateway::types::codes::TOOL_ASK),
        "{body:?}"
    );
    let hitl_request_id: Uuid = body["error"]["data"]["hitl_request_id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("a verified flow must persist an approval row: {body:?}"));

    let (tool_name, question): (Option<String>, Value) =
        sqlx::query_as("SELECT tool_name, question FROM hitl_requests WHERE id = $1")
            .bind(hitl_request_id)
            .fetch_one(&server.db)
            .await
            .expect("the approval row must exist");
    assert_eq!(
        tool_name.as_deref(),
        Some(SAVE_FILE),
        "the approval record must carry the inner tool name"
    );
    assert_eq!(question["tool_name"], json!(SAVE_FILE), "{question:?}");
    assert_eq!(
        question["connector_id"],
        json!(connector_id),
        "{question:?}"
    );

    match events.try_recv() {
        Ok(nasiko_flow::FlowEvent::ToolApprovalRequired {
            agent_id: event_agent,
            server: event_server,
            tool,
        }) => {
            assert_eq!(event_agent, agent_id.to_string());
            assert_eq!(
                tool, SAVE_FILE,
                "the approval card must name the inner tool"
            );
            assert_eq!(
                event_server, "ws-call-tool-ask-connector",
                "the approval card must name the inner tool's connector"
            );
        }
        other => panic!("expected a ToolApprovalRequired event for the inner tool, got {other:?}"),
    }
    assert!(
        backend_calls.lock().unwrap().is_empty(),
        "an ask decision must not reach the backend"
    );

    server.cleanup().await;
}
