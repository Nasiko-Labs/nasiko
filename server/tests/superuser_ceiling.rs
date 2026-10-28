//! NAS-1133: a caller who is not a superuser must not be able to create a
//! superuser, or act on one (reset credentials, update, deactivate, reinstate,
//! delete, change role). User management is also open to `role = 'admin'`
//! users who are not superusers, so without this ceiling they could mint a
//! superuser or take one over.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test superuser_ceiling -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

/// The non-superuser caller in these tests: an admin by role, so it passes
/// `require_user_manager` and only the ceiling can stop it.
const MANAGER: &str = "dummy-manager";

async fn seed_user(server: &common::TestServer, username: &str, superuser: bool) -> Uuid {
    let role = if username == MANAGER {
        "admin"
    } else {
        "member"
    };
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser, is_active, role) \
         VALUES ($1, $2, $3, true, $4::user_role) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .bind(superuser)
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

async fn secret_hash(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT access_secret_hash FROM user_credentials WHERE user_id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_non_superuser_cannot_create_a_superuser() {
    let server = common::TestServer::start().await;
    let member = seed_user(&server, MANAGER, false).await;

    let resp = common::as_member(
        server.client.post(server.url("/api/users")),
        &member.to_string(),
        MANAGER,
    )
    .json(&json!({"username": "dummy-escalated", "email": "esc@local.test", "is_superuser": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let created: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'dummy-escalated'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(created, 0, "the superuser row was written");

    // A plain user is still fine, so the check is the ceiling, not a blanket 403.
    let resp = common::as_member(
        server.client.post(server.url("/api/users")),
        &member.to_string(),
        MANAGER,
    )
    .json(&json!({"username": "dummy-plain", "email": "plain@local.test"}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_non_superuser_cannot_act_on_a_superuser() {
    let server = common::TestServer::start().await;
    let member = seed_user(&server, MANAGER, false).await;
    let root = seed_user(&server, "dummy-root", true).await;
    // A second superuser so the last-admin guard can never be what answers.
    seed_user(&server, "dummy-root-2", true).await;
    let caller = member.to_string();

    let attempts = [
        (
            server
                .client
                .post(server.url(&format!("/api/users/{root}/regenerate-credentials"))),
            None,
        ),
        (
            server.client.put(server.url(&format!("/api/users/{root}"))),
            Some(json!({"password": "Takeover-Passw0rd!"})),
        ),
        (
            server
                .client
                .put(server.url(&format!("/api/users/{root}/role"))),
            Some(json!({"role": "member"})),
        ),
        (
            server
                .client
                .post(server.url(&format!("/api/users/{root}/deactivate"))),
            None,
        ),
        (
            server
                .client
                .post(server.url(&format!("/api/users/{root}/reinstate"))),
            None,
        ),
        (
            server
                .client
                .delete(server.url(&format!("/api/users/{root}"))),
            None,
        ),
    ];
    for (rb, body) in attempts {
        let rb = common::as_member(rb, &caller, MANAGER);
        let rb = match body {
            Some(b) => rb.json(&b),
            None => rb,
        };
        let resp = rb.send().await.unwrap();
        let url = resp.url().to_string();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{url}");
    }

    assert_eq!(
        secret_hash(&server, root).await,
        "seeded-hash",
        "credentials were replaced"
    );
    let (active, superuser): (bool, bool) = sqlx::query_as(
        "SELECT is_active, is_superuser FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(root)
    .fetch_one(&server.db)
    .await
    .expect("the superuser still exists");
    assert!(active && superuser);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_ceiling_does_not_block_normal_management() {
    let server = common::TestServer::start().await;
    let member = seed_user(&server, MANAGER, false).await;
    let other = seed_user(&server, "dummy-dev", false).await;
    let root = seed_user(&server, "dummy-root", true).await;
    let target = seed_user(&server, "dummy-root-2", true).await;

    // A non-superuser admin can still manage a non-superuser.
    let resp = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/users/{other}/regenerate-credentials"))),
        &member.to_string(),
        MANAGER,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A superuser can create a superuser and reset another superuser.
    let resp = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &root.to_string(),
        "dummy-root",
    )
    .json(&json!({"username": "dummy-root-3", "email": "root3@local.test", "is_superuser": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/users/{target}/regenerate-credentials"))),
        &root.to_string(),
        "dummy-root",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_ne!(secret_hash(&server, target).await, "seeded-hash");

    server.cleanup().await;
}
