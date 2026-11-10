//! End-to-end tests for the HITL resume dispatcher (`oss/server/src/hitl/mod.rs`) and the
//! `POST /api/hitl/{id}/resolve` validation gate (`oss/server/src/router/hitl.rs`).
//!
//! Regression coverage:
//!   1. The resume dispatcher must route its outbound agent call through `FlowGuard`, the same
//!      cascade-limit chokepoint every other inter-agent call goes through.
//!   2. `POST /api/hitl/{id}/resolve` must reject an empty-string `answer` for `input_required`,
//!      not just `null`.
//!
//! Requires infra (Postgres, Redis) like the rest of the suite:
//!   `just infra` then `cargo test -p nasiko-server --test hitl_resume -- --test-threads=1`

mod common;

use std::time::Duration;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

fn auth(rb: reqwest::RequestBuilder, user_id: Uuid) -> reqwest::RequestBuilder {
    let user_id_str = user_id.to_string();
    common::as_member(rb, &user_id_str, &format!("user_{}", &user_id_str[..8]))
}

async fn seed_user(server: &common::TestServer, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(format!("user_{}", &user_id.to_string()[..8]))
    .bind(format!("user_{}@test.example", &user_id.to_string()[..8]))
    .execute(&server.db)
    .await
    .expect("seed_user");
}

/// A `status = 'running'` agent with a (never actually dialed, for the flow-guard test) URL —
/// `agent_resolver::resolve_target` requires `status IN ('running', 'parked')` to find the row at all, and `FakeRuntime`'s
/// `endpoint()` errors for a container that was never `deploy()`-ed, so this always falls back
/// to the stored `url` (mirrors how `agent_proxy.rs`'s own doc comment describes the fallback).
async fn seed_running_agent(server: &common::TestServer, owner_id: Uuid, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') RETURNING id",
    )
    .bind(format!("hitl-resume-test-agent-{}", Uuid::new_v4()))
    .bind(owner_id)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .expect("seed_running_agent")
}

/// Inserts a `pending` `hitl_requests` row directly (bypassing the pause-detection code paths
/// this suite doesn't need to exercise) — same shape `NewHitlRequest::direct_chat` produces.
async fn seed_pending_hitl_request(
    server: &common::TestServer,
    agent_id: Uuid,
    owner_user_id: Uuid,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, expires_at)
         VALUES ('input_required', 'direct_chat', $1, $2, $3, $4, $5, now() + interval '1 day')
         RETURNING id",
    )
    .bind(agent_id)
    .bind(owner_user_id)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("ctx-{}", Uuid::new_v4()))
    .bind(json!({"message": "what should I do?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed_pending_hitl_request")
}

async fn resolve(server: &common::TestServer, user_id: Uuid, id: Uuid, body: Value) -> Value {
    auth(
        server
            .client
            .post(server.url(&format!("/api/hitl/{id}/resolve")))
            .json(&body),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

/// Poll `hitl_requests.resume_status` until it leaves `not_started`, or panic on timeout.
///
/// The budget has to cover a *retryable* failure running out its cap, not just one attempt:
/// `mark_resume_failed` only flips to `failed` once `resume_dispatch_attempts` reaches
/// `MAX_RESUME_ATTEMPTS` (5), and each retry waits for the dispatcher's 2s poll tick — so roughly
/// 8s, not the 6s an earlier 40-iteration budget allowed.
async fn wait_for_resume_outcome(
    server: &common::TestServer,
    id: Uuid,
) -> (String, Option<String>) {
    for _ in 0..120 {
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT resume_status, resume_last_error FROM hitl_requests WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap();
        if row.0 != "not_started" {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("hitl request {id} never left resume_status = not_started");
}

/// Regression: the resume dispatcher must call `FlowGuard` before contacting the agent. With
/// `flow_max_depth` forced to 0, a fresh flow's depth (0) already meets `>= max_depth`, so
/// `FlowGuard::check` rejects deterministically on the very first call — this proves the check
/// actually runs, since before the fix `deliver()` never touched `flow_guard` at all and this
/// row would instead have gotten a network-level failure (or succeeded, if something happened
/// to be listening) rather than this specific rejection.
#[tokio::test]
#[serial]
async fn hitl_resume_is_rejected_by_flow_guard_when_depth_budget_is_exhausted() {
    let server = common::TestServer::start_with(|c| c.flow_max_depth = 0).await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    // Never actually dialed — FlowGuard must reject before the HTTP call is attempted.
    let agent_id = seed_running_agent(&server, user_id, "http://127.0.0.1:1").await;
    let hitl_id = seed_pending_hitl_request(&server, agent_id, user_id).await;

    let resolved = resolve(&server, user_id, hitl_id, json!({"answer": "go ahead"})).await;
    assert_eq!(resolved["status"], "resolved");

    let (resume_status, resume_last_error) = wait_for_resume_outcome(&server, hitl_id).await;
    assert_eq!(
        resume_status, "failed",
        "a flow-guard rejection must be a clean failure, not left hanging"
    );
    let err = resume_last_error.unwrap_or_default();
    assert!(
        err.contains("flow guard rejected resume"),
        "expected a flow-guard rejection message, got: {err:?}"
    );

    server.cleanup().await;
}

/// Regression: an empty-string `answer` must be rejected the same way a missing (`null`) one
/// already was.
#[tokio::test]
#[serial]
async fn resolve_input_required_rejects_empty_string_answer() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_running_agent(&server, user_id, "http://127.0.0.1:1").await;
    let hitl_id = seed_pending_hitl_request(&server, agent_id, user_id).await;

    let res = auth(
        server
            .client
            .post(server.url(&format!("/api/hitl/{hitl_id}/resolve")))
            .json(&json!({"answer": ""})),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        400,
        "an empty-string answer must be rejected exactly like a missing one"
    );

    // The row must still be pending — untouched by the rejected request — so a real answer can
    // still resolve it.
    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(hitl_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "pending");

    server.cleanup().await;
}

/// Sanity check that a real, non-empty answer is still accepted (guards against the empty-string
/// fix becoming over-strict, e.g. rejecting valid whitespace-padded answers by trimming them away
/// server-side instead of just checking for emptiness).
#[tokio::test]
#[serial]
async fn resolve_input_required_accepts_a_real_answer() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_running_agent(&server, user_id, "http://127.0.0.1:1").await;
    let hitl_id = seed_pending_hitl_request(&server, agent_id, user_id).await;

    let resolved = resolve(&server, user_id, hitl_id, json!({"answer": "yes, proceed"})).await;
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["human_response"]["answer"], "yes, proceed");

    server.cleanup().await;
}
