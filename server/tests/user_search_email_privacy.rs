//! User search email privacy: `/api/search/users` returns email only for the
//! caller themself, not for other users (prevents enumeration).
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test user_search_email_privacy -- --test-threads=1
mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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

async fn seed_user(server: &common::TestServer, username: &str, email: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (username, email, role) VALUES ($1, $2, 'member') RETURNING id",
    )
    .bind(username)
    .bind(email)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn create_token(server: &common::TestServer, user_id: Uuid) -> String {
    // Get the username from the database to construct the token properly.
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    common::sign_token(&user_id.to_string(), &username, false, "member")
}

async fn search_users(server: &common::TestServer, token: &str, query: &str) -> Value {
    server
        .client
        .get(server.url(&format!("/api/search/users?q={}", query)))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn caller_sees_own_email() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "alice", "alice@example.com").await;
    let token_a = create_token(&server, user_a).await;

    // User A searches for themselves.
    let resp = search_users(&server, &token_a, "alice").await;

    let users = resp["data"].as_array().unwrap();
    assert_eq!(users.len(), 1, "should find alice");

    let alice = &users[0];
    assert_eq!(alice["username"], "alice");
    // User A should see their own email.
    assert_eq!(
        alice["email"].as_str(),
        Some("alice@example.com"),
        "caller should see own email"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn caller_does_not_see_others_email() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "alice", "alice@example.com").await;
    let _ = seed_user(&server, "bob", "bob@example.com").await;
    let token_a = create_token(&server, user_a).await;

    // User A searches for user B.
    let resp = search_users(&server, &token_a, "bob").await;

    let users = resp["data"].as_array().unwrap();
    assert_eq!(users.len(), 1, "should find bob");

    let bob = &users[0];
    assert_eq!(bob["username"], "bob");
    // User A should NOT see Bob's email.
    assert!(
        bob.get("email").is_none() || bob["email"].is_null(),
        "caller should not see other user's email, got: {:?}",
        bob.get("email")
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn search_with_multiple_results_filters_each() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;

    let user_a = seed_user(&server, "alice", "alice@example.com").await;
    let _user_b = seed_user(&server, "alicia", "alicia@example.com").await;
    let _user_c = seed_user(&server, "alexandra", "alexandra@example.com").await;
    let token_a = create_token(&server, user_a).await;

    // User A searches for "al" which matches all three.
    let resp = search_users(&server, &token_a, "al").await;

    let users = resp["data"].as_array().unwrap();
    assert!(users.len() >= 2, "should find multiple matches");

    // Find alice (the caller).
    let alice = users.iter().find(|u| u["username"] == "alice").unwrap();
    assert!(
        alice["email"].as_str().is_some(),
        "caller should see own email"
    );

    // Check that other users have no email.
    for user in users.iter() {
        if user["username"] != "alice" {
            assert!(
                user.get("email").is_none() || user["email"].is_null(),
                "caller should not see other user's email, user: {:?}",
                user["username"]
            );
        }
    }

    server.cleanup().await;
}
