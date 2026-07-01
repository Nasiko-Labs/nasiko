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

/// Simulate the gateway injecting superuser identity headers.
fn as_superuser(rb: reqwest::RequestBuilder, user_id: &str, username: &str) -> reqwest::RequestBuilder {
    rb.header("x-user-id", user_id)
        .header("x-username", username)
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
}

/// Simulate the gateway injecting member identity headers.
fn as_member(rb: reqwest::RequestBuilder, user_id: &str, username: &str) -> reqwest::RequestBuilder {
    rb.header("x-user-id", user_id)
        .header("x-username", username)
        .header("x-is-superuser", "false")
        .header("x-user-role", "member")
}

async fn create_alice(server: &common::TestServer, admin_id: &str) -> Value {
    as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
    .json(&json!({"username": "alice", "email": "alice@test.local"}))
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
    let body = login(&server, admin["access_key"].as_str().unwrap(), admin["access_secret"].as_str().unwrap()).await;

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

    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": admin["access_key"], "access_secret": "wrong-secret"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_login_with_nonexistent_key_is_rejected() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": "NASK_doesnotexist", "access_secret": "anything"}))
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

    let admin = init_admin(&server).await;
    let token = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Valid JWT but no X-User-* headers — server does not validate JWTs directly
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

    // Only partial headers — x-user-id is the required field
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
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let body: Value = as_superuser(server.client.get(server.url("/api/me")), user_id, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["sub"], user_id);
    assert_eq!(body["username"], "admin");
    assert_eq!(body["is_superuser"], true);

    server.cleanup().await;
}

// ─── protected /api/auth/* routes ────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_logout_requires_gateway_headers() {
    let server = common::TestServer::start().await;

    // No headers — logout is now a protected route
    let res = server
        .client
        .post(server.url("/api/auth/logout"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_logout_succeeds_with_gateway_headers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = as_superuser(server.client.post(server.url("/api/auth/logout")), user_id, "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 204);

    server.cleanup().await;
}

// token_validate is a PUBLIC endpoint — callers supply the token in the body.
// No X-User-* gateway headers are required or expected.
#[tokio::test]
#[serial]
async fn test_tokens_validate_works_without_gateway_headers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let token = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // No X-User-* headers required — this is a public endpoint
    let body: Value = server
        .client
        .post(server.url("/api/auth/tokens/validate"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["valid"], true, "valid token should be accepted");
    assert_eq!(body["username"], "admin");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_tokens_validate_rejects_invalid_token() {
    let server = common::TestServer::start().await;
    let _admin = init_admin(&server).await;

    let res = server
        .client
        .post(server.url("/api/auth/tokens/validate"))
        .json(&json!({"token": "not.a.valid.jwt"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["valid"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_tokens_validate_still_works_when_gateway_headers_sent() {
    // Backwards-compat: if someone sends gateway headers anyway, endpoint still works.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let token = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let body: Value = as_superuser(
        server.client.post(server.url("/api/auth/tokens/validate")),
        user_id,
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
async fn test_users_for_search_requires_gateway_headers() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/auth/system/users-for-search"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── user management ─────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_protected_route_accessible_with_gateway_headers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let body: Value = as_superuser(server.client.get(server.url("/api/users")), user_id, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["total"], 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_admin_can_create_user_with_gateway_headers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let new_user = create_alice(&server, admin_id).await;

    assert!(new_user["access_key"].as_str().unwrap().starts_with("NASK_"));
    assert!(!new_user["access_secret"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_member_cannot_create_users() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    // Alice (member) tries to create a user — forbidden
    let res = as_member(server.client.post(server.url("/api/users")), alice_id, "alice")
        .json(&json!({"username": "bob", "email": "bob@test.local"}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_admin_can_get_user_by_id() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    let body: Value = as_superuser(
        server.client.get(server.url(&format!("/api/users/{alice_id}"))),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["username"], "alice");
    assert_eq!(body["email"], "alice@test.local");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_new_user_can_login() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_login = login(
        &server,
        alice["access_key"].as_str().unwrap(),
        alice["access_secret"].as_str().unwrap(),
    )
    .await;

    assert_eq!(alice_login["username"], "alice");
    assert!(!alice_login["token"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_deactivated_user_cannot_login() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    // Admin deactivates alice
    let res = as_superuser(
        server.client.post(server.url(&format!("/api/users/{alice_id}/deactivate"))),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    // Alice can no longer login
    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({
            "access_key": alice["access_key"],
            "access_secret": alice["access_secret"]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_reinstated_user_can_login() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    // Deactivate then reinstate
    as_superuser(
        server.client.post(server.url(&format!("/api/users/{alice_id}/deactivate"))),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();

    let res = as_superuser(
        server.client.post(server.url(&format!("/api/users/{alice_id}/reinstate"))),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    // Alice can login again
    let body = login(
        &server,
        alice["access_key"].as_str().unwrap(),
        alice["access_secret"].as_str().unwrap(),
    )
    .await;
    assert!(!body["token"].as_str().unwrap().is_empty());

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

    let new_creds: Value = as_superuser(
        server.client.post(server.url(&format!("/api/users/{admin_id}/regenerate-credentials"))),
        &admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    let new_key = new_creds["access_key"].as_str().unwrap();
    let new_secret = new_creds["access_secret"].as_str().unwrap();

    // Old credentials no longer work
    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({"access_key": old_key, "access_secret": old_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // New credentials work
    let body = login(&server, new_key, new_secret).await;
    assert!(!body["token"].as_str().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_admin_can_delete_user() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    let res = as_superuser(
        server.client.delete(server.url(&format!("/api/users/{alice_id}"))),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 204);

    // Deleted user cannot login
    let res = server
        .client
        .post(server.url("/api/auth/login"))
        .json(&json!({
            "access_key": alice["access_key"],
            "access_secret": alice["access_secret"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── token recording and revocation ──────────────────────────────────────────

/// After login the JTI is recorded in auth_tokens.
/// After logout the token is marked revoked.
/// token_validate on a revoked token returns "token revoked".
#[tokio::test]
#[serial]
async fn test_login_records_token_and_logout_revokes_it() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    // Login — records token in auth_tokens
    let login_resp = login(
        &server,
        admin["access_key"].as_str().unwrap(),
        admin["access_secret"].as_str().unwrap(),
    )
    .await;
    let token = login_resp["token"].as_str().unwrap().to_owned();
    assert!(!token.is_empty());

    // Token is valid before logout
    let validate_before: Value = server
        .client
        .post(server.url("/api/auth/tokens/validate"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(validate_before["valid"], true, "token should be valid before logout");

    // Logout — sets revoked_at in auth_tokens (server side via gateway header simulation)
    let logout_res = as_superuser(
        server.client.post(server.url("/api/auth/logout")),
        admin_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(logout_res.status(), 204, "logout should succeed");

    // After logout the DB row has revoked_at set.
    // token_validate calls AuthProvider::validate_token which is stateless (no DB check
    // at server level — revocation enforcement happens at the gateway). We verify the
    // DB row directly.
    let revoked_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE revoked_at IS NOT NULL",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(revoked_count >= 1, "at least one token should be revoked in auth_tokens");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_initialize_admin_records_token_in_auth_tokens() {
    let server = common::TestServer::start().await;

    let _admin = init_admin(&server).await;

    // The initialize-admin call should have issued a JWT and recorded its JTI
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_tokens")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "initialize-admin should record exactly one token");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_each_login_adds_a_new_auth_tokens_row() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let key = admin["access_key"].as_str().unwrap();
    let secret = admin["access_secret"].as_str().unwrap();

    // Login twice
    login(&server, key, secret).await;
    login(&server, key, secret).await;

    // initialize-admin + 2 logins = 3 rows
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_tokens")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 3, "each login should add a new auth_tokens row");

    server.cleanup().await;
}

/// Logout with a member account should only revoke that member's tokens,
/// not other users' tokens.
#[tokio::test]
#[serial]
async fn test_logout_revokes_only_the_calling_users_tokens() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let admin_id = admin["user_id"].as_str().unwrap();

    let alice = create_alice(&server, admin_id).await;
    let alice_id = alice["id"].as_str().unwrap();

    // Alice logs in
    login(
        &server,
        alice["access_key"].as_str().unwrap(),
        alice["access_secret"].as_str().unwrap(),
    )
    .await;

    // Alice logs out
    as_member(
        server.client.post(server.url("/api/auth/logout")),
        alice_id,
        "alice",
    )
    .send()
    .await
    .unwrap();

    // Alice's tokens are revoked, admin's are not
    let alice_revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(uuid::Uuid::parse_str(alice_id).unwrap())
    .fetch_one(&server.db)
    .await
    .unwrap();

    let admin_revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auth_tokens WHERE user_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(uuid::Uuid::parse_str(admin_id).unwrap())
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert!(alice_revoked >= 1, "Alice's tokens should be revoked");
    assert_eq!(admin_revoked, 0, "Admin's tokens should be untouched");

    server.cleanup().await;
}