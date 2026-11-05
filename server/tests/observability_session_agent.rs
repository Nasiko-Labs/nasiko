//! Which agent a session shows. A direct chat has one; an orchestrator-routed chat picks its agent
//! per turn, so the list and the detail name the agent that did the work (the one with the most
//! tokens), never the orchestrator's own model calls. The detail uses the same SQL fragment
//! (`sessions_sql::ran_agent_name`), but needs a trace store to answer, so only the list is
//! exercised here.
//!
//!   cargo test -p nasiko-server --test observability_session_agent -- --test-threads=1

mod common;

use serde_json::Value;
use serial_test::serial;
use uuid::Uuid;

const LIST: &str = "/api/observability/session/list?limit=100&start_time=2020-01-01T00:00:00Z";

async fn seed_user(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('owner', 'owner@test.local') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'x:1', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_session(
    server: &common::TestServer,
    user: Uuid,
    session: &str,
    agent: Option<Uuid>,
    session_type: &str,
) {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title, session_type) \
         VALUES ($1, $2, $3, 'title', $4)",
    )
    .bind(session)
    .bind(user)
    .bind(agent)
    .bind(session_type)
    .execute(&server.db)
    .await
    .unwrap();
}

/// One `trace_usage` row. `agent` is `None` for the orchestrator's own model calls.
async fn seed_usage(
    server: &common::TestServer,
    session: &str,
    user: Uuid,
    name: &str,
    agent: Option<Uuid>,
    tokens: i64,
) {
    sqlx::query(
        "INSERT INTO trace_usage \
         (trace_id, agent_name, session_id, agent_id, user_id, input_tokens, started_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now())",
    )
    .bind(Uuid::new_v4().simple().to_string())
    .bind(name)
    .bind(session)
    .bind(agent)
    .bind(user)
    .bind(tokens)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn get(server: &common::TestServer, user: Uuid, path: &str) -> Value {
    let res = common::as_member(server.client.get(server.url(path)), &user.to_string(), "u")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{path}");
    res.json().await.unwrap()
}

fn listed_agent(list: &Value, session: &str) -> String {
    list["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == session)
        .unwrap_or_else(|| panic!("{session} not listed: {list}"))["agent_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
#[serial]
async fn an_orchestrator_session_shows_the_agent_that_did_the_most_work() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server).await;
    let translator = seed_agent(&server, user, "translator").await;
    let calendar = seed_agent(&server, user, "calendar").await;
    seed_session(&server, user, "orch", None, "orchestrator").await;
    // The orchestrator's own calls are the largest, and must not be named.
    seed_usage(&server, "orch", user, "nasiko-cp", None, 5000).await;
    seed_usage(&server, "orch", user, "translator", Some(translator), 300).await;
    seed_usage(&server, "orch", user, "calendar", Some(calendar), 100).await;

    let list = get(&server, user, LIST).await;
    assert_eq!(listed_agent(&list, "orch"), "translator");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_direct_chat_keeps_its_own_agent() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server).await;
    let own = seed_agent(&server, user, "own").await;
    let other = seed_agent(&server, user, "other").await;
    seed_session(&server, user, "direct", Some(own), "direct_chat").await;
    seed_usage(&server, "direct", user, "other", Some(other), 9000).await;

    let list = get(&server, user, LIST).await;
    assert_eq!(listed_agent(&list, "direct"), "own");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_orchestrator_session_that_called_no_agent_has_no_agent() {
    let server = common::TestServer::start().await;
    let user = seed_user(&server).await;
    seed_session(&server, user, "chatty", None, "orchestrator").await;
    seed_usage(&server, "chatty", user, "nasiko-cp", None, 800).await;

    let list = get(&server, user, LIST).await;
    assert_eq!(listed_agent(&list, "chatty"), "");

    server.cleanup().await;
}
