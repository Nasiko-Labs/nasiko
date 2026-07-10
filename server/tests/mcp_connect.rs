//! HTTP-level tests for the unified connect / disconnect flow (custom connectors;
//! Composio paths need a live provider and are covered by crate mockito tests).
//!
//!   cargo test -p nasiko-server --test mcp_connect -- --test-threads=1

mod common;

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
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

async fn seed_connector(server: &common::TestServer, owner: Uuid, name: &str, auth_type: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, 'https://example.com', $3) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(auth_type)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn connect_none_auth_is_immediately_connected() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "none-tool", "none").await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({"connector_id": cid}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.json::<Value>().await.unwrap()["status"], "connected");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_bearer_requires_and_stores_credential() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "bearer-tool", "bearer").await;

    // Missing credential → 400.
    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({"connector_id": cid}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // With credential → 200 + a connection row is stored (encrypted).
    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({"connector_id": cid, "credentials": {"value": "sk-secret"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let (status, enc): (String, Option<String>) =
        sqlx::query_as("SELECT status, encrypted_credential FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2")
            .bind(uuid)
            .bind(cid)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(status, "ACTIVE");
    assert!(enc.is_some(), "credential must be stored");
    assert_ne!(enc.unwrap(), "sk-secret", "credential must be encrypted at rest");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_and_disconnect_removes_connection() {
    let server = common::TestServer::start().await;
    let (uid, uuid) = init_admin(&server).await;
    let cid = seed_connector(&server, uuid, "dc-tool", "bearer").await;

    common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({"connector_id": cid, "credentials": {"value": "tok"}}))
        .send()
        .await
        .unwrap();

    // list shows it.
    let body: Value = common::as_superuser(server.client.get(server.url("/api/mcp/connections")), &uid, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["data"][0]["connector_id"], cid.to_string());

    // disconnect.
    let res = common::as_superuser(server.client.delete(server.url(&format!("/api/mcp/connections/{cid}"))), &uid, "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_user_connections WHERE connector_id = $1")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_without_target_is_bad_request() {
    let server = common::TestServer::start().await;
    let (uid, _) = init_admin(&server).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn connect_to_unreachable_connector_id_is_not_found() {
    let server = common::TestServer::start().await;
    let (uid, _) = init_admin(&server).await;

    let res = common::as_superuser(server.client.post(server.url("/api/mcp/connect")), &uid, "admin")
        .json(&json!({"connector_id": Uuid::new_v4()}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}
