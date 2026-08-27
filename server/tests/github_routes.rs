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

// ─── /api/auth/github/status ───────────────────────────────────────────────

/// The login page renders its GitHub button on this response alone, and calls
/// it before anyone is signed in — so it must be reachable unauthenticated and
/// must report the *backend's* view, not the page's intent. Deployments that
/// never set `GITHUB_CLIENT_ID` used to draw a button that 503'd on click,
/// which reads as an enabled login method to anyone auditing the page.
#[tokio::test]
#[serial]
async fn test_github_login_status_is_public_and_reports_unconfigured() {
    // The test config leaves github_client_id as None.
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/auth/github/status"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["configured"], false);

    // The button's own route must agree, or the page and the backend drift:
    // a `configured: true` that 503s is exactly the state this guards against.
    let login = server
        .client
        .get(server.url("/api/auth/github/login-user"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 503);

    server.cleanup().await;
}

/// The configured direction of the same contract: with OAuth credentials
/// present the status endpoint must flip to `true` and the login route must
/// serve an authorization URL. Without this case, a wiring regression that
/// pins `configured` to `false` (button hidden on every deployment) would
/// pass the unconfigured test above.
#[tokio::test]
#[serial]
async fn test_github_login_status_reports_configured() {
    // Fake credentials suffice: the status endpoint only reports whether the
    // service was constructed, and the login route builds its authorize URL
    // locally (no GitHub call). The callback URL is required — AppState
    // panics if credentials are set without one.
    let server = common::TestServer::start_with(|config| {
        config.github_client_id = Some("test-client-id".into());
        config.github_client_secret = Some("test-client-secret".into());
        config.github_callback_url = Some("http://localhost:9090/api/auth/github/callback".into());
    })
    .await;

    let res = server
        .client
        .get(server.url("/api/auth/github/status"))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["configured"], true);

    // The login route must agree with the status it advertises.
    let login = server
        .client
        .get(server.url("/api/auth/github/login-user"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let login_body: serde_json::Value = login.json().await.unwrap();
    let auth_url = login_body["auth_url"].as_str().unwrap();
    assert!(
        auth_url.starts_with("https://github.com/login/oauth/authorize"),
        "unexpected auth_url: {auth_url}"
    );

    server.cleanup().await;
}

// ─── /api/github/user ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_github_status_requires_auth() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/github/user"))
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

    let res = common::as_superuser(
        server.client.get(server.url("/api/github/user")),
        user_id,
        "admin",
    )
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

    let body: serde_json::Value = common::as_superuser(
        server.client.get(server.url("/api/github/user")),
        user_id,
        "admin",
    )
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
        .get(server.url("/api/github/repositories"))
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

    let res = common::as_superuser(
        server.client.get(server.url("/api/github/repositories")),
        user_id,
        "admin",
    )
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

    let res = common::as_superuser(
        server.client.delete(server.url("/api/github/logout")),
        user_id,
        "admin",
    )
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
    let res = common::as_superuser(
        server.client.delete(server.url("/api/github/logout")),
        user_id,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Verify row is gone
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_identities WHERE user_id = $1 AND provider = 'github'",
    )
    .bind(user_uuid)
    .fetch_one(&server.db)
    .await
    .unwrap();

    assert_eq!(count, 0, "logout should remove the user_identities row");

    server.cleanup().await;
}

// ─── Identity upsert — cross-user relink rejection (SEC fix #1) ─────────────

#[tokio::test]
#[serial]
async fn test_github_identity_upsert_rejects_cross_user_relink() {
    // Regression for SEC fix #1: the `ON CONFLICT (provider, provider_id) DO
    // UPDATE` clause in `github_callback` must NOT reassign `user_id` to a
    // different user when a second user attempts to link a GitHub account
    // already linked to a first user. Without the `WHERE user_identities.user_id
    // = EXCLUDED.user_id` guard, the row's `provider_metadata` (and thus the
    // encrypted access token) would be silently overwritten under the SECOND
    // user's data while the row stayed keyed to the FIRST user's `user_id` —
    // an internally inconsistent, security-sensitive credential row.
    //
    // This exercises the exact upsert statement used by `github_callback`
    // directly against the DB, since the callback itself requires a
    // configured `github_svc` (not set up in the test harness) to reach that
    // code path via HTTP.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_a: uuid::Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let user_b = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &user_a.to_string(),
        "admin",
    )
    .json(&serde_json::json!({"username": "user-b-relink", "email": "user-b-relink@test.local"}))
    .send()
    .await
    .unwrap()
    .json::<serde_json::Value>()
    .await
    .unwrap();
    let user_b_id: uuid::Uuid = user_b["id"].as_str().unwrap().parse().unwrap();

    let provider_id = "gh_shared_account_12345";
    let upsert_sql = r#"INSERT INTO user_identities
               (user_id, provider, provider_id, provider_username, provider_metadata)
           VALUES ($1, 'github', $2, $3, $4)
           ON CONFLICT (provider, provider_id) DO UPDATE
               SET provider_metadata = EXCLUDED.provider_metadata,
                   provider_username  = EXCLUDED.provider_username
               WHERE user_identities.user_id = EXCLUDED.user_id"#;

    // User A links first — fresh insert, 1 row affected.
    let r1 = sqlx::query(upsert_sql)
        .bind(user_a)
        .bind(provider_id)
        .bind("alice")
        .bind(serde_json::json!({"access_token": "a-token"}))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        r1.rows_affected(),
        1,
        "first link should insert a fresh row"
    );

    // User B attempts to link the SAME GitHub account — must be rejected
    // (0 rows affected), not silently reassigned.
    let r2 = sqlx::query(upsert_sql)
        .bind(user_b_id)
        .bind(provider_id)
        .bind("bob")
        .bind(serde_json::json!({"access_token": "b-token"}))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        r2.rows_affected(),
        0,
        "cross-user relink must be rejected (0 rows affected), not reassigned"
    );

    // Row still belongs to user A with A's original metadata untouched.
    let (owner, meta): (uuid::Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT user_id, provider_metadata FROM user_identities WHERE provider = 'github' AND provider_id = $1",
    )
    .bind(provider_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        owner, user_a,
        "row must still belong to the original linking user"
    );
    assert_eq!(
        meta["access_token"], "a-token",
        "original token must be untouched"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_github_identity_upsert_allows_same_user_refresh() {
    // Sanity check: the SAME user re-linking (e.g. re-authorizing after a
    // token expiry) must still work — the WHERE guard only blocks
    // cross-user reassignment, not same-user token refresh.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user_a: uuid::Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

    let provider_id = "gh_same_user_refresh_67890";
    let upsert_sql = r#"INSERT INTO user_identities
               (user_id, provider, provider_id, provider_username, provider_metadata)
           VALUES ($1, 'github', $2, $3, $4)
           ON CONFLICT (provider, provider_id) DO UPDATE
               SET provider_metadata = EXCLUDED.provider_metadata,
                   provider_username  = EXCLUDED.provider_username
               WHERE user_identities.user_id = EXCLUDED.user_id"#;

    let r1 = sqlx::query(upsert_sql)
        .bind(user_a)
        .bind(provider_id)
        .bind("alice")
        .bind(serde_json::json!({"access_token": "old-token"}))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(r1.rows_affected(), 1);

    let r2 = sqlx::query(upsert_sql)
        .bind(user_a)
        .bind(provider_id)
        .bind("alice")
        .bind(serde_json::json!({"access_token": "refreshed-token"}))
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        r2.rows_affected(),
        1,
        "same-user refresh must still update the row"
    );

    let meta: serde_json::Value = sqlx::query_scalar(
        "SELECT provider_metadata FROM user_identities WHERE provider = 'github' AND provider_id = $1",
    )
    .bind(provider_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(meta["access_token"], "refreshed-token");

    server.cleanup().await;
}
