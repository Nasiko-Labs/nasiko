//! HTTP-level tests for per-user credential management on custom connectors.
//!
//!   cargo test -p nasiko-server --test mcp_credentials -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> String {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["user_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn create_user(
    server: &common::TestServer,
    admin_id: &str,
    username: &str,
) -> (String, Uuid) {
    let v = common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": username, "email": format!("{username}@test.local")}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

async fn seed_connector(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    auth_type: &str,
) -> Uuid {
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
async fn register_status_and_delete_credential() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_uuid = Uuid::parse_str(&admin).unwrap();
    let cid = seed_connector(&server, admin_uuid, "cred-tool", "bearer").await;

    // Register.
    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .json(&json!({"value": "sk-abc"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.json::<Value>().await.unwrap()["data"]["connected"], true);

    // Status: connected.
    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/credential/status"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["connected"], true);
    assert_eq!(body["data"]["auth_type"], "bearer");

    // Delete → 200 (envelope), then status: not connected.
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/credential/status"))),
        &admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["connected"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_credential_on_inaccessible_connector_forbidden() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (_alice_id, alice_uuid) = create_user(&server, &admin, "cr-alice").await;
    let (bob_id, _) = create_user(&server, &admin, "cr-bob").await;
    let cid = seed_connector(&server, alice_uuid, "alice-cred-tool", "bearer").await;

    // Bob can't reach alice's private connector.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &bob_id,
        "cr-bob",
    )
    .json(&json!({"value": "x"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn register_credential_on_none_auth_is_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_uuid = Uuid::parse_str(&admin).unwrap();
    let cid = seed_connector(&server, admin_uuid, "noauth-tool", "none").await;

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/credential"))),
        &admin,
        "admin",
    )
    .json(&json!({"value": "x"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        400,
        "credentials only apply to bearer/basic/url_param"
    );

    server.cleanup().await;
}
