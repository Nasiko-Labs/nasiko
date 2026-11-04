//! `GET/PUT /api/settings/composio`: who may use it, how the key is stored, and how
//! switching a toolkit off hides it from the catalog without deleting it.
//!
//! Registering a brand-new toolkit calls Composio, so these tests only switch toolkits whose
//! connector row is seeded directly.
//!
//!   cargo test -p nasiko-server --test composio_settings -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> String {
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
    body["user_id"].as_str().unwrap().to_string()
}

async fn put(server: &common::TestServer, admin: &str, body: Value) -> (u16, Value) {
    let res = common::as_superuser(
        server.client.put(server.url("/api/settings/composio")),
        admin,
        "admin",
    )
    .json(&body)
    .send()
    .await
    .unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

async fn get(server: &common::TestServer, admin: &str) -> Value {
    common::as_superuser(
        server.client.get(server.url("/api/settings/composio")),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

fn toolkit<'a>(settings: &'a Value, name: &str) -> &'a Value {
    settings["toolkits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["toolkit"] == name)
        .unwrap_or_else(|| panic!("{name} is not offered"))
}

async fn catalog_names(server: &common::TestServer, user: &str) -> Vec<String> {
    let body: Value = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        user,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    body["data"]["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[serial]
async fn only_a_superuser_can_read_or_change_it() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let read = common::as_member(
        server.client.get(server.url("/api/settings/composio")),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(read.status(), 403);

    let write = common::as_member(
        server.client.put(server.url("/api/settings/composio")),
        &admin,
        "admin",
    )
    .json(&json!({"clear_api_key": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(write.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_known_toolkits_are_offered_with_nothing_switched_on() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let settings = get(&server, &admin).await;
    assert_eq!(settings["api_key_configured"], false);
    assert_eq!(settings["key_source"], "none");
    let slack = toolkit(&settings, "slack");
    assert_eq!(slack["enabled"], false);
    assert!(slack["logo_url"].is_string());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn toolkits_cannot_be_switched_on_without_a_key() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let (status, body) = put(&server, &admin, json!({"toolkits": ["slack"]})).await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "composio_not_configured");

    let (status, body) = put(
        &server,
        &admin,
        json!({"api_key": "ak_test", "toolkits": ["not-a-toolkit"]}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "unknown_toolkit");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_key_is_stored_encrypted_and_never_returned() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let (status, saved) = put(&server, &admin, json!({"api_key": "  ak_secret_value  "})).await;
    assert_eq!(status, 200);
    assert_eq!(saved["api_key_configured"], true);
    assert_eq!(saved["key_source"], "ui");
    assert!(!saved.to_string().contains("ak_secret_value"));
    assert!(!get(&server, &admin).await.to_string().contains("ak_secret"));

    let stored: String =
        sqlx::query_scalar("SELECT composio_api_key_encrypted FROM settings WHERE id = 1")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!stored.contains("ak_secret_value"));

    // Saving without a key keeps the stored one.
    let (_, kept) = put(&server, &admin, json!({"toolkits": []})).await;
    assert_eq!(kept["key_source"], "ui");

    let (status, cleared) = put(&server, &admin, json!({"clear_api_key": true})).await;
    assert_eq!(status, 200);
    assert_eq!(cleared["api_key_configured"], false);
    assert_eq!(cleared["key_source"], "none");

    let (status, body) = put(
        &server,
        &admin,
        json!({"api_key": "ak_x", "clear_api_key": true}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "conflicting_key_update");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn switching_a_toolkit_off_hides_it_and_back_on_restores_it() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let slack: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id) \
         VALUES ('composio', 'slack', 'ac_slack') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, name, auth_config_id) \
         VALUES ('composio', 'gmail', 'ac_gmail')",
    )
    .execute(&server.db)
    .await
    .unwrap();

    let (status, settings) = put(
        &server,
        &admin,
        json!({"api_key": "ak_test", "toolkits": ["gmail"]}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(toolkit(&settings, "gmail")["enabled"], true);
    assert_eq!(toolkit(&settings, "slack")["enabled"], false);
    assert_eq!(catalog_names(&server, &admin).await, vec!["gmail"]);

    // The row survives the hide, so connected accounts and permission rules are kept.
    let kept: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM mcp_connectors WHERE id = $1)")
            .bind(slack)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(kept);

    let (_, settings) = put(&server, &admin, json!({"toolkits": ["gmail", "slack"]})).await;
    assert_eq!(toolkit(&settings, "slack")["enabled"], true);
    let mut names = catalog_names(&server, &admin).await;
    names.sort();
    assert_eq!(names, vec!["gmail", "slack"]);

    server.cleanup().await;
}
