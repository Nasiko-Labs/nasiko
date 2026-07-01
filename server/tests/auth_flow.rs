mod common;

use serial_test::serial;
use serde_json::{Value, json};

// ─── helpers ────────────────────────────────────────────────────────────────

async fn login(server: &common::TestServer, username: &str, password: &str) -> Value {
    server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

fn as_superuser(rb: reqwest::RequestBuilder, user_id: &str, username: &str) -> reqwest::RequestBuilder {
    rb.header("x-user-id", user_id)
        .header("x-username", username)
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
}

fn as_member(rb: reqwest::RequestBuilder, user_id: &str, username: &str) -> reqwest::RequestBuilder {
    rb.header("x-user-id", user_id)
        .header("x-username", username)
        .header("x-is-superuser", "false")
        .header("x-user-role", "member")
}

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str, email: &str) -> Value {
    as_superuser(
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

// ─── public routes ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_health() {
    let server = common::TestServer::start().await;

    let res = server.client.get(server.url("/health")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "ok");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_valid_credentials() {
    let server = common::TestServer::start().await;

    let body = login(&server, "admin", "test-password").await;

    assert!(!body["token"].as_str().unwrap().is_empty());
    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_superuser"], true);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_wrong_password_is_rejected() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username": "admin", "password": "wrong-password"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_nonexistent_user_is_rejected() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"username": "nobody", "password": "anything"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── gateway header enforcement ──────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_protected_route_requires_gateway_headers() {
    let server = common::TestServer::start().await;

    let res = server.client.get(server.url("/api/users")).send().await.unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_bearer_jwt_alone_is_rejected() {
    let server = common::TestServer::start().await;

    let token = login(&server, "admin", "test-password").await["token"]
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

#[tokio::test]
#[serial]
async fn test_missing_x_user_id_returns_401() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/me"))
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── /me endpoint ────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_me_returns_correct_identity() {
    let server = common::TestServer::start().await;

    let admin = login(&server, "admin", "test-password").await;
    let user_id = admin["user_id"].as_str().unwrap();

    let body: Value = as_superuser(server.client.get(server.url("/api/me")), user_id, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_superuser"], true);
    assert_eq!(body["user_id"], user_id);

    server.cleanup().await;
}

// ─── token validation ────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_token_validate_accepts_valid_token() {
    let server = common::TestServer::start().await;

    let admin = login(&server, "admin", "test-password").await;
    let admin_id = admin["user_id"].as_str().unwrap();
    let token = admin["token"].as_str().unwrap().to_owned();

    let body: Value = as_superuser(
        server.client.post(server.url("/api/auth/tokens/validate")),
        admin_id,
        "admin",
    )
    .json(&json!({"token": token}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["valid"], true);
    assert_eq!(body["username"], "admin");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_token_validate_rejects_invalid_token() {
    let server = common::TestServer::start().await;

    let admin = login(&server, "admin", "test-password").await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let body: Value = as_superuser(
        server.client.post(server.url("/api/auth/tokens/validate")),
        admin_id,
        "admin",
    )
    .json(&json!({"token": "not.a.valid.jwt"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["valid"], false);

    server.cleanup().await;
}

// ─── user management ─────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_admin_can_create_user() {
    let server = common::TestServer::start().await;

    let admin = login(&server, "admin", "test-password").await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    assert_eq!(alice["username"], "alice");
    assert!(!alice["id"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_member_cannot_create_users() {
    let server = common::TestServer::start().await;

    let admin = login(&server, "admin", "test-password").await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_user(&server, admin_id, "alice", "alice@test.local").await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = as_member(server.client.post(server.url("/api/users")), alice_id, "alice")
        .json(&json!({"username": "bob", "email": "bob@test.local"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 403);

    server.cleanup().await;
}
