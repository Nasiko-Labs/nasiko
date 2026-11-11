//! A direct-agent turn whose model calls the gateway refused ends as failed with that refusal,
//! even when the agent answered with a friendly "completed" reply of its own. Without a
//! refusing gate (the open-source default) the same reply stays completed.
//!
//!   cargo test -p nasiko-server --test direct_turn_budget_refusal -- --test-threads=1

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::post;
use nasiko_server::spend_gate::SpendGate;
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const REFUSAL: &str = r#"{"code":"budget_exceeded","error":"Your monthly LLM budget is used up."}"#;
const AGENT_WORDS: &str = "I couldn't reach the language model. Please try again.";

struct RefusingGate;

#[async_trait]
impl SpendGate for RefusingGate {
    async fn refuse(&self, _user_id: Uuid, _chat_session_id: &str) -> Option<String> {
        None
    }

    async fn refusal_in_flow(&self, _flow_id: &str) -> Option<String> {
        Some(REFUSAL.to_string())
    }
}

async fn init_admin(server: &common::TestServer) -> (String, Uuid) {
    let v = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let id = v["user_id"].as_str().unwrap().to_string();
    (id.clone(), Uuid::parse_str(&id).unwrap())
}

async fn seed_running_agent(db: &PgPool, owner: Uuid, url: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, description, status, owner_id, url, skills, tags)
         VALUES ('refused-turn-agent', 'Budget refusal test agent', 'running', $1, $2, '[]'::jsonb, '{}')
         RETURNING id",
    )
    .bind(owner)
    .bind(url)
    .fetch_one(db)
    .await
    .expect("insert running agent")
}

/// An agent that answers `message/stream` with a completed task, as one SSE frame or as plain
/// JSON, saying `AGENT_WORDS` either way.
async fn start_agent(streaming: bool) -> String {
    let reply = json!({
        "result": {
            "statusUpdate": {
                "taskId": "stub-task-1",
                "contextId": "stub-context-1",
                "status": {
                    "state": "TASK_STATE_COMPLETED",
                    "message": { "role": "ROLE_AGENT", "parts": [{ "text": AGENT_WORDS }] }
                },
                "final": true
            }
        },
        "id": "1",
        "jsonrpc": "2.0"
    });
    let app = axum::Router::new().route(
        "/",
        post(move || {
            let reply = reply.clone();
            async move {
                if streaming {
                    (
                        [(CONTENT_TYPE, "text/event-stream")],
                        format!("data: {reply}\n\n"),
                    )
                        .into_response()
                } else {
                    ([(CONTENT_TYPE, "application/json")], reply.to_string()).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/")
}

/// Dispatch one turn to the agent and return the whole SSE body.
async fn dispatch(server: &common::TestServer, admin_id: &str, agent_id: Uuid) -> String {
    let req = server.client.post(server.url("/api/orchestrator/a2a"));
    let res = common::as_superuser(req, admin_id, "admin")
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "message/stream",
            "id": Uuid::new_v4().to_string(),
            "params": {
                "message": {
                    "messageId": Uuid::new_v4().to_string(),
                    "role": "ROLE_USER",
                    "parts": [{ "text": "summarize my meeting" }]
                },
                "metadata": { "agent_id": agent_id.to_string() }
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.status());
    res.text().await.unwrap()
}

async fn flow_outcome(server: &common::TestServer) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error_message FROM flows")
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn run(server: common::TestServer, streaming: bool) -> (String, (String, Option<String>)) {
    let (admin_id, admin) = init_admin(&server).await;
    let url = start_agent(streaming).await;
    let agent = seed_running_agent(&server.db, admin, &url).await;
    let body = dispatch(&server, &admin_id, agent).await;
    let outcome = flow_outcome(&server).await;
    server.cleanup().await;
    (body, outcome)
}

#[tokio::test]
#[serial]
async fn a_streamed_reply_ends_failed_with_the_refusal() {
    let server = common::TestServer::start_with_spend_gate(Arc::new(RefusingGate)).await;
    let (body, (status, error)) = run(server, true).await;
    assert!(body.contains("TASK_STATE_FAILED"), "{body}");
    assert!(body.contains("budget_exceeded"), "{body}");
    assert_eq!(
        (status.as_str(), error.as_deref()),
        ("failed", Some(REFUSAL))
    );
}

#[tokio::test]
#[serial]
async fn a_plain_json_reply_ends_failed_with_the_refusal() {
    let server = common::TestServer::start_with_spend_gate(Arc::new(RefusingGate)).await;
    let (body, (status, error)) = run(server, false).await;
    assert!(body.contains("TASK_STATE_FAILED"), "{body}");
    assert!(body.contains("budget_exceeded"), "{body}");
    assert!(!body.contains("TASK_STATE_COMPLETED"), "{body}");
    assert_eq!(
        (status.as_str(), error.as_deref()),
        ("failed", Some(REFUSAL))
    );
}

#[tokio::test]
#[serial]
async fn without_a_refusal_the_reply_stays_completed() {
    for streaming in [true, false] {
        let server = common::TestServer::start().await;
        let (body, (status, error)) = run(server, streaming).await;
        assert!(!body.contains("TASK_STATE_FAILED"), "{body}");
        assert!(!body.contains("budget_exceeded"), "{body}");
        assert_eq!((status.as_str(), error), ("completed", None));
    }
}
