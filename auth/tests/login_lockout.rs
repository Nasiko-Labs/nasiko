use nasiko_auth::{AuthError, AuthService, AuthServiceImpl, LoginRefusal, hash_password};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::task::JoinSet;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn concurrent_failed_logins_trigger_lockout_atomically() {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set to run DB-backed auth tests");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("connect to Postgres");

    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let user_id = Uuid::new_v4();
    let username = format!("lockout_race_{}", user_id.simple());
    let email = format!("{username}@nasiko.dev");
    let secret_hash = hash_password("CorrectPassword123!").expect("hash password");

    sqlx::query(
        "INSERT INTO users (id, username, email, is_active, is_superuser, failed_login_attempts)
         VALUES ($1, $2, $3, TRUE, FALSE, 0)",
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .execute(&pool)
    .await
    .expect("insert active user");

    sqlx::query(
        "INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
         VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&username)
    .bind(&secret_hash)
    .execute(&pool)
    .await
    .expect("insert user credentials");

    let auth = Arc::new(AuthServiceImpl::new(
        pool.clone(),
        "test-jwt-secret-key-at-least-32-bytes".to_owned(),
    ));

    // Fire 10 simultaneous login attempts with an incorrect password.
    let mut join_set = JoinSet::new();
    for _ in 0..10 {
        let svc = Arc::clone(&auth);
        let user = username.clone();
        join_set.spawn(async move { svc.authenticate(&user, "WrongPassword!").await });
    }

    let mut wrong_password_count = 0;
    let mut locked_count = 0;

    while let Some(res) = join_set.join_next().await {
        match res.expect("task panicked") {
            Err(AuthError::InvalidCredentials {
                reason: LoginRefusal::WrongPassword,
                ..
            }) => wrong_password_count += 1,
            Err(AuthError::InvalidCredentials {
                reason: LoginRefusal::Locked,
                ..
            }) => locked_count += 1,
            other => panic!("unexpected authenticate outcome: {other:?}"),
        }
    }

    // Verify the database counter stopped at 3 and locked_until was set.
    let (attempts, locked_until): (i32, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT failed_login_attempts, locked_until FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("query user lockout state");

    // Clean up test rows before asserting so failed assertions don't leave test users behind.
    let _ = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await;

    assert_eq!(attempts, 3, "expected failed_login_attempts to stop at 3");
    assert!(locked_until.is_some(), "expected locked_until to be set");
    assert_eq!(wrong_password_count, 2);
    assert_eq!(locked_count, 8);
}
