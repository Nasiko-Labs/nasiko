//! NAS-1134: the unauthenticated `initialize-admin` bootstrap must stay closed
//! once the database has any administrator. It used to count only
//! `role = 'admin'`, so a database whose administrators were superusers with
//! another role let anyone create a fresh superuser.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test initialize_admin_gate -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::json;
use serial_test::serial;

async fn initialize_admin(server: &common::TestServer, username: &str) -> StatusCode {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": username, "email": format!("{username}@local.test")}))
        .send()
        .await
        .unwrap()
        .status()
}

async fn superuser_count(server: &common::TestServer) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_superuser")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_superuser_without_the_admin_role_closes_the_gate() {
    let server = common::TestServer::start().await;
    sqlx::query(
        "INSERT INTO users (username, email, is_superuser, is_active, role) \
         VALUES ('dummy-root', 'root@local.test', true, true, 'member'::user_role)",
    )
    .execute(&server.db)
    .await
    .unwrap();

    assert_eq!(
        initialize_admin(&server, "dummy-intruder").await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        superuser_count(&server).await,
        1,
        "a second superuser was created"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_soft_deleted_admin_still_closes_the_gate() {
    let server = common::TestServer::start().await;
    sqlx::query(
        "INSERT INTO users (username, email, is_superuser, is_active, role, deleted_at) \
         VALUES ('dummy-gone', 'gone@local.test', false, true, 'admin'::user_role, now())",
    )
    .execute(&server.db)
    .await
    .unwrap();

    assert_eq!(
        initialize_admin(&server, "dummy-intruder").await,
        StatusCode::CONFLICT
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn concurrent_bootstraps_create_exactly_one_admin() {
    let server = common::TestServer::start().await;

    let calls = (0..8).map(|i| {
        let client = server.client.clone();
        let url = server.url("/api/auth/initialize-admin");
        async move {
            client
                .post(url)
                .json(&json!({"username": format!("dummy-admin-{i}"), "email": format!("a{i}@local.test")}))
                .send()
                .await
                .unwrap()
                .status()
        }
    });
    let statuses = futures::future::join_all(calls).await;

    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    let refused = statuses
        .iter()
        .filter(|s| **s == StatusCode::CONFLICT)
        .count();
    assert_eq!((created, refused), (1, 7), "{statuses:?}");
    assert_eq!(superuser_count(&server).await, 1);

    // And it stays closed afterwards.
    assert_eq!(
        initialize_admin(&server, "dummy-late").await,
        StatusCode::CONFLICT
    );

    server.cleanup().await;
}
