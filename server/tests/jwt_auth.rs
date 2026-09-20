//! Tests the server's JWT-based auth path.
//!
//! The server validates Bearer JWTs directly and extracts identity from the
//! token claims. These tests use the common JWT helpers to sign tokens with
//! the known test secret so the full auth pipeline can be verified without
//! needing the gateway binary to be running.

mod common;

use serde_json::{Value, json};
use serial_test::serial;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Bootstrap the admin (creates the user + issues a token) and return the
/// response body. Used by tests that need a valid user_id or token.
async fn login(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_user(
    server: &common::TestServer,
    admin_id: &str,
    username: &str,
    email: &str,
) -> Value {
    common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": username, "email": email}))
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap()
}

// ─── primary path: server validates JWT and uses token identity ──────────────

#[tokio::test]
#[serial]
async fn test_server_uses_jwt_identity() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = common::as_superuser(server.client.get(server.url("/api/me")), user_id, "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["sub"], user_id);
    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_superuser"], true);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_server_uses_correct_role_from_jwt() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(server.client.get(server.url("/api/me")), alice_id, "alice")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["username"], "alice");
    assert_eq!(body["is_superuser"], false);

    server.cleanup().await;
}

// ─── no auth → 401 ──────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_no_auth_returns_401() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/me"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_bearer_jwt_is_accepted() {
    let server = common::TestServer::start().await;

    let token = login(&server).await["token"].as_str().unwrap().to_owned();

    let res = server
        .client
        .get(server.url("/api/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    server.cleanup().await;
}

// ─── superuser-only routes respect JWT role ──────────────────────────────────

#[tokio::test]
#[serial]
async fn test_member_cannot_access_user_management() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = common::as_member(
        server.client.get(server.url("/api/users")),
        alice_id,
        "alice",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 403);

    let res = common::as_superuser(
        server.client.get(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(res.status(), 200);

    server.cleanup().await;
}
