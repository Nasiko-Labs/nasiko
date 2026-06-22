mod common;

use serial_test::serial;
use serde_json::{Value, json};

// ─── helpers ────────────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
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

async fn login(server: &common::TestServer, access_key: &str, access_secret: &str) -> Value {
    server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": access_key, "access_secret": access_secret}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

// ─── tests ──────────────────────────────────────────────────────────────────

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
async fn test_initialize_admin_returns_credentials() {
    let server = common::TestServer::start().await;

    let body = init_admin(&server).await;

    assert!(body["access_key"].as_str().unwrap().starts_with("NASK_"));
    assert!(!body["access_secret"].as_str().unwrap().is_empty());
    assert!(!body["token"].as_str().unwrap().is_empty());
    assert_eq!(body["username"], "admin");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_initialize_admin_rejects_second_call() {
    let server = common::TestServer::start().await;

    init_admin(&server).await;

    let res = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin2", "email": "admin2@test.local"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 409);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_valid_credentials() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let key = admin["access_key"].as_str().unwrap();
    let secret = admin["access_secret"].as_str().unwrap();

    let body = login(&server, key, secret).await;

    assert!(!body["token"].as_str().unwrap().is_empty());
    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_superuser"], true);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_wrong_secret_is_rejected() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let key = admin["access_key"].as_str().unwrap();

    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": key, "access_secret": "wrong-secret"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_protected_route_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/users"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_protected_route_accessible_with_token() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let token = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await;
    let jwt = token["token"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/users"))
        .bearer_auth(jwt)
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["total"], 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_admin_can_create_user_with_credentials() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let jwt = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let new_user = server
        .client
        .post(server.url("/api/users"))
        .bearer_auth(&jwt)
        .json(&json!({"username": "alice", "email": "alice@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert!(new_user["access_key"].as_str().unwrap().starts_with("NASK_"));
    assert!(!new_user["access_secret"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_new_user_can_login() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let jwt = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let new_user = server
        .client
        .post(server.url("/api/users"))
        .bearer_auth(&jwt)
        .json(&json!({"username": "alice", "email": "alice@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    let alice_login = login(
        &server,
        new_user["access_key"].as_str().unwrap(),
        new_user["access_secret"].as_str().unwrap(),
    )
    .await;

    assert_eq!(alice_login["username"], "alice");
    assert!(!alice_login["token"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_regenerate_credentials_invalidates_old_ones() {
    let server = common::TestServer::start().await;

    let admin = init_admin(&server).await;
    let old_key = admin["access_key"].as_str().unwrap().to_owned();
    let old_secret = admin["access_secret"].as_str().unwrap().to_owned();
    let admin_id = admin["user_id"].as_str().unwrap().to_owned();

    let jwt = login(&server, &old_key, &old_secret).await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Regenerate credentials
    let new_creds = server
        .client
        .post(server.url(&format!("/api/users/{admin_id}/regenerate-credentials")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    let new_key = new_creds["access_key"].as_str().unwrap();
    let new_secret = new_creds["access_secret"].as_str().unwrap();

    // Old credentials should no longer work
    let old_login = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": old_key, "access_secret": old_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(old_login.status(), 401);

    // New credentials should work
    let new_login = login(&server, new_key, new_secret).await;
    assert!(!new_login["token"].as_str().unwrap().is_empty());

    server.cleanup().await;
}