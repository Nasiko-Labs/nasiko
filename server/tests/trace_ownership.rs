//! NAS-1138 item 2: who owns a trace was decided by the `session.id` span
//! attribute, which agents write. An agent called in Alice's flow could tag
//! her trace with a session its own owner (Mallory) controls, and Mallory could
//! then open the trace, or see it in that session. The server's own records
//! (`session_traces`, `flows`) now decide; a span's claim is used only when the
//! server recorded nothing, and never against a known flow owner.
//!
//! A fake Tempo serves the traces. Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test trace_ownership -- --test-threads=1
mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// Trace id → the `session.id` an agent span in it claims.
type Claims = Arc<HashMap<String, String>>;

const ALICE_TRACE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const ALICE_UNINDEXED_TRACE: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";

fn otlp(session_claim: &str) -> Value {
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let span = |id: &str, parent: Option<&str>, attrs: Value| {
        json!({
            "spanId": id, "parentSpanId": parent, "name": "turn",
            "startTimeUnixNano": now.to_string(),
            "endTimeUnixNano": (now + 1_000_000).to_string(),
            "attributes": attrs, "events": []
        })
    };
    json!({"batches": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "dummy-agent"}}]},
        "scopeSpans": [{"spans": [
            span("1111111111111111", None, json!([])),
            span("2222222222222222", Some("1111111111111111"), json!([
                {"key": "session.id", "value": {"stringValue": session_claim}},
                {"key": "gen_ai.input.messages", "value": {"stringValue": "alice's private prompt"}}
            ]))
        ]}]
    }]})
}

async fn trace(
    State(claims): State<Claims>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    claims
        .get(&id)
        .map(|claim| Json(otlp(claim)))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn search(
    State(claims): State<Claims>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let query = q.get("q").cloned().unwrap_or_default();
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let traces: Vec<Value> = claims
        .iter()
        .filter(|(_, claim)| query.contains(claim.as_str()))
        .map(
            |(id, _)| json!({"traceID": id, "startTimeUnixNano": now.to_string(), "durationMs": 1}),
        )
        .collect();
    Json(json!({"traces": traces}))
}

async fn start_fake_tempo(claims: Claims) -> String {
    let app = Router::new()
        .route("/api/traces/{id}", get(trace))
        .route("/api/search", get(search))
        .with_state(claims);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://127.0.0.1:{port}")
}

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

async fn seed_session(server: &common::TestServer, session: &str, user: Uuid) {
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 'chat')")
        .bind(session)
        .bind(user)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn seed_flow(server: &common::TestServer, flow: &str, user: Uuid) {
    sqlx::query("INSERT INTO flows (flow_id, user_id, status) VALUES ($1, $2, 'completed')")
        .bind(flow)
        .bind(user)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn get_as(server: &common::TestServer, user: Uuid, path: &str) -> (u16, String) {
    let resp = common::as_member(
        server.client.get(server.url(path)),
        &user.to_string(),
        "member",
    )
    .send()
    .await
    .unwrap();
    (
        resp.status().as_u16(),
        resp.text().await.unwrap_or_default(),
    )
}

struct Fixture {
    server: common::TestServer,
    alice: Uuid,
    mallory: Uuid,
    mallory_session: String,
}

async fn fixture() -> Fixture {
    let alice_session = format!("ses_{}", Uuid::new_v4().simple());
    let mallory_session = format!("ses_{}", Uuid::new_v4().simple());
    // Both of Alice's traces carry a span claiming Mallory's session.
    let claims: Claims = Arc::new(HashMap::from([
        (ALICE_TRACE.to_string(), mallory_session.clone()),
        (ALICE_UNINDEXED_TRACE.to_string(), mallory_session.clone()),
    ]));
    let tempo = start_fake_tempo(claims).await;
    let server = common::TestServer::start_with(|c| {
        c.tempo_url = tempo.clone();
        c.loki_url = tempo.clone();
        c.observability_enabled = true;
    })
    .await;

    let alice = seed_user(&server, "dummy-alice").await;
    let mallory = seed_user(&server, "dummy-mallory").await;
    seed_session(&server, &alice_session, alice).await;
    seed_session(&server, &mallory_session, mallory).await;
    seed_flow(&server, ALICE_TRACE, alice).await;
    seed_flow(&server, ALICE_UNINDEXED_TRACE, alice).await;
    sqlx::query("INSERT INTO session_traces (session_id, trace_id) VALUES ($1, $2)")
        .bind(&alice_session)
        .bind(ALICE_TRACE)
        .execute(&server.db)
        .await
        .unwrap();

    Fixture {
        server,
        alice,
        mallory,
        mallory_session,
    }
}

#[tokio::test]
#[serial]
async fn an_agent_tag_does_not_hand_a_trace_to_another_user() {
    let f = fixture().await;

    for trace in [ALICE_TRACE, ALICE_UNINDEXED_TRACE] {
        let (status, body) = get_as(
            &f.server,
            f.mallory,
            &format!("/api/observability/trace/{trace}"),
        )
        .await;
        assert_eq!(status, 404, "{trace}: {body}");
    }
    let (status, body) = get_as(
        &f.server,
        f.alice,
        &format!("/api/observability/trace/{ALICE_TRACE}"),
    )
    .await;
    assert_eq!(status, 200, "the owner lost access: {body}");

    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_tag_does_not_put_a_trace_into_another_users_session() {
    let f = fixture().await;

    let (_, body) = get_as(
        &f.server,
        f.mallory,
        &format!("/api/observability/session/{}", f.mallory_session),
    )
    .await;
    assert!(
        !body.contains(ALICE_TRACE),
        "Alice's trace is in Mallory's session: {body}"
    );
    assert!(
        !body.contains("alice's private prompt"),
        "content leaked: {body}"
    );

    f.server.cleanup().await;
}
