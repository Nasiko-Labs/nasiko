//! A connector reaches an agent only when someone enables it for that agent: connecting an
//! account does not enable it on every agent the user owns, and setting a tool rule does not
//! enable it either.
//!
//!   cargo test -p nasiko-server --test mcp_agent_access_defaults -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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

async fn seed_connector(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type) \
         VALUES ('mcp_server', $1, $2, 'https://example.com', 'none') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, is_public) \
         VALUES ($1, $2, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn access_rows(server: &common::TestServer, agent: Uuid) -> Vec<(Uuid, bool)> {
    sqlx::query_as(
        "SELECT connector_id, enabled FROM mcp_agent_connector_access WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_all(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn connecting_does_not_enable_the_connector_on_the_users_agents() {
    let server = common::TestServer::start().await;
    let (admin, owner) = init_admin(&server).await;
    let connector = seed_connector(&server, owner, "news").await;
    let agent = seed_agent(&server, owner, "account-news").await;

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

    assert!(access_rows(&server, agent).await.is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn setting_a_tool_rule_does_not_enable_the_connector() {
    let server = common::TestServer::start().await;
    let (admin, owner) = init_admin(&server).await;
    let connector = seed_connector(&server, owner, "news").await;
    let agent = seed_agent(&server, owner, "account-news").await;

    let res = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent}/tools"))),
        &admin,
        "admin",
    )
    .json(&json!({"rules": [
        {"connector_id": connector, "tool_pattern": "search_*", "stance": "block"}
    ]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    assert_eq!(access_rows(&server, agent).await, vec![(connector, false)]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_tool_rule_keeps_an_enabled_connector_enabled() {
    let server = common::TestServer::start().await;
    let (admin, owner) = init_admin(&server).await;
    let connector = seed_connector(&server, owner, "news").await;
    let agent = seed_agent(&server, owner, "account-news").await;

    let enable = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent}/connectors/{connector}"))),
        &admin,
        "admin",
    )
    .json(&json!({"enabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(enable.status(), 200);

    let rules = common::as_superuser(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent}/tools"))),
        &admin,
        "admin",
    )
    .json(&json!({"rules": [
        {"connector_id": connector, "tool_pattern": "search_*", "stance": "ask"}
    ]}))
    .send()
    .await
    .unwrap();
    assert_eq!(rules.status(), 200);

    assert_eq!(access_rows(&server, agent).await, vec![(connector, true)]);

    server.cleanup().await;
}
