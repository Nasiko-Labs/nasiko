//! Integration tests for database-backed login lockout.
//!
//! Run with: `cargo test -p nasiko-auth --test lockout -- --ignored --test-threads=1`

use std::sync::Arc;

use nasiko_auth::{AuthService, AuthServiceImpl, AuthError, hash_password_async};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Barrier;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for live tests");
    PgPoolOptions::new()
        .max_connections(32)
        .connect(&url)
        .await
        .expect("connect to the auth database")
}

#[tokio::test]
#[ignore = "requires PostgreSQL with the migrated auth schema"]
async fn concurrent_failed_logins_reach_lockout_threshold() {
    let db = pool().await;
    let user_id = Uuid::new_v4();
    let username = format!("lockout-test-{user_id}");
    let email = format!("{username}@example.invalid");
    let password_hash = hash_password_async("Correct-Horse9!").await.unwrap();

    sqlx::query(
        "INSERT INTO users (id, username, email, is_active, role)
         VALUES ($1, $2, $3, true, 'member'::user_role)",
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .execute(&db)
    .await
    .expect("insert test user");

    sqlx::query(
        "INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
         VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(format!("lockout-key-{user_id}"))
    .bind(password_hash)
    .execute(&db)
    .await
    .expect("insert test credentials");

    let auth = AuthServiceImpl::new(db.clone(), "lockout-test-secret".into());
    let barrier = Arc::new(Barrier::new(20));
    let mut tasks = Vec::new();

    for _ in 0..20 {
        let auth = auth.clone();
        let barrier = barrier.clone();
        let username = username.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            auth.authenticate(&username, "wrong-password").await
        }));
    }

    let mut lockout_errors = 0;
    for task in tasks {
        match task.await.expect("login task must not panic") {
            Err(AuthError::AccountLocked { .. }) => lockout_errors += 1,
            Err(AuthError::InvalidCredentials { .. }) => {}
            other => panic!("unexpected concurrent login result: {other:?}"),
        }
    }

    let (failed_attempts, locked_until): (i32, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as(
            "SELECT failed_login_attempts, locked_until FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_one(&db)
        .await
        .expect("read lockout state");

    assert_eq!(failed_attempts, 3);
    assert!(locked_until.is_some());
    assert!(lockout_errors > 0, "at least one request must observe lockout");

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&db)
        .await
        .expect("delete test user");
}
