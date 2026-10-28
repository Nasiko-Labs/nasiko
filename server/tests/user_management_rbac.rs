//! OSS user management is for superusers and `role = 'admin'` users only.
//! `can_manage_users` used to allow every signed-in user, so a plain member
//! could reset another member's credentials and sign in as them.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test user_management_rbac -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &common::TestServer, username: &str, role: &str, active: bool) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active, role) \
         VALUES ($1, $2, false, $3, $4::user_role) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .bind(active)
    .bind(role)
    .fetch_one(&server.db)
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT INTO user_credentials (user_id, access_key, access_secret_hash) \
         VALUES ($1, $2, 'seeded-hash')",
    )
    .bind(id)
    .bind(format!("AK-{username}"))
    .execute(&server.db)
    .await
    .expect("seed credentials");
    id
}

async fn regenerate(
    server: &common::TestServer,
    caller: Uuid,
    name: &str,
    target: Uuid,
) -> StatusCode {
    common::as_member(
        server
            .client
            .post(server.url(&format!("/api/users/{target}/regenerate-credentials"))),
        &caller.to_string(),
        name,
    )
    .send()
    .await
    .unwrap()
    .status()
}

async fn secret_hash(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT access_secret_hash FROM user_credentials WHERE user_id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_member_cannot_take_over_another_member() {
    let server = common::TestServer::start().await;
    let mallory = seed_user(&server, "dummy-mallory", "member", true).await;
    let victim = seed_user(&server, "dummy-victim", "member", true).await;

    assert_eq!(
        regenerate(&server, mallory, "dummy-mallory", victim).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(secret_hash(&server, victim).await, "seeded-hash");

    let resp = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/users/{victim}"))),
        &mallory.to_string(),
        "dummy-mallory",
    )
    .json(&json!({"password": "Takeover-Passw0rd!"}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_admin_role_is_read_from_the_database() {
    let server = common::TestServer::start().await;
    let admin = seed_user(&server, "dummy-admin", "admin", true).await;
    let inactive_admin = seed_user(&server, "dummy-old-admin", "admin", false).await;
    let victim = seed_user(&server, "dummy-victim", "member", true).await;

    assert_eq!(
        regenerate(&server, admin, "dummy-admin", victim).await,
        StatusCode::OK
    );
    assert_eq!(
        regenerate(&server, inactive_admin, "dummy-old-admin", victim).await,
        StatusCode::FORBIDDEN,
        "a deactivated admin still manages users"
    );

    // Demoting takes effect on the next request, with the same token.
    sqlx::query("UPDATE users SET role = 'member'::user_role WHERE id = $1")
        .bind(admin)
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        regenerate(&server, admin, "dummy-admin", victim).await,
        StatusCode::FORBIDDEN
    );

    server.cleanup().await;
}
