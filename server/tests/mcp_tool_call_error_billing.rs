//! A tool call that fails is recorded as a failure, so the settler does not charge it (R-143).
//!
//! MCP reports a tool's own failure inside a JSON-RPC success envelope (`result.isError: true`),
//! not as a top-level `error`. Both must be recorded as `finish_reason = "error"`.

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use nasiko_mcp_gateway::types::connector_prefix;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const TOOL: &str = "lookup";

/// What the stub answers `tools/call` with, and every tool name it was called with.
#[derive(Clone)]
struct Stub {
    call_reply: Value,
    calls: Arc<Mutex<Vec<String>>>,
}

async fn start_stub_mcp_backend(call_reply: Value) -> (String, Stub) {
    let stub = Stub {
        call_reply,
        calls: Arc::new(Mutex::new(Vec::new())),
    };

    async fn handle(State(stub): State<Stub>, Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        match body.get("method").and_then(Value::as_str).unwrap_or("") {
            "tools/list" => Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": [{"name": TOOL, "inputSchema": {"type": "object"}}]},
            })),
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or("").to_string();
                stub.calls.lock().unwrap().push(name);
                let mut reply = stub.call_reply.clone();
                reply["id"] = id;
                Json(reply)
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
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}/"), stub)
}

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let v: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = v["user_id"].as_str().unwrap().to_string();
    let uuid = Uuid::parse_str(&id).unwrap();
    (id, uuid)
}

/// Registers the stub as a connector, calls its tool through the gateway as a dispatched agent,
/// and returns the gateway's response body and the recorded `(finish_reason, success)`.
async fn call_tool_through_gateway(call_reply: Value) -> (Value, String, bool) {
    let server = common::TestServer::start().await;
    let (owner_id, owner_uuid) = init_admin(&server).await;
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) VALUES ('tool-agent', $1, 'x:1', 'stopped') RETURNING id",
    )
    .bind(owner_uuid)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // SAFETY: serialized by `#[serial]`, as in mcp_e2e_agent_flow.rs.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
    let (backend_url, stub) = start_stub_mcp_backend(call_reply).await;
    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connectors")),
        &owner_id,
        "admin",
    )
    .json(&json!({"name": "billing-stub", "url": backend_url}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201, "connector registration must succeed");
    let connector: Value = res.json().await.unwrap();
    let connector_id =
        Uuid::parse_str(connector["data"]["connector_id"].as_str().unwrap()).unwrap();
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &owner_id,
        "admin",
    )
    .json(&json!({"rules": [
        {"connector_id": connector_id, "tool_pattern": TOOL, "stance": "allow"},
    ]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "the agent must be allowed the tool");

    let token = common::mint_gateway_token(&server.db, agent_id).await;
    let (_flow_id, traceparent) = common::open_flow(&server.db, owner_uuid, agent_id).await;
    let res = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .header("traceparent", &traceparent)
        .json(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": format!("{}__{TOOL}", connector_prefix(connector_id)), "arguments": {}},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        stub.calls.lock().unwrap().as_slice(),
        &[TOOL.to_string()],
        "the call must reach the backend, or the recorded outcome proves nothing: {body}"
    );

    let (finish_reason, success) = recorded_outcome(&server.db, agent_id).await;
    server.cleanup().await;
    (body, finish_reason, success)
}

/// The usage row is written by a spawned task, so it is polled for.
async fn recorded_outcome(db: &sqlx::PgPool, agent_id: Uuid) -> (String, bool) {
    for _ in 0..100 {
        let row: Option<(String, bool)> = sqlx::query_as(
            "SELECT status, (arguments->>'success')::boolean FROM tool_calls \
             WHERE agent_id = $1 AND tool_kind = 'mcp'",
        )
        .bind(agent_id)
        .fetch_optional(db)
        .await
        .unwrap();
        if let Some(row) = row {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no mcp_tool_call usage row was recorded for agent {agent_id}");
}

#[tokio::test]
#[serial]
async fn a_result_with_is_error_is_recorded_as_a_failure() {
    let (body, finish_reason, success) = call_tool_through_gateway(json!({
        "jsonrpc": "2.0",
        "result": {"isError": true, "content": [{"type": "text", "text": "upstream refused"}]},
    }))
    .await;

    assert_eq!(body["result"]["isError"], true, "{body}");
    assert_eq!(finish_reason, "error");
    assert!(!success);
}

#[tokio::test]
#[serial]
async fn a_top_level_error_is_recorded_as_a_failure() {
    let (body, finish_reason, success) = call_tool_through_gateway(json!({
        "jsonrpc": "2.0",
        "error": {"code": -32000, "message": "upstream exploded"},
    }))
    .await;

    assert!(body.get("error").is_some(), "{body}");
    assert_eq!(finish_reason, "error");
    assert!(!success);
}

#[tokio::test]
#[serial]
async fn a_successful_result_is_recorded_as_ok() {
    let (body, finish_reason, success) = call_tool_through_gateway(json!({
        "jsonrpc": "2.0",
        "result": {"isError": false, "content": [{"type": "text", "text": "found it"}]},
    }))
    .await;

    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(finish_reason, "ok");
    assert!(success);
}
