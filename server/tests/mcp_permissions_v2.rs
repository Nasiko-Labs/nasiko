//! Integration tests for v2 per-agent connector permissions
//! (`/api/mcp/agents/{agent_id}/connectors` + `/tools`), keyed by connector id.
//!
//! Also covers `GET .../connectors/{id}/tools` (per-connector tool listing with
//! stance) and the full permission-permutation matrix for `tools/list` /
//! `tools/call` through the real HTTP + DB + protocol stack (see the
//! `─ matrix ─` section below for the enforcement contract this determined).
//!
//!   cargo test -p nasiko-server --test mcp_permissions_v2 -- --test-threads=1

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use nasiko_auth::jwt::mint_delegation_token;
use nasiko_mcp_gateway::types::{codes, connector_prefix};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let v = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let id = v["user_id"].as_str().unwrap().to_string();
    let uuid = Uuid::parse_str(&id).unwrap();
    (id, uuid)
}

async fn seed_connector(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, 'https://example.com', 'none') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ($1, $2, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]` — same convention as mcp_connectors.rs.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

// ─── stub MCP backend ────────────────────────────────────────────────────────
//
// Mirrors `mcp_e2e_agent_flow.rs`'s `start_stub_mcp_backend` pattern (a real
// JSON-RPC `tools/list`/`tools/call` responder), parameterized here with three
// named tools so the permission matrix below has enough distinct tool names to
// combine with per-tool rules while leaving one name ("OTHER_TOOL") permanently
// rule-free to exercise the "matches nothing" default.

const TOOL_SEND: &str = "SEND_EMAIL";
const TOOL_READ: &str = "READ_EMAIL";
const TOOL_OTHER: &str = "OTHER_TOOL";

type CallLog = Arc<Mutex<Vec<String>>>;

async fn start_stub_backend() -> (String, CallLog) {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));

    async fn handle(State(calls): State<CallLog>, Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
        match method {
            "tools/list" => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"tools": [
                    {"name": TOOL_SEND, "description": "send"},
                    {"name": TOOL_READ, "description": "read"},
                    {"name": TOOL_OTHER, "description": "other"},
                ]},
            })),
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or("").to_string();
                calls.lock().unwrap().push(name.clone());
                Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {"content": [{"type": "text", "text": format!("executed {name}")}]},
                }))
            }
            other => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("stub: method not found: {other}")},
            })),
        }
    }

    let app = Router::new().route("/", post(handle)).with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}/"), calls)
}

/// Fixture shared by every matrix test: one owner, one agent, one connector
/// (backed by a live stub) registered through the real HTTP routes exactly the
/// way an operator would, plus the delegation token an agent container would
/// hold for `(owner, agent)`.
struct Fixture {
    owner_id: String,
    connector_id: Uuid,
    prefix: String,
    token: String,
    calls: CallLog,
}

async fn setup_fixture(server: &common::TestServer) -> Fixture {
    let (owner_id, owner_uuid) = init_admin(server).await;
    let agent_id = seed_agent(server, owner_uuid, "matrix-agent").await;

    allow_private_urls();
    let (backend_url, calls) = start_stub_backend().await;
    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors")), &owner_id, "admin")
        .json(&json!({"name": "matrix-stub", "url": backend_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let connector_id =
        Uuid::parse_str(res.json::<Value>().await.unwrap()["connector_id"].as_str().unwrap()).unwrap();
    disallow_private_urls();

    let token = mint_delegation_token(common::TEST_JWT_SECRET, &owner_id, &agent_id.to_string()).unwrap();
    let prefix = connector_prefix(connector_id);
    Fixture { owner_id, connector_id, prefix, token, calls }
}

impl Fixture {
    fn namespaced(&self, tool: &str) -> String {
        format!("{}__{}", self.prefix, tool)
    }

    async fn mcp(&self, server: &common::TestServer, method: &str, params: Option<Value>) -> Value {
        let mut body = json!({"jsonrpc": "2.0", "id": 1, "method": method});
        if let Some(p) = params {
            body["params"] = p;
        }
        server
            .client
            .post(server.url("/api/mcp"))
            .header("x-nasiko-agent-token", &self.token)
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn tools_list_names(&self, server: &common::TestServer) -> Vec<String> {
        let body = self.mcp(server, "tools/list", None).await;
        body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    async fn call(&self, server: &common::TestServer, tool: &str) -> Value {
        self.mcp(server, "tools/call", Some(json!({"name": self.namespaced(tool), "arguments": {}}))).await
    }

    async fn set_connector_enabled(&self, server: &common::TestServer, enabled: bool) {
        let res = common::as_superuser(
            server.client.put(server.url(&format!(
                "/api/mcp/agents/{}/connectors/{}",
                self.agent_id_from_token(),
                self.connector_id
            ))),
            &self.owner_id,
            "admin",
        )
        .json(&json!({"enabled": enabled}))
        .send()
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
    }

    async fn set_tool_rules(&self, server: &common::TestServer, rules: Vec<(Uuid, &str, &str)>) {
        let rules: Vec<Value> = rules
            .into_iter()
            .map(|(cid, pattern, stance)| json!({"connector_id": cid, "tool_pattern": pattern, "stance": stance}))
            .collect();
        let res = common::as_superuser(
            server.client.put(server.url(&format!("/api/mcp/agents/{}/tools", self.agent_id_from_token()))),
            &self.owner_id,
            "admin",
        )
        .json(&json!({"rules": rules}))
        .send()
        .await
        .unwrap();
        assert_eq!(res.status(), 200, "setting tool rules must succeed");
    }

    /// The delegation token's `act` claim, decoded back out — avoids threading
    /// `agent_id` through every helper call site separately.
    fn agent_id_from_token(&self) -> String {
        let (_user, agent) = nasiko_auth::jwt::validate_delegation_token(common::TEST_JWT_SECRET, &self.token).unwrap();
        agent
    }
}

#[tokio::test]
#[serial]
async fn default_allow_lists_connector_enabled() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "perm-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "perm-agent").await;

    let res = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let entry = body["data"].as_array().unwrap().iter().find(|e| e["connector_id"] == cid.to_string()).unwrap();
    assert_eq!(entry["enabled"], true, "no row → enabled by default");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn disable_connector_persists_and_lists() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "dis-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "dis-agent").await;

    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))),
        &admin_id,
        "admin",
    )
    .json(&json!({"enabled": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.json::<Value>().await.unwrap()["enabled"], false);

    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let entry = body["data"].as_array().unwrap().iter().find(|e| e["connector_id"] == cid.to_string()).unwrap();
    assert_eq!(entry["enabled"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tool_rules_bulk_update_list_and_reset() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "tr-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "tr-agent").await;

    // Bulk update.
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &admin_id,
        "admin",
    )
    .json(&json!({"rules": [
        {"connector_id": cid, "tool_pattern": "SEND_*", "stance": "block"},
        {"connector_id": cid, "tool_pattern": "READ_*", "stance": "ask"},
    ]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // List.
    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let rules = body["data"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().any(|r| r["tool_pattern"] == "SEND_*" && r["stance"] == "block"));

    // Invalid stance → 400.
    let bad = common::as_superuser(
        server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &admin_id,
        "admin",
    )
    .json(&json!({"rules": [{"connector_id": cid, "tool_pattern": "X", "stance": "bogus"}]}))
    .send()
    .await
    .unwrap();
    assert_eq!(bad.status(), 400);

    // Reset.
    let res = common::as_superuser(
        server.client.delete(server.url(&format!("/api/mcp/agents/{agent_id}/permissions"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_agent_connector_access WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 0, "reset must delete all access rows");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn toggle_preserves_existing_tool_rules() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "pre-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "pre-agent").await;

    // Set a tool rule first.
    common::as_superuser(server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))), &admin_id, "admin")
        .json(&json!({"rules": [{"connector_id": cid, "tool_pattern": "SEND_*", "stance": "block"}]}))
        .send()
        .await
        .unwrap();

    // Now toggle the connector off — must not wipe the tool rule.
    common::as_superuser(server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))), &admin_id, "admin")
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();

    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1, "toggling enabled must preserve tool_rules");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn permissions_require_manage_agent() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    // Agent owned by admin; a different member must be forbidden.
    let member = common::as_superuser(server.client.post(server.url("/api/users")), &admin_id, "admin")
        .json(&json!({"username": "pm-member", "email": "pm-member@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let member_id = member["id"].as_str().unwrap();
    let agent_id = seed_agent(&server, admin_uuid, "pm-agent").await;

    let res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        member_id,
        "pm-member",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

// ─── GET /api/mcp/agents/{agent_id}/connectors/{connector_id}/tools ─────────

#[tokio::test]
#[serial]
async fn list_connector_tools_syncs_and_shows_default_allow_stance() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let agent_id = seed_agent(&server, admin_uuid, "lct-agent").await;

    allow_private_urls();
    let (backend_url, _calls) = start_stub_backend().await;
    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors")), &admin_id, "admin")
        .json(&json!({"name": "lct-stub", "url": backend_url}))
        .send()
        .await
        .unwrap();
    let cid = Uuid::parse_str(res.json::<Value>().await.unwrap()["connector_id"].as_str().unwrap()).unwrap();
    disallow_private_urls();

    // No catalog rows yet — the endpoint must sync live from the backend.
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_connector_tools WHERE connector_id = $1")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(empty, 0, "catalog must start empty (nothing synced yet)");

    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3, "must have synced live and returned all 3 stub tools: {data:?}");
    assert!(
        data.iter().all(|t| t["stance"] == "allow"),
        "with no access row and no tool_rules, every tool must default to 'allow': {data:?}"
    );
    assert!(data.iter().any(|t| t["name"] == TOOL_SEND));
    assert!(
        data.iter().all(|t| !t["last_synced_at"].is_null()),
        "a freshly synced tool must carry its last_synced_at timestamp: {data:?}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_connector_tools_empty_catalog_when_backend_has_no_tools() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let agent_id = seed_agent(&server, admin_uuid, "lct-empty-agent").await;

    async fn handle_empty(Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        Json(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": []}}))
    }
    allow_private_urls();
    let app = Router::new().route("/", post(handle_empty));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let backend_url = format!("http://127.0.0.1:{port}/");

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connectors")), &admin_id, "admin")
        .json(&json!({"name": "lct-empty-stub", "url": backend_url}))
        .send()
        .await
        .unwrap();
    let cid = Uuid::parse_str(res.json::<Value>().await.unwrap()["connector_id"].as_str().unwrap()).unwrap();
    disallow_private_urls();

    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0, "an empty backend catalog must list as empty, not error");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_connector_tools_requires_manage_agent() {
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "lct-forbidden-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "lct-forbidden-agent").await;
    let member = common::as_superuser(server.client.post(server.url("/api/users")), &admin_id, "admin")
        .json(&json!({"username": "lct-member", "email": "lct-member@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let member_id = member["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}/tools"))),
        member_id,
        "lct-member",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403, "a caller who cannot manage the agent must be forbidden");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_connector_tools_default_allow_with_no_access_row_at_all() {
    // A connector the agent has NEVER had any mcp_agent_connector_access row
    // for (no enable/disable toggle, no tool_rules ever written) — default-allow
    // semantics must apply cleanly, not error.
    let server = common::TestServer::start().await;
    let (admin_id, admin_uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, admin_uuid, "lct-no-row-tool").await;
    let agent_id = seed_agent(&server, admin_uuid, "lct-no-row-agent").await;
    // Seed a synced catalog row directly (skip the live sync round-trip; this
    // test is about the access-row-absent path, not the sync path).
    sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name) VALUES ($1, 'PING')")
        .bind(cid)
        .execute(&server.db)
        .await
        .unwrap();

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_agent_connector_access WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(rows, 0, "precondition: no access row exists for this agent at all");

    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["name"], "PING");
    assert_eq!(data[0]["stance"], "allow", "no access row at all → default-allow, reflected correctly");

    server.cleanup().await;
}

// ─── tools/list & tools/call permission matrix (Part B) ─────────────────────
//
// Enforcement contract determined by reading `permissions.rs::get_stance` +
// `protocol.rs::handle_tools_call`/`handle_tools_list` end-to-end (see the final
// report for the full writeup); summarized per test below. Every test in this
// section drives the real HTTP routes: `PUT .../connectors/{id}` and
// `PUT .../tools` to set state, `POST /api/mcp` with a real delegation token to
// read `tools/list` / exercise `tools/call`, and a live stub MCP backend so a
// "call actually reached the backend" claim is provable via `Fixture.calls`.

#[tokio::test]
#[serial]
async fn matrix_absent_access_row_is_default_allow() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.contains(&fx.namespaced(TOOL_OTHER)), "{names:?}");
    assert!(names.contains(&fx.namespaced(TOOL_SEND)), "{names:?}");

    let res = fx.call(&server, TOOL_OTHER).await;
    assert!(res.get("error").is_none(), "{res:?}");
    assert_eq!(fx.calls.lock().unwrap().as_slice(), &[TOOL_OTHER.to_string()]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_explicit_enabled_true_row_behaves_like_absent() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_connector_enabled(&server, true).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.contains(&fx.namespaced(TOOL_OTHER)), "{names:?}");

    let res = fx.call(&server, TOOL_OTHER).await;
    assert!(res.get("error").is_none(), "{res:?}");

    server.cleanup().await;
}

/// Regression guard for finding #10 (authorization bypass): disabling a
/// connector for an agent (`enabled=false`) must be enforced at BOTH surfaces.
/// `tools/list` hid it before; `tools/call` did not (its `ServerType::Mcp` branch
/// only checked `can_access_connector` + tool stance, never the enable flag), so
/// a caller who derived the namespaced tool name could still reach the backend.
/// Both surfaces now go through `PermissionContext::decide`, so a disabled
/// connector is denied at call time and the backend is never invoked.
#[tokio::test]
#[serial]
async fn matrix_disabled_connector_is_denied_at_both_list_and_call() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_connector_enabled(&server, false).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.is_empty(), "a disabled connector's tools must be hidden entirely from tools/list: {names:?}");

    let res = fx.call(&server, TOOL_OTHER).await;
    assert_eq!(
        res["error"]["code"],
        json!(codes::TOOL_BLOCKED),
        "tools/call on a disabled connector must be denied, not executed: {res:?}"
    );
    assert!(
        fx.calls.lock().unwrap().is_empty(),
        "the real backend must NEVER be invoked for a connector disabled for this agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_exact_allow_rule() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, TOOL_SEND, "allow")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.contains(&fx.namespaced(TOOL_SEND)), "{names:?}");

    let res = fx.call(&server, TOOL_SEND).await;
    assert!(res.get("error").is_none(), "{res:?}");
    assert_eq!(fx.calls.lock().unwrap().as_slice(), &[TOOL_SEND.to_string()]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_exact_block_rule() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, TOOL_SEND, "block")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(!names.contains(&fx.namespaced(TOOL_SEND)), "a blocked tool must be OMITTED from tools/list: {names:?}");
    assert!(names.contains(&fx.namespaced(TOOL_READ)), "an unrelated tool must remain listed: {names:?}");

    let res = fx.call(&server, TOOL_SEND).await;
    assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res:?}");
    assert!(fx.calls.lock().unwrap().is_empty(), "the backend must never see a blocked call");

    server.cleanup().await;
}

/// Determines the actual `Ask` contract: unlike `Block`, `aggregator.rs` does
/// NOT filter `Ask` out of `tools/list` (it only special-cases
/// `Stance::Block`) — an "ask" tool is listed exactly like an allowed one, with
/// no visible marker distinguishing it. `tools/call`, however, rejects it with
/// a distinct JSON-RPC error code (`TOOL_ASK`, carrying `data.server` for the
/// caller's approval-flow UI) — it is never silently allowed through, and there
/// is no "auto-approve"/partial-execute path for a bare `tools/call` on an
/// `Ask`-stance tool (the flow event goes out via the server's route layer,
/// `oss/server/src/mcp/handlers/gateway.rs`, not this crate).
#[tokio::test]
#[serial]
async fn matrix_exact_ask_rule() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, TOOL_SEND, "ask")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(
        names.contains(&fx.namespaced(TOOL_SEND)),
        "an 'ask' tool IS still listed by tools/list (only Block is filtered at list time): {names:?}"
    );

    let res = fx.call(&server, TOOL_SEND).await;
    assert_eq!(res["error"]["code"], json!(codes::TOOL_ASK), "{res:?}");
    assert!(res["error"]["data"]["server"].is_string(), "{res:?}");
    assert!(fx.calls.lock().unwrap().is_empty(), "an ask-stance tool must never reach the backend via a bare call");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_wildcard_allow_rule() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, "SEND_*", "allow")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.contains(&fx.namespaced(TOOL_SEND)), "{names:?}");

    let res = fx.call(&server, TOOL_SEND).await;
    assert!(res.get("error").is_none(), "{res:?}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_wildcard_block_rule() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, "SEND_*", "block")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(!names.contains(&fx.namespaced(TOOL_SEND)), "{names:?}");

    let res = fx.call(&server, TOOL_SEND).await;
    assert_eq!(res["error"]["code"], json!(codes::TOOL_BLOCKED), "{res:?}");
    assert!(fx.calls.lock().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_overlapping_allow_and_block_block_wins() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;
    fx.set_tool_rules(&server, vec![(fx.connector_id, "*", "allow"), (fx.connector_id, TOOL_SEND, "block")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(!names.contains(&fx.namespaced(TOOL_SEND)), "block must win over the wildcard allow: {names:?}");
    assert!(names.contains(&fx.namespaced(TOOL_READ)), "READ_EMAIL only matches the wildcard allow: {names:?}");

    let blocked = fx.call(&server, TOOL_SEND).await;
    assert_eq!(blocked["error"]["code"], json!(codes::TOOL_BLOCKED), "{blocked:?}");

    let allowed = fx.call(&server, TOOL_READ).await;
    assert!(allowed.get("error").is_none(), "{allowed:?}");
    assert_eq!(fx.calls.lock().unwrap().as_slice(), &[TOOL_READ.to_string()]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn matrix_rule_scoped_to_different_connector_does_not_leak() {
    let server = common::TestServer::start().await;
    let fx = setup_fixture(&server).await;

    // A second, unrelated connector owned by the same user, with a blanket
    // block rule — must have zero effect on `fx.connector_id`'s resolution.
    let other_cid = seed_connector(&server, Uuid::parse_str(&fx.owner_id).unwrap(), "matrix-other-connector").await;
    fx.set_tool_rules(&server, vec![(other_cid, "*", "block")]).await;

    let names = fx.tools_list_names(&server).await;
    assert!(names.contains(&fx.namespaced(TOOL_SEND)), "a rule on a different connector must not leak: {names:?}");

    let res = fx.call(&server, TOOL_SEND).await;
    assert!(res.get("error").is_none(), "{res:?}");
    assert_eq!(fx.calls.lock().unwrap().as_slice(), &[TOOL_SEND.to_string()]);

    server.cleanup().await;
}

// ─── Agent-scoped (not caller-scoped) permissions ───────────────────────────
//
// `mcp_agent_connector_access` used to be keyed `(user_id, agent_id,
// connector_id)`: two different people who can manage the same agent (e.g.
// its owner and a superuser) got independent Allow/Block state. If the owner
// blocked a tool, a superuser managing the same agent was unaffected. These
// tests prove the fix: exactly one row per `(agent_id, connector_id)`, shared
// by every caller who manages the agent.

#[tokio::test]
#[serial]
async fn tool_block_set_by_one_manager_is_seen_by_a_different_manager() {
    let server = common::TestServer::start().await;
    let (admin_id, _) = init_admin(&server).await;
    let owner = common::as_superuser(server.client.post(server.url("/api/users")), &admin_id, "admin")
        .json(&json!({"username": "shared-owner", "email": "shared-owner@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let owner_id = owner["id"].as_str().unwrap();
    let owner_uuid = Uuid::parse_str(owner_id).unwrap();

    // Agent owned by the member — both the owner AND the admin (superuser
    // bypass in `can_manage_agent`) can manage it; two distinct callers.
    let agent_id = seed_agent(&server, owner_uuid, "shared-agent").await;

    allow_private_urls();
    let (backend_url, _calls) = start_stub_backend().await;
    let res = common::as_member(server.client.post(server.url("/api/mcp/connectors")), owner_id, "shared-owner")
        .json(&json!({"name": "shared-tool", "url": backend_url}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let cid = Uuid::parse_str(res.json::<Value>().await.unwrap()["connector_id"].as_str().unwrap()).unwrap();
    disallow_private_urls();

    // Share publicly so the admin — a different manager who doesn't own this
    // connector — also passes the Layer-1 reachability check.
    let res = common::as_member(
        server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))),
        owner_id,
        "shared-owner",
    )
    .json(&json!({"public": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // Caller #1 (the owner) blocks a tool.
    let res = common::as_member(
        server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        owner_id,
        "shared-owner",
    )
    .json(&json!({"rules": [{"connector_id": cid, "tool_pattern": "SEND_*", "stance": "block"}]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Caller #2 (a different manager — the admin) must see the SAME
    // restriction, not a fresh default-allow row scoped to their own identity.
    let body: Value = common::as_superuser(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}/tools"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let tools = body["data"].as_array().unwrap();
    let send_tool = tools.iter().find(|t| t["name"] == TOOL_SEND).expect("stub tool list includes SEND_EMAIL");
    assert_eq!(send_tool["stance"], "block", "a different manager must see the same shared stance: {body:?}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connector_disabled_by_one_manager_is_seen_by_a_different_manager() {
    let server = common::TestServer::start().await;
    let (admin_id, _) = init_admin(&server).await;
    let owner = common::as_superuser(server.client.post(server.url("/api/users")), &admin_id, "admin")
        .json(&json!({"username": "shared-owner2", "email": "shared-owner2@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let owner_id = owner["id"].as_str().unwrap();
    let owner_uuid = Uuid::parse_str(owner_id).unwrap();

    let cid = seed_connector(&server, owner_uuid, "shared-tool2").await;
    let agent_id = seed_agent(&server, owner_uuid, "shared-agent2").await;

    // Share publicly so the admin — a different manager who doesn't own this
    // connector — also passes the Layer-1 reachability check.
    let res = common::as_member(
        server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))),
        owner_id,
        "shared-owner2",
    )
    .json(&json!({"public": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // Caller #1 (admin) disables the connector for the agent.
    let res = common::as_superuser(
        server.client.put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))),
        &admin_id,
        "admin",
    )
    .json(&json!({"enabled": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Caller #2 (the owner) must see it disabled too — not their own,
    // independent default-enabled row.
    let body: Value = common::as_member(
        server.client.get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        owner_id,
        "shared-owner2",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let entry = body["data"].as_array().unwrap().iter().find(|c| c["connector_id"] == json!(cid)).unwrap();
    assert_eq!(entry["enabled"], false, "a different manager must see the shared disabled state: {body:?}");

    server.cleanup().await;
}
