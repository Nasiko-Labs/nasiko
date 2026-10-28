//! NAS-1138 item 1: the agent proxy linked a trace to whatever `contextId` a
//! non-message call (`tasks/get`, `tasks/cancel`, …) carried. Nothing checked
//! that the caller owned that session, so anyone who knew another user's
//! session id could attach their own traces to that user's session view. Only
//! the session the proxy verified for a message send may be recorded now.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test session_trace_ownership -- --test-threads=1
mod common;

use std::time::Duration;

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
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

async fn reply(Json(_): Json<Value>) -> Json<Value> {
    Json(json!({
        "jsonrpc": "2.0", "id": "1",
        "result": {"kind": "task", "status": {"state": "completed"},
                   "artifacts": [{"parts": [{"kind": "text", "text": "stub reply"}]}]}
    }))
}

async fn start_agent() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", post(reply)))
            .await
            .unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

async fn post_as(server: &common::TestServer, agent: Uuid, user: Uuid, body: Value) -> u16 {
    common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent}"))),
        &user.to_string(),
        "member",
    )
    .json(&body)
    .send()
    .await
    .unwrap()
    .status()
    .as_u16()
}

async fn traces(server: &common::TestServer, session: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM session_traces WHERE session_id = $1")
        .bind(session)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

/// The write is fire-and-forget, so poll for it, then give a stray write time to land.
async fn traces_after_settling(server: &common::TestServer, session: &str) -> i64 {
    for _ in 0..40 {
        if traces(server, session).await > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    traces(server, session).await
}

#[tokio::test]
#[serial]
async fn a_caller_cannot_link_traces_to_someone_elses_session() {
    let server = common::TestServer::start().await;
    let alice = seed_user(&server, "dummy-alice").await;
    let mallory = seed_user(&server, "dummy-mallory").await;
    let url = start_agent().await;
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, is_public) \
         VALUES ('dummy-public-agent', $1, 'x:1.0.0', 'running', $2, true) RETURNING id",
    )
    .bind(alice)
    .bind(&url)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Alice opens a session; her own message is linked to it.
    let session = format!("ses_{}", Uuid::new_v4().simple());
    let send = json!({
        "jsonrpc": "2.0", "method": "message/send", "id": "1",
        "params": {"message": {
            "messageId": Uuid::new_v4().to_string(), "role": "user", "contextId": session,
            "parts": [{"kind": "text", "text": "hello"}]
        }}
    });
    assert_eq!(post_as(&server, agent, alice, send).await, 200);
    assert_eq!(traces_after_settling(&server, &session).await, 1);

    // Mallory names Alice's session on a non-message method.
    for method in ["tasks/get", "tasks/cancel"] {
        let probe = json!({
            "jsonrpc": "2.0", "method": method, "id": "2",
            "params": {"id": "task-1", "contextId": session}
        });
        post_as(&server, agent, mallory, probe).await;
    }
    assert_eq!(
        traces_after_settling(&server, &session).await,
        1,
        "a trace was linked to another user's session"
    );

    server.cleanup().await;
}
