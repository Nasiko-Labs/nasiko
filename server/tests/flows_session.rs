//! `POST /api/flows` stores `flows.session_id`: the caller's session when named, otherwise the
//! flow itself (single-run session — the same rule as the 0085 backfill).
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test flows_session -- --test-threads=1
mod common;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@local.test"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn stored_session(server: &common::TestServer, flow_id: &str) -> String {
    sqlx::query_scalar("SELECT session_id FROM flows WHERE flow_id = $1")
        .bind(flow_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn create(server: &common::TestServer, user: Uuid, body: serde_json::Value) -> u16 {
    let rb = reqwest::Client::new()
        .post(server.url("/api/flows"))
        .json(&body);
    common::as_member(rb, &user.to_string(), "flows-session")
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
#[serial]
async fn create_flow_without_session_uses_flow_id() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server, &format!("fs-{}", Uuid::new_v4().simple())).await;
    let flow_id = Uuid::new_v4().simple().to_string();

    let status = create(&server, user, json!({ "flow_id": flow_id })).await;

    assert_eq!(status, 201);
    assert_eq!(stored_session(&server, &flow_id).await, flow_id);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_flow_with_session_keeps_it() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server, &format!("fs-{}", Uuid::new_v4().simple())).await;
    let flow_id = Uuid::new_v4().simple().to_string();

    let status = create(
        &server,
        user,
        json!({ "flow_id": flow_id, "session_id": "ctx-named" }),
    )
    .await;

    assert_eq!(status, 201);
    assert_eq!(stored_session(&server, &flow_id).await, "ctx-named");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn create_flow_in_another_users_session_is_forbidden() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, &format!("fs-{}", Uuid::new_v4().simple())).await;
    let other = seed_user(&server, &format!("fs-{}", Uuid::new_v4().simple())).await;
    let sid = format!("sess-{}", Uuid::new_v4().simple());

    let first = create(
        &server,
        owner,
        json!({ "flow_id": Uuid::new_v4().simple().to_string(), "session_id": sid }),
    )
    .await;
    assert_eq!(first, 201);

    let stolen = create(
        &server,
        other,
        json!({ "flow_id": Uuid::new_v4().simple().to_string(), "session_id": sid }),
    )
    .await;
    assert_eq!(stolen, 403);
    server.cleanup().await;
}
