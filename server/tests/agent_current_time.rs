//! A user's own message reaches the agent with the current date and time as its own last part,
//! the user's text untouched: through direct dispatch (`/api/orchestrator/a2a` naming the agent)
//! and through the agent proxy (`/api/agents/{id}`). Agent-to-agent calls are covered in
//! `agent_proxy_agent_caller.rs`.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_current_time -- --test-threads=1

mod common;

use std::sync::{Arc, Mutex};

use axum::{Router, extract::State, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// Records every request body and answers each with a completed task, as a stream.
#[derive(Clone, Default)]
struct StubAgent {
    seen: Arc<Mutex<Vec<Value>>>,
}

async fn answer(
    State(agent): State<StubAgent>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let req: Value = serde_json::from_slice(&body).unwrap_or_default();
    agent.seen.lock().unwrap().push(req);
    let event =
        nasiko_types::a2a::status_event(nasiko_types::a2a::completed("stub-task", "stub-ctx"));
    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {}\n\n",
            serde_json::to_string(&event).unwrap()
        )))
        .unwrap()
}

async fn start_stub_agent() -> (String, StubAgent) {
    let agent = StubAgent::default();
    let app = Router::new()
        .route("/", post(answer))
        .with_state(agent.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, agent)
}

async fn seed_user(server: &common::TestServer, name: &str) -> String {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (username, email, password_hash, role) \
         VALUES ($1, $1 || '@test.local', '', 'member'::user_role) RETURNING id",
    )
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
    .to_string()
}

async fn seed_agent(server: &common::TestServer, owner: &str, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2::uuid, $3, 'running') \
         RETURNING id",
    )
    .bind(format!("timed-agent-{}", Uuid::new_v4()))
    .bind(owner)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

fn message(method: &str, metadata: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": Uuid::new_v4().to_string(),
        "method": method,
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "contextId": Uuid::new_v4().to_string(),
                "role": "ROLE_USER",
                "parts": [{ "text": "what is on today?" }],
            },
            "metadata": metadata,
        },
    })
}

/// The one request the agent saw: its parts' texts.
fn texts_seen(agent: &StubAgent) -> Vec<String> {
    let seen = agent.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the agent should be called once: {seen:?}");
    seen[0]["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn assert_told_the_time_last(texts: &[String]) {
    assert!(
        texts[0].ends_with("what is on today?"),
        "the user's text stays first: {texts:?}"
    );
    let lead = nasiko_types::agent_clock::NOTE_LEAD;
    assert!(
        texts.last().is_some_and(|t| t.starts_with(lead)),
        "the time goes last: {texts:?}"
    );
    assert_eq!(texts.iter().filter(|t| t.starts_with(lead)).count(), 1);
}

#[tokio::test]
#[serial]
async fn direct_dispatch_tells_the_agent_the_time() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server, "timed-dispatch").await;
    let (url, agent) = start_stub_agent().await;
    let agent_id = seed_agent(&server, &user, &url).await;

    let res = common::as_member(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &user,
        "timed-dispatch",
    )
    .json(&message(
        "message/stream",
        json!({ "agent_id": agent_id.to_string() }),
    ))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let _ = res.text().await;

    assert_told_the_time_last(&texts_seen(&agent));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_agent_proxy_tells_the_agent_the_time() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server, "timed-proxy").await;
    let (url, agent) = start_stub_agent().await;
    let agent_id = seed_agent(&server, &user, &url).await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}"))),
        &user,
        "timed-proxy",
    )
    .json(&message("message/send", json!({})))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let _ = res.text().await;

    assert_told_the_time_last(&texts_seen(&agent));
    server.cleanup().await;
}
