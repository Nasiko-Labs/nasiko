//! End-to-end wiring test for the "agent calls the MCP gateway" path.
//!
//! ## What this proves vs. what already exists
//!
//! `mcp_delegation_auth.rs` proves the `require_delegation` auth gate in
//! isolation (hand-minted token, `initialize` only). `mcp_connectors.rs` /
//! `mcp_permissions_v2.rs` prove connector registration and permission CRUD.
//! Nobody yet drives the FULL path in one test: register a real backend →
//! grant an agent permission on some of its tools but not others → mint a
//! delegation token exactly the way production code mints it → `tools/list`
//! reflects the permission choices → `tools/call` on the allowed tool reaches
//! the backend and returns its result → `tools/call` on the blocked tool is
//! rejected *without the backend ever seeing it* (enforcement before the
//! proxy hop, not after).
//!
//! ## Architecture note (read this before extending this file)
//!
//! There are two distinct producers of the `x-nasiko-agent-token` delegation
//! header in this codebase:
//!
//!   1. `oss/server/src/agent_proxy.rs` mints one when the platform forwards a
//!      request to an agent CONTAINER (`POST /api/agents/{id}/*`).
//!   2. `oss/react-agent/src/tool.rs`'s `A2aTool` mints one when the
//!      orchestrator's ReAct loop calls another AGENT over A2A.
//!
//! In BOTH cases the token lands on an agent container, which is expected to
//! itself act as an MCP client and call `POST /api/mcp` directly: the agent
//! reads the *inbound* `X-Nasiko-Agent-Token` header and forwards it here.
//!
//! `oss/react-agent/src/react_loop.rs` builds its tool set exclusively from
//! `A2aTool` (`Orchestrator::build_tools` / `run_stream_inner`, both call
//! `A2aTool::new(agent.clone(), ...)` for every entry in the agent registry —
//! grep the file for any construction of an MCP-namespaced tool and there is
//! none). There is no code path where the LLM inside the ReAct loop decides to
//! call an MCP tool and the orchestrator process itself issues the
//! `tools/call` HTTP request — `toolset.call(name, ...)` only ever dispatches
//! to `call_agent_*` tools. So: **MCP tool-calling is not wired into the
//! orchestrator's ReAct loop.** The only real, wired path today is exactly
//! what this file exercises — an agent container (or anything else holding a
//! valid delegation token) hitting `POST /api/mcp` directly over HTTP. That is
//! the intended architecture (the Python helper module exists for precisely
//! this), not a stand-in for a missing LLM auto-dispatch feature.
//!
//!   cargo test -p nasiko-server --test mcp_e2e_agent_flow -- --test-threads=1

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use nasiko_auth::jwt::mint_delegation_token;
use nasiko_mcp_gateway::types::connector_prefix;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── helpers (local to this file, mirroring mcp_connect.rs / mcp_permissions_v2.rs) ──

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

/// Names of the two tools the stub backend advertises.
const ALLOWED_TOOL: &str = "echo";
const BLOCKED_TOOL: &str = "delete_all";

/// Tracks every `tools/call` the stub backend actually received, by tool name —
/// this is the "proof the proxy hop never happened" counter for the blocked tool.
type CallLog = Arc<Mutex<Vec<String>>>;

/// A tiny real MCP JSON-RPC backend: answers `tools/list` with two tools and
/// `tools/call` by echoing the arguments back, recording every call it gets.
/// Mirrors `mcp_connectors.rs`'s `start_stub_mcp_server` pattern, extended to
/// speak actual `tools/list`/`tools/call` JSON-RPC instead of just probing
/// auth headers.
async fn start_stub_mcp_backend() -> (String, CallLog) {
    let calls: CallLog = Arc::new(Mutex::new(Vec::new()));

    async fn handle(State(calls): State<CallLog>, Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");

        match method {
            "tools/list" => Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": ALLOWED_TOOL,
                            "description": "echoes its arguments back",
                            "inputSchema": {"type": "object"},
                        },
                        {
                            "name": BLOCKED_TOOL,
                            "description": "a destructive tool this agent must never reach",
                            "inputSchema": {"type": "object"},
                        },
                    ]
                },
            })),
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or("").to_string();
                let arguments = body["params"]["arguments"].clone();
                calls.lock().unwrap().push(name.clone());
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": format!("stub backend executed '{name}'")}],
                        "echoed_arguments": arguments,
                    },
                }))
            }
            other => Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("stub: method not found: {other}")},
            })),
        }
    }

    let app = Router::new()
        .route("/", post(handle))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{port}/"), calls)
}

#[tokio::test]
#[serial]
async fn agent_calls_mcp_gateway_end_to_end_with_permission_enforcement() {
    let server = common::TestServer::start().await;
    let (owner_id, owner_uuid) = init_admin(&server).await;
    let agent_id = seed_agent(&server, owner_uuid, "e2e-mcp-agent").await;

    // ── Register the stub as a real connector, exactly the way an operator would ──
    allow_private_urls();
    let (backend_url, backend_calls) = start_stub_mcp_backend().await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        &owner_id,
        "admin",
    )
    .json(&json!({"name": "e2e-stub", "url": backend_url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201, "connector registration must succeed");
    let connector: Value = res.json().await.unwrap();
    let connector_id = Uuid::parse_str(connector["connector_id"].as_str().unwrap()).unwrap();
    disallow_private_urls();

    // ── Grant the agent an explicit permission: block one tool, leave the other on default-allow ──
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &owner_id,
        "admin",
    )
    .json(&json!({"rules": [
        {"connector_id": connector_id, "tool_pattern": BLOCKED_TOOL, "stance": "block"},
    ]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "permission rule must be accepted");

    // ── Mint the delegation token exactly the way production code does ──
    // (same function, same argument order as `agent_proxy.rs`'s
    // `mint_delegation_token(&jwt_secret, &claims.sub, &agent_id_str)` and
    // `tool.rs`'s `mint_delegation_token(&d.jwt_secret, &d.user_id, &self.agent.id)`).
    let token = mint_delegation_token(common::TEST_JWT_SECRET, &owner_id, &agent_id.to_string())
        .expect("mint delegation token");

    let mcp = |body: Value| {
        server
            .client
            .post(server.url("/api/mcp"))
            .header("x-nasiko-agent-token", &token)
            .json(&body)
    };

    // ── 1. initialize ──
    let res = mcp(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["result"]["serverInfo"]["name"], "MCP Gateway");

    // ── 2. tools/list — allowed tool visible & namespaced, blocked tool absent entirely ──
    let res = mcp(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let tools = body["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    let prefix = connector_prefix(connector_id);
    let allowed_namespaced = format!("{prefix}__{ALLOWED_TOOL}");
    let blocked_namespaced = format!("{prefix}__{BLOCKED_TOOL}");

    assert!(
        names.contains(&allowed_namespaced.as_str()),
        "allowed tool must be in the manifest: {names:?}"
    );
    assert!(
        !names.contains(&blocked_namespaced.as_str()),
        "blocked tool must NOT be in the manifest at all: {names:?}"
    );

    // ── 3. tools/call on the allowed tool — must reach the stub and round-trip its result ──
    let res = mcp(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": allowed_namespaced, "arguments": {"q": "hello"}},
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "allowed tool call must not error: {body:?}"
    );
    assert_eq!(
        body["result"]["echoed_arguments"]["q"], "hello",
        "the gateway's response must actually be the stub backend's response, proving the round trip: {body:?}"
    );
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &[ALLOWED_TOOL.to_string()],
        "the stub backend must have received exactly one call, for the allowed tool"
    );

    // ── 4. tools/call on the blocked tool — rejected before the proxy hop, backend never called ──
    let res = mcp(json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": blocked_namespaced, "arguments": {"q": "should never arrive"}},
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "JSON-RPC errors are still HTTP 200");
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["error"]["code"],
        json!(nasiko_mcp_gateway::types::codes::TOOL_BLOCKED),
        "blocked tool must come back as a JSON-RPC TOOL_BLOCKED error: {body:?}"
    );
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        &[ALLOWED_TOOL.to_string()],
        "the stub backend's call log must be unchanged — enforcement happened before the proxy hop, \
         the backend must never see the blocked call"
    );

    server.cleanup().await;
}
