//! `POST /api/users` — regression coverage for the `is_superuser` field, which
//! used to be hardcoded `false` in the INSERT regardless of the request body
//! (see `oss/server/src/users/routes.rs::create_user`).
//!
//!   cargo test -p nasiko-server --test users_create -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
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

/// Sending `is_superuser: true` at creation must actually set the column —
/// this was previously silently discarded (hardcoded `false` in the INSERT).
#[tokio::test]
#[serial]
async fn create_user_with_is_superuser_true_actually_sets_it() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin,
        "admin",
    )
    .json(&json!({
        "username": "new-superuser",
        "email": "new-superuser@test.local",
        "role": "admin",
        "is_superuser": true,
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body = res.json::<Value>().await.unwrap();
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    let (role, is_superuser): (String, bool) =
        sqlx::query_as("SELECT role::text, is_superuser FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(role, "admin");
    assert!(
        is_superuser,
        "is_superuser: true in the request must persist"
    );

    server.cleanup().await;
}

/// Omitting `is_superuser` (the common case — every existing caller) must
/// still default to `false`, unchanged from before this field existed.
#[tokio::test]
#[serial]
async fn create_user_without_is_superuser_field_defaults_false() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin,
        "admin",
    )
    .json(&json!({
        "username": "plain-member",
        "email": "plain-member@test.local",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    let body = res.json::<Value>().await.unwrap();
    let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    let is_superuser: bool = sqlx::query_scalar("SELECT is_superuser FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!is_superuser, "must default to false when field is omitted");

    server.cleanup().await;
}

/// A role the database rejects is the caller's mistake (Q3-15): 400 with the database's reason,
/// not 500. In OSS that is an unknown `user_role` value; an EE trigger refusing a retired role
/// takes the same path.
#[tokio::test]
#[serial]
async fn create_user_with_a_role_the_database_rejects_is_a_bad_request() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;

    let res = common::as_superuser(
        server.client.post(server.url("/api/users")),
        &admin,
        "admin",
    )
    .json(&json!({
        "username": "bad-role",
        "email": "bad-role@test.local",
        "role": "no_such_role",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);
    assert!(res.text().await.unwrap().contains("no_such_role"));

    server.cleanup().await;
}
