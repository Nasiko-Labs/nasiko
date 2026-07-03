//! Tests the server's gateway header-trust path.
//!
//! The gateway validates JWTs and injects X-User-* identity headers before
//! forwarding requests to the server. The server trusts these headers as the
//! only auth path — there is no JWT fallback.
//!
//! These tests simulate what the gateway injects so the full pipeline can be
//! verified without needing the gateway binary to be running.

mod common;

use serial_test::serial;
use serde_json::{Value, json};

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

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str, email: &str) -> Value {
    server
        .client
        .post(server.url("/api/users"))
        .header("x-user-id", admin_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .json(&json!({"username": username, "email": email}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

// ─── primary path: server trusts gateway-injected headers ───────────────────

#[tokio::test]
#[serial]
async fn test_server_uses_injected_user_id_header() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/me"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
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
async fn test_server_uses_correct_role_from_header() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/me"))
        .header("x-user-id", alice_id)
        .header("x-username", "alice")
        .header("x-is-superuser", "false")
        .header("x-user-role", "member")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["username"], "alice");
    assert_eq!(body["is_superuser"], false);

    server.cleanup().await;
}

// ─── no headers → 401 (no JWT fallback) ─────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_no_gateway_headers_returns_401() {
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
async fn test_bearer_jwt_without_headers_returns_401() {
    let server = common::TestServer::start().await;

    let token = login(&server).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let res = server
        .client
        .get(server.url("/api/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── superuser-only routes respect injected role ────────────────────────────

#[tokio::test]
#[serial]
async fn test_member_cannot_access_user_management() {
    let server = common::TestServer::start().await;
    let admin = login(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/users"))
        .header("x-user-id", alice_id)
        .header("x-username", "alice")
        .header("x-is-superuser", "false")
        .header("x-user-role", "member")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 403);

    let res = server
        .client
        .get(server.url("/api/users"))
        .header("x-user-id", admin_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);

    server.cleanup().await;
}
