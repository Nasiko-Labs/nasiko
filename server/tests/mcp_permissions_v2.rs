//! Integration tests for v2 per-agent connector permissions
//! (`/api/mcp/agents/{agent_id}/connectors` + `/tools`), keyed by connector id.
//!
//!   cargo test -p nasiko-server --test mcp_permissions_v2 -- --test-threads=1

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
