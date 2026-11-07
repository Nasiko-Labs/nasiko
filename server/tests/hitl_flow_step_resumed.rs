//! Regression: the `flow_steps` row an orchestrator pause leaves at `awaiting_human` must move to
//! `'resumed'` once the human's answer actually reaches the agent.
//!
//! The first attempt at this ran in `orchestrator_stream`'s own `ToolCall` arm, keyed on the
//! *current* turn's `flow_id`. A resumed turn is a new `orchestrator_stream` call with a new flow
//! id, so that UPDATE matched zero rows on every call and the row stayed stuck forever.
//! `hitl/mod.rs::close_resumed_flow_step` closes it from the resume side instead, keyed through
//! `session_traces`.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test hitl_flow_step_resumed -- --test-threads=1`

mod common;

use std::time::Duration;

use axum::{Router, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// The agent's raw registry name. Deliberately contains a space: `flow_steps.agent_name` stores the
/// display-folded form (`A2aTool::agent_display_name`), so a name that folds to itself would let
/// this test pass even if the resume side matched on the raw name.
const AGENT_NAME: &str = "Weather Agent";
const AGENT_DISPLAY_NAME: &str = "Weather-Agent";

fn auth(rb: reqwest::RequestBuilder, user_id: Uuid) -> reqwest::RequestBuilder {
    let user_id_str = user_id.to_string();
    common::as_member(rb, &user_id_str, &format!("user_{}", &user_id_str[..8]))
}

/// Answers a resume with another pause. A *completed* resume would send an orchestrator-origin row
/// on to `trigger_new_orchestrator_turn`, which needs a live LLM; the step has to close either way,
/// which is why `close_resumed_flow_step` runs unconditionally on the disposition.
async fn mock_agent_handler(body: axum::body::Bytes) -> axum::response::Response {
    let req: Value = serde_json::from_slice(&body).unwrap_or_default();
    let task_id = req["params"]["message"]["taskId"]
        .as_str()
        .unwrap_or("mock-task")
        .to_string();
    let context_id = req["params"]["message"]["contextId"]
        .as_str()
        .unwrap_or("mock-ctx")
        .to_string();

    let event = serde_json::to_value(nasiko_types::a2a::status_event(
        nasiko_types::a2a::input_required(&task_id, &context_id, "Which region?"),
    ))
    .unwrap();
    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!("data: {event}\n\n")))
        .unwrap()
}

async fn start_mock_agent() -> String {
    let app = Router::new().route("/", post(mock_agent_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Everything `orchestrator_stream` durably writes when a delegated agent pauses, seeded directly:
/// the chat session, the paused turn's flow (filed under that session via `flows.session_id`), the
/// `flow_steps` row stuck at `awaiting_human`, and the orchestrator-origin `hitl_requests` row.
/// Returns `(hitl_request_id, flow_step_id)`.
#[allow(clippy::too_many_arguments)]
async fn seed_paused_orchestrator_turn(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
    chat_session_id: &str,
    paused_flow_id: &str,
) -> (Uuid, Uuid) {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, NULL, '/api/orchestrator/a2a', 'resumed step test')",
    )
    .bind(chat_session_id)
    .bind(user_id)
    .execute(&server.db)
    .await
    .expect("seed chat_sessions");

    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, session_id) \
         VALUES ($1, $2, 'orchestrator', 'paused turn', 'paused', $3)",
    )
    .bind(paused_flow_id)
    .bind(user_id)
    .bind(chat_session_id)
    .execute(&server.db)
    .await
    .expect("seed flows");

    let flow_step_id: Uuid = sqlx::query_scalar(
        "INSERT INTO flow_steps \
            (flow_id, step_order, depth, agent_name, caller_agent_name, input_summary, status) \
         VALUES ($1, 1, 1, $2, 'orchestrator', 'what is the weather?', 'awaiting_human') \
         RETURNING id",
    )
    .bind(paused_flow_id)
    .bind(AGENT_DISPLAY_NAME)
    .fetch_one(&server.db)
    .await
    .expect("seed flow_steps");

    let hitl_id: Uuid = sqlx::query_scalar(
        "INSERT INTO hitl_requests \
            (kind, origin, agent_id, owner_user_id, task_id, context_id, chat_session_id, \
             question, expires_at) \
         VALUES ('input_required', 'orchestrator', $1, $2, $3, $4, $5, $6, now() + interval '1 day') \
         RETURNING id",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("agent-ctx-{}", Uuid::new_v4()))
    .bind(chat_session_id)
    .bind(json!({"message": "which region?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed hitl_requests");

    (hitl_id, flow_step_id)
}

/// A second paused turn in an *already-seeded* chat session (so `chat_sessions` isn't
/// double-inserted) — same shape as [`seed_paused_orchestrator_turn`] minus that one insert.
/// Returns `(hitl_request_id, flow_step_id)`.
async fn seed_second_paused_turn_in_the_same_chat(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
    chat_session_id: &str,
    paused_flow_id: &str,
) -> (Uuid, Uuid) {
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, session_id) \
         VALUES ($1, $2, 'orchestrator', 'paused turn', 'paused', $3)",
    )
    .bind(paused_flow_id)
    .bind(user_id)
    .bind(chat_session_id)
    .execute(&server.db)
    .await
    .expect("seed flows");

    let flow_step_id: Uuid = sqlx::query_scalar(
        "INSERT INTO flow_steps \
            (flow_id, step_order, depth, agent_name, caller_agent_name, input_summary, status) \
         VALUES ($1, 1, 1, $2, 'orchestrator', 'what is the weather?', 'awaiting_human') \
         RETURNING id",
    )
    .bind(paused_flow_id)
    .bind(AGENT_DISPLAY_NAME)
    .fetch_one(&server.db)
    .await
    .expect("seed flow_steps");

    let hitl_id: Uuid = sqlx::query_scalar(
        "INSERT INTO hitl_requests \
            (kind, origin, agent_id, owner_user_id, task_id, context_id, chat_session_id, \
             question, expires_at) \
         VALUES ('input_required', 'orchestrator', $1, $2, $3, $4, $5, $6, now() + interval '1 day') \
         RETURNING id",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("agent-ctx-{}", Uuid::new_v4()))
    .bind(chat_session_id)
    .bind(json!({"message": "which region?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed hitl_requests");

    (hitl_id, flow_step_id)
}

async fn flow_step_status(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM flow_steps WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .expect("read flow_steps.status")
}

/// Poll until the dispatcher records an outcome, so the assertion below never races delivery.
async fn wait_for_resume_outcome(server: &common::TestServer, id: Uuid) -> String {
    for _ in 0..80 {
        let status: String =
            sqlx::query_scalar("SELECT resume_status FROM hitl_requests WHERE id = $1")
                .bind(id)
                .fetch_one(&server.db)
                .await
                .unwrap();
        if status != "not_started" {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("hitl request {id} never left resume_status = not_started");
}

#[tokio::test]
#[serial]
async fn resuming_an_orchestrator_pause_closes_its_awaiting_human_flow_step() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("user_{}", &user_id.to_string()[..8]))
        .bind(format!("user_{}@test.example", &user_id.to_string()[..8]))
        .execute(&server.db)
        .await
        .expect("seed user");

    let agent_url = start_mock_agent().await;
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') \
         RETURNING id",
    )
    .bind(AGENT_NAME)
    .bind(user_id)
    .bind(&agent_url)
    .fetch_one(&server.db)
    .await
    .expect("seed agent");

    let chat_session_id = format!("ses_{}", Uuid::new_v4().simple());
    let paused_flow_id = Uuid::new_v4().simple().to_string();
    let (hitl_id, flow_step_id) = seed_paused_orchestrator_turn(
        &server,
        user_id,
        agent_id,
        &chat_session_id,
        &paused_flow_id,
    )
    .await;

    assert_eq!(
        flow_step_status(&server, flow_step_id).await,
        "awaiting_human",
        "precondition: the seeded step starts paused"
    );

    let res = auth(
        server
            .client
            .post(server.url(&format!("/api/hitl/{hitl_id}/resolve")))
            .json(&json!({"answer": "europe"})),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "resolve must succeed");

    let resume_status = wait_for_resume_outcome(&server, hitl_id).await;
    assert_eq!(
        resume_status, "completed",
        "the mock agent accepted the resume, so delivery must be recorded as completed"
    );

    assert_eq!(
        flow_step_status(&server, flow_step_id).await,
        "resumed",
        "the paused step must leave `awaiting_human` once the answer reached the agent — it \
         belongs to the PAUSED turn's flow, which the resumed turn's own flow id never matches"
    );

    server.cleanup().await;
}

/// Regression: an orchestrator session that delegates twice to the SAME sub-agent, with both
/// calls paused, must have answering one of them close only that one `flow_steps` row — not both.
/// `close_resumed_flow_step` matches on `(chat_session_id, agent_name)` alone (via the
/// `session_traces` mapping), which is ambiguous whenever more than one `awaiting_human` row for
/// this agent exists in the same chat; before this fix the UPDATE had no row limit at all and
/// closed every match, falsely reporting a still-genuinely-paused step as resumed (found in
/// review).
#[tokio::test]
#[serial]
async fn resuming_one_of_two_concurrent_pauses_for_the_same_agent_closes_only_one_flow_step() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("user_{}", &user_id.to_string()[..8]))
        .bind(format!("user_{}@test.example", &user_id.to_string()[..8]))
        .execute(&server.db)
        .await
        .expect("seed user");

    let agent_url = start_mock_agent().await;
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') \
         RETURNING id",
    )
    .bind(AGENT_NAME)
    .bind(user_id)
    .bind(&agent_url)
    .fetch_one(&server.db)
    .await
    .expect("seed agent");

    let chat_session_id = format!("ses_{}", Uuid::new_v4().simple());
    let first_flow_id = Uuid::new_v4().simple().to_string();
    let (first_hitl_id, first_flow_step_id) =
        seed_paused_orchestrator_turn(&server, user_id, agent_id, &chat_session_id, &first_flow_id)
            .await;
    let second_flow_id = Uuid::new_v4().simple().to_string();
    let (_second_hitl_id, second_flow_step_id) = seed_second_paused_turn_in_the_same_chat(
        &server,
        user_id,
        agent_id,
        &chat_session_id,
        &second_flow_id,
    )
    .await;

    assert_eq!(
        flow_step_status(&server, first_flow_step_id).await,
        "awaiting_human"
    );
    assert_eq!(
        flow_step_status(&server, second_flow_step_id).await,
        "awaiting_human"
    );

    // Answer only the first pause.
    let res = auth(
        server
            .client
            .post(server.url(&format!("/api/hitl/{first_hitl_id}/resolve")))
            .json(&json!({"answer": "europe"})),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200, "resolve must succeed");
    wait_for_resume_outcome(&server, first_hitl_id).await;

    let statuses = [
        flow_step_status(&server, first_flow_step_id).await,
        flow_step_status(&server, second_flow_step_id).await,
    ];
    assert_eq!(
        statuses.iter().filter(|s| s.as_str() == "resumed").count(),
        1,
        "exactly one paused step must close when only one hitl_request is resolved, got {statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| s.as_str() == "awaiting_human")
            .count(),
        1,
        "the other, still-genuinely-paused step must not be falsely marked resumed, got {statuses:?}"
    );

    server.cleanup().await;
}
