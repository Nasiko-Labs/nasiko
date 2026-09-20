mod common;

use chrono::{TimeZone, Utc};
use nasiko_types::{
    CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentLlmCall,
    CodingAgentSession, CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
    CodingAgentToolCall, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id,
    coding_agent_session_id,
};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn setup(server: &common::TestServer) -> (Uuid, Uuid) {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = Uuid::parse_str(admin["user_id"].as_str().unwrap()).unwrap();
    let agent_id = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('coding-agent', $1, 'claude') RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    (user_id, agent_id)
}

fn event(session: &str, turn: &str, policy: CapturePolicy) -> CodingAgentEventV1 {
    let started_at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let ended_at = Utc.timestamp_opt(1_700_000_002, 0).unwrap();
    CodingAgentEventV1 {
        version: CODING_AGENT_EVENT_VERSION,
        event_id: coding_agent_event_id("claude", session, turn),
        captured_at: ended_at,
        source: CodingAgentSource {
            agent_id: "claude".into(),
            agent_name: "coding-agent".into(),
        },
        session: CodingAgentSession {
            id: coding_agent_session_id("claude", session),
            source_id: session.into(),
        },
        turn: CodingAgentTurn {
            id: turn.into(),
            prompt: (policy == CapturePolicy::Content).then(|| "question".into()),
            response: (policy == CapturePolicy::Content).then(|| "answer".into()),
            started_at,
            ended_at,
            llm_calls: vec![CodingAgentLlmCall {
                id: format!("call-{turn}"),
                provider: "anthropic".into(),
                model: "claude-test".into(),
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 2,
                cache_creation_tokens: 3,
                started_at,
                ended_at,
            }],
            tool_calls: vec![],
        },
        capture_policy: policy,
    }
}

async fn post(server: &common::TestServer, user_id: Uuid, events: &[CodingAgentEventV1]) -> Value {
    common::as_member(
        server
            .client
            .post(server.url("/api/telemetry/coding-agent/events/batch")),
        &user_id.to_string(),
        "admin",
    )
    .json(&json!({"events": events}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn accepts_content_and_metadata_and_handles_replays_independently() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let mut content = event("content-session", "turn-1", CapturePolicy::Content);
    content.turn.tool_calls.push(CodingAgentToolCall {
        id: "native-tool".into(),
        name: "read_file".into(),
        kind: "tool".into(),
        model_call_id: Some("call-turn-1".into()),
        status: CodingAgentToolCallStatus::Succeeded,
        arguments: Some(json!({"path": "redacted"})),
        output: Some(json!("ok")),
        raw: None,
        error: None,
        started_at: Some(content.turn.started_at),
        ended_at: Some(content.turn.ended_at),
        duration_ms: Some(2000),
        association: CodingAgentToolAssociation::Exact,
        timestamp_quality: CodingAgentTimestampQuality::Exact,
    });
    let metadata = event("metadata-session", "turn-1", CapturePolicy::MetadataOnly);

    let first = post(&server, user_id, &[content.clone(), metadata.clone()]).await;
    assert_eq!(first["data"]["results"][0]["status"], "accepted");
    assert_eq!(first["data"]["results"][1]["status"], "accepted");
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM coding_agent_telemetry_events WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(receipt_count, 2);
    let content_session_id: String = sqlx::query_scalar(
        "SELECT session_id FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&content.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let metadata_session_id: String = sqlx::query_scalar(
        "SELECT session_id FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&metadata.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let content_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
            .bind(&content_session_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    let metadata_messages: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
            .bind(&metadata_session_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(content_messages, 2);
    assert_eq!(metadata_messages, 0);
    let assistant_metadata: Value = sqlx::query_scalar(
        "SELECT metadata FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(&content_session_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        assistant_metadata["coding_agent"]["capture_policy"],
        "content"
    );
    assert_eq!(
        assistant_metadata["coding_agent"]["tool_calls"][0]["id"],
        "native-tool"
    );

    let replay = post(&server, user_id, std::slice::from_ref(&content)).await;
    assert_eq!(replay["data"]["results"][0]["status"], "duplicate");
    let mut changed = content.clone();
    changed.turn.response = Some("changed".into());
    let conflict = post(&server, user_id, &[changed]).await;
    assert_eq!(conflict["data"]["results"][0]["status"], "rejected");

    let other_user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, 'other', 'other@test.local')")
        .bind(other_user)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agents (name, owner_id) VALUES ('other-agent', $1)")
        .bind(other_user)
        .execute(&server.db)
        .await
        .unwrap();
    let mut wrong_owner = event("wrong-owner", "turn", CapturePolicy::MetadataOnly);
    wrong_owner.source.agent_name = "other-agent".into();
    let valid = event("mixed", "valid", CapturePolicy::MetadataOnly);
    let mixed = post(&server, user_id, &[wrong_owner, valid]).await;
    assert_eq!(mixed["data"]["results"][0]["status"], "rejected");
    assert_eq!(mixed["data"]["results"][1]["status"], "accepted");

    let stored_agent: Uuid = sqlx::query_scalar(
        "SELECT agent_id FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&content.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(stored_agent, agent_id);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn content_turns_keep_source_order_when_ingested_out_of_order() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let mut earlier = event("ordered-session", "turn-1", CapturePolicy::Content);
    earlier.turn.prompt = Some("first question".into());
    earlier.turn.response = Some("first answer".into());
    let mut later = event("ordered-session", "turn-2", CapturePolicy::Content);
    later.turn.started_at += chrono::Duration::minutes(1);
    later.turn.ended_at += chrono::Duration::minutes(1);
    later.captured_at = later.turn.ended_at;
    later.turn.prompt = Some("second question".into());
    later.turn.response = Some("second answer".into());

    let response = post(&server, user_id, &[later, earlier]).await;
    assert_eq!(response["data"]["results"][0]["status"], "accepted");
    assert_eq!(response["data"]["results"][1]["status"], "accepted");
    let contents: Vec<String> =
        sqlx::query_scalar("SELECT content FROM chat_messages ORDER BY timestamp, id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(
        contents,
        [
            "first question",
            "first answer",
            "second question",
            "second answer"
        ]
    );
    let bounds: (chrono::DateTime<Utc>, chrono::DateTime<Utc>) = sqlx::query_as(
        "SELECT created_at, updated_at FROM chat_sessions WHERE title = 'Coding session'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(bounds.0, Utc.timestamp_opt(1_700_000_000, 0).unwrap());
    assert!(
        bounds.1 >= Utc.timestamp_opt(1_700_000_002, 0).unwrap() + chrono::Duration::minutes(1)
    );
}

#[tokio::test]
#[serial]
async fn route_requires_authentication() {
    let server = common::TestServer::start().await;
    let response = server
        .client
        .post(server.url("/api/telemetry/coding-agent/events/batch"))
        .json(&json!({"events": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn route_rejects_bodies_larger_than_eight_mib() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let response = common::as_member(
        server
            .client
            .post(server.url("/api/telemetry/coding-agent/events/batch")),
        &user_id.to_string(),
        "admin",
    )
    .header(reqwest::header::CONTENT_TYPE, "application/json")
    .body(format!(
        r#"{{"events":[],"padding":"{}"}}"#,
        "x".repeat(8 * 1024 * 1024)
    ))
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn concurrent_identical_events_create_one_receipt() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let event = event("concurrent", "turn", CapturePolicy::Content);
    let (left, right) = tokio::join!(
        post(&server, user_id, std::slice::from_ref(&event)),
        post(&server, user_id, std::slice::from_ref(&event))
    );
    let statuses = [
        left["data"]["results"][0]["status"].as_str().unwrap(),
        right["data"]["results"][0]["status"].as_str().unwrap(),
    ];
    assert!(statuses.contains(&"accepted"));
    assert!(statuses.contains(&"duplicate"));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn same_native_session_is_scoped_to_each_owned_agent() {
    let server = common::TestServer::start().await;
    let (first_user, _) = setup(&server).await;
    let second_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, 'second', 'second@test.local')",
    )
    .bind(second_user)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('coding-agent', $1, 'claude')",
    )
    .bind(second_user)
    .execute(&server.db)
    .await
    .unwrap();
    let shared = event("same-native-session", "turn", CapturePolicy::Content);

    assert_eq!(
        post(&server, first_user, std::slice::from_ref(&shared)).await["data"]["results"][0]["status"],
        "accepted"
    );
    assert_eq!(
        post(&server, second_user, std::slice::from_ref(&shared)).await["data"]["results"][0]["status"],
        "accepted"
    );
    let sessions: Vec<String> = sqlx::query_scalar(
        "SELECT session_id FROM coding_agent_telemetry_events WHERE event_id = $1 ORDER BY user_id",
    )
    .bind(&shared.event_id)
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(sessions.len(), 2);
    assert_ne!(sessions[0], sessions[1]);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn outbox_exports_traces_then_logs_and_marks_delivered() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let event = event("export", "turn", CapturePolicy::Content);
    let response = post(&server, user_id, std::slice::from_ref(&event)).await;
    assert_eq!(response["data"]["results"][0]["status"], "accepted");
    sqlx::query(
        "UPDATE coding_agent_telemetry_events SET otlp_state = 'processing', otlp_last_attempt_at = now() - interval '10 minutes', otlp_claim_id = $3 WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .bind(Uuid::new_v4())
    .execute(&server.db)
    .await
    .unwrap();

    let mut collector = mockito::Server::new_async().await;
    let traces = collector
        .mock("POST", "/v1/traces")
        .match_header("content-type", "application/json")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let logs = collector
        .mock("POST", "/v1/logs")
        .match_header("content-type", "application/json")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;

    let collector_url = collector.url();
    let (left, right) = tokio::join!(
        nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &collector_url),
        nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &collector_url),
    );
    assert_eq!(left.unwrap() + right.unwrap(), 1);
    traces.assert_async().await;
    logs.assert_async().await;
    let state: String = sqlx::query_scalar(
        "SELECT otlp_state FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(state, "delivered");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn outbox_failure_records_error_and_can_retry() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let event = event("retry", "turn", CapturePolicy::MetadataOnly);
    post(&server, user_id, std::slice::from_ref(&event)).await;

    let mut failing = mockito::Server::new_async().await;
    let failure = failing
        .mock("POST", "/v1/traces")
        .with_status(503)
        .with_body("collector unavailable")
        .expect(1)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &failing.url())
        .await
        .unwrap();
    failure.assert_async().await;
    let failed: (String, i32, Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT otlp_state, otlp_attempts, otlp_last_error, otlp_last_error_at FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(failed.0, "failed");
    assert_eq!(failed.1, 1);
    assert!(failed.2.unwrap().contains("503 Service Unavailable"));
    assert!(failed.3.is_some());

    sqlx::query(
        "UPDATE coding_agent_telemetry_events SET otlp_next_attempt_at = now() WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .execute(&server.db)
    .await
    .unwrap();
    let mut recovered = mockito::Server::new_async().await;
    let traces = recovered
        .mock("POST", "/v1/traces")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let logs = recovered
        .mock("POST", "/v1/logs")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &recovered.url())
        .await
        .unwrap();
    traces.assert_async().await;
    logs.assert_async().await;
    let delivered: (String, i32) = sqlx::query_as(
        "SELECT otlp_state, otlp_attempts FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(delivered, ("delivered".into(), 2));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn outbox_retry_skips_an_already_delivered_trace() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let event = event("partial", "turn", CapturePolicy::MetadataOnly);
    post(&server, user_id, std::slice::from_ref(&event)).await;

    let mut partial = mockito::Server::new_async().await;
    let trace = partial
        .mock("POST", "/v1/traces")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let failed_log = partial
        .mock("POST", "/v1/logs")
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &partial.url())
        .await
        .unwrap();
    trace.assert_async().await;
    failed_log.assert_async().await;
    let trace_delivered: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT otlp_trace_delivered_at FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id).bind(&event.event_id).fetch_one(&server.db).await.unwrap();
    assert!(trace_delivered.is_some());

    sqlx::query("UPDATE coding_agent_telemetry_events SET otlp_next_attempt_at = now() WHERE user_id = $1 AND event_id = $2")
        .bind(user_id).bind(&event.event_id).execute(&server.db).await.unwrap();
    let mut recovered = mockito::Server::new_async().await;
    let log = recovered
        .mock("POST", "/v1/logs")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &recovered.url())
        .await
        .unwrap();
    log.assert_async().await;
    let state: String = sqlx::query_scalar(
        "SELECT otlp_state FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(state, "delivered");
    server.cleanup().await;
}

/// Receipts are an OTLP outbox, not a system of record — so deleting the chat
/// session takes them with it. Guards the 0017 FK, which was RESTRICT and made
/// delete_session 500 *after* it had already dropped the session's files.
#[tokio::test]
#[serial]
async fn deleting_the_chat_session_cascades_its_receipts() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let event = event("deletable", "turn-1", CapturePolicy::Content);
    post(&server, user_id, std::slice::from_ref(&event)).await;

    let session_id: String = sqlx::query_scalar(
        "SELECT session_id FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(&event.event_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let deleted = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/chat/sessions/{session_id}"))),
        &user_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(deleted.status(), 204);

    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM coding_agent_telemetry_events WHERE session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(receipts, 0, "receipts outlived the session they belong to");

    let sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions WHERE session_id = $1")
            .bind(&session_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(sessions, 0);
    server.cleanup().await;
}
