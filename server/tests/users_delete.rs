//! `DELETE /api/users/{id}` must never leave a user half-deleted (Q3-14). The erasure and
//! deactivation used to run before the final `DELETE FROM users`, which then failed on any row
//! still referencing the user (a soft-deleted agent, an MCP connector) with a 500.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test users_delete -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::Connection;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> String {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["user_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn seed_member(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active, role) \
         VALUES ($1, $2, false, true, 'member') RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .fetch_one(&server.db)
    .await
    .expect("seed user")
}

async fn delete_user(server: &common::TestServer, admin: &str, id: Uuid) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/users/{id}"))),
        admin,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

/// A user whose only agent was soft-deleted can be deleted; the agent row passes to the admin.
#[tokio::test]
#[serial]
async fn a_user_who_owns_only_a_deleted_agent_is_deleted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user = seed_member(&server, "former-owner").await;
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, deleted_at) VALUES ('retired', $1, now()) RETURNING id",
    )
    .bind(user)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let res = delete_user(&server, &admin, user).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // The delete spawns the cold-store erasure, which can hold the test pool's only connection
    // for longer than its acquire timeout, so verify on a connection of our own.
    let mut db = sqlx::PgConnection::connect(&server.state.config.database_url)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    assert_eq!(owner.to_string(), admin);

    server.cleanup().await;
}

/// A row that still blocks the delete refuses it with 409 before anything is erased: the user
/// stays active.
#[tokio::test]
#[serial]
async fn a_user_who_owns_a_connector_is_refused_before_anything_is_erased() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let user = seed_member(&server, "connector-owner").await;
    sqlx::query(
        "INSERT INTO mcp_connectors (provider_type, name, owner_id, url) \
         VALUES ('mcp_server', 'mine', $1, 'http://mcp.local.test')",
    )
    .bind(user)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_user(&server, &admin, user).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = res.json::<Value>().await.unwrap();
    assert_eq!(body["table"], "mcp_connectors");

    let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(active, "a refused delete must not deactivate the user");

    server.cleanup().await;
}
