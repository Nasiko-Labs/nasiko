//! First-time external sign-in (GitHub, OIDC) must never take over an existing
//! account just because the provider's username matches it. The username is
//! chosen by whoever owns the provider account, so a match proves nothing.
mod common;

use serial_test::serial;

async fn seed_user(server: &common::TestServer, username: &str, superuser: bool) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active) \
         VALUES ($1, $2, $3, true) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .bind(superuser)
    .fetch_one(&server.db)
    .await
    .expect("seed user")
}

#[tokio::test]
#[serial]
async fn a_matching_provider_username_does_not_adopt_a_local_superuser() {
    let server = common::TestServer::start().await;
    let local = seed_user(&server, "dummy-root", true).await;

    let login = server
        .state
        .auth
        .upsert_oauth_user("github", "gh-1001", "dummy-root", None)
        .await
        .expect("first GitHub sign-in succeeds");

    assert_ne!(
        login.user_id,
        local.to_string(),
        "the local account was adopted"
    );
    assert!(!login.is_superuser, "the new account inherited superuser");
    assert_eq!(login.username, "dummy-root-2");

    let linked: uuid::Uuid = sqlx::query_scalar(
        "SELECT user_id FROM user_identities WHERE provider = 'github' AND provider_id = 'gh-1001'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_ne!(linked, local, "the identity was bound to the local account");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_returning_identity_signs_in_to_its_own_account() {
    let server = common::TestServer::start().await;
    seed_user(&server, "dummy-dev", false).await;

    let first = server
        .state
        .auth
        .upsert_oauth_user("github", "gh-2002", "dummy-dev", None)
        .await
        .unwrap();
    let again = server
        .state
        .auth
        .upsert_oauth_user("github", "gh-2002", "dummy-dev", None)
        .await
        .unwrap();

    assert_eq!(again.user_id, first.user_id);
    assert_eq!(again.username, first.username);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_verified_email_still_links_to_its_existing_account() {
    let server = common::TestServer::start().await;
    let local = seed_user(&server, "dummy-mail", false).await;

    let login = server
        .state
        .auth
        .upsert_oauth_user(
            "github",
            "gh-3003",
            "someone-else",
            Some("dummy-mail@local.test"),
        )
        .await
        .unwrap();

    assert_eq!(login.user_id, local.to_string());

    server.cleanup().await;
}

/// `would_create_user` is what a cap on new accounts asks before a sign-in:
/// "does this create an account?". It must answer exactly what
/// `upsert_oauth_user` then does: a first sign-in whose username is taken
/// creates an account (`name-2`), so it is new; a known identity, or a verified
/// email that belongs to an account, is not.
#[tokio::test]
#[serial]
async fn the_capacity_check_agrees_with_what_the_sign_in_creates() {
    let server = common::TestServer::start().await;
    let auth = &server.state.auth;
    let users = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&server.db)
            .await
            .unwrap()
    };
    seed_user(&server, "dummy-admin", true).await;

    // A first sign-in whose provider username is taken: a new account.
    let predicted = auth
        .would_create_user("github", "gh-3003", None)
        .await
        .unwrap();
    let before = users().await;
    auth.upsert_oauth_user("github", "gh-3003", "dummy-admin", None)
        .await
        .unwrap();
    assert!(users().await > before, "the sign-in did create an account");
    assert!(
        predicted,
        "a taken username was counted as an existing user"
    );

    // The same identity again: no new account, and the check says so.
    assert!(
        !auth
            .would_create_user("github", "gh-3003", None)
            .await
            .unwrap()
    );

    // A verified email that belongs to an account links to it: not new either.
    seed_user(&server, "dummy-mail", false).await;
    assert!(
        !auth
            .would_create_user("oidc", "sub-4004", Some("dummy-mail@local.test"))
            .await
            .unwrap()
    );

    server.cleanup().await;
}
