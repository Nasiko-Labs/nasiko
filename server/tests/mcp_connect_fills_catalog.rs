//! Connecting a connector fills its tool catalog, so the connector list shows its tools without
//! an agent having to load them first.
//!
//!   cargo test -p nasiko-server --test mcp_connect_fills_catalog -- --test-threads=1

mod common;

use std::time::Duration;

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

fn allow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::set_var("MCP_ALLOW_PRIVATE_URLS", "true") };
}
fn disallow_private_urls() {
    // SAFETY: serialized by `#[serial]`.
    unsafe { std::env::remove_var("MCP_ALLOW_PRIVATE_URLS") };
}

/// An MCP backend that answers every call with two tools.
async fn start_stub_mcp_server() -> String {
    async fn respond() -> Json<Value> {
        Json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"tools": [
                {"name": "alpha", "description": "first", "inputSchema": {"type": "object"}},
                {"name": "beta", "description": "second", "inputSchema": {"type": "object"}},
            ]},
        }))
    }
    let app = Router::new().route("/", post(respond));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/")
}

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let body: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = body["user_id"].as_str().unwrap().to_string();
    let uuid = Uuid::parse_str(&id).unwrap();
    (id, uuid)
}

async fn catalog_size(server: &common::TestServer, connector: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM mcp_connector_tools WHERE connector_id = $1")
        .bind(connector)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn connecting_fills_an_empty_tool_catalog() {
    allow_private_urls();
    let url = start_stub_mcp_server().await;
    let server = common::TestServer::start().await;
    let (admin, owner) = init_admin(&server).await;
    let connector: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type) \
         VALUES ('mcp_server', $1, 'stub', $2, 'none') RETURNING id",
    )
    .bind(owner)
    .bind(&url)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(catalog_size(&server, connector).await, 0);

    let res = common::as_superuser(
        server.client.post(server.url("/api/mcp/connect")),
        &admin,
        "admin",
    )
    .json(&json!({"connector_id": connector}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // The sync runs off the request.
    let mut size = 0;
    for _ in 0..50 {
        size = catalog_size(&server, connector).await;
        if size > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(size, 2, "connect should have synced the stub's two tools");

    server.cleanup().await;
    disallow_private_urls();
}
