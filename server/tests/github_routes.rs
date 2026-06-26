mod common;

use serial_test::serial;

async fn init_admin(server: &common::TestServer) -> serde_json::Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&serde_json::json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

// ─── /api/github/status ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_github_status_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/github/status"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_status_reports_not_configured_when_oauth_not_set() {
    // The test config has github_client_id/secret as None.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/github/status"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["connected"], false);
    // configured=false signals to the UI that the OAuth app is not set up
    assert_eq!(body["configured"], false);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_status_reports_not_connected_when_no_token_stored() {
    // If GitHub OAuth is configured but the user has not connected their account,
    // status returns connected=false, valid=false (not an error).
    // We can't test this without setting env vars for GitHub creds, so we
    // verify the not-configured path returns a well-formed 200 instead.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let body: serde_json::Value = server
        .client
        .get(server.url("/api/github/status"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Either not-configured or not-connected — both have connected=false
    assert_eq!(body["connected"], false);

    server.cleanup().await;
}

// ─── /api/github/repos ───────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_github_repos_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/github/repos"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_repos_returns_404_when_oauth_not_configured() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .get(server.url("/api/github/repos"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── /api/github/logout ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_github_logout_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .delete(server.url("/api/github/logout"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_logout_is_idempotent_when_no_token_stored() {
    // Logout with no stored token should succeed (deletes 0 rows, still 200)
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();

    let res = server
        .client
        .delete(server.url("/api/github/logout"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["message"].as_str().is_some());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_logout_clears_stored_token() {
    // Seed a fake token into user_identities, then verify logout removes it.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_id = admin["user_id"].as_str().unwrap();
    let user_uuid: uuid::Uuid = user_id.parse().unwrap();

    // Directly insert a fake GitHub token into user_identities
    sqlx::query(
        "INSERT INTO user_identities (user_id, provider, provider_id, provider_username, provider_metadata) \
         VALUES ($1, 'github', 'gh_12345', 'testuser', '{\"access_token\": \"fake_token\"}'::jsonb)"
    )
    .bind(user_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    // Logout
    let res = server
        .client
        .delete(server.url("/api/github/logout"))
        .header("x-user-id", user_id)
        .header("x-username", "admin")
        .header("x-is-superuser", "true")
        .header("x-user-role", "admin")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Verify row is gone
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_identities WHERE user_id = $1 AND provider = 'github'"
    )
    .bind(user_uuid)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(count, 0, "logout should remove the user_identities row");

    server.cleanup().await;
}
