mod common;

use chrono::{TimeZone, Utc};
use nasiko_types::{
    CODING_AGENT_EVENT_VERSION, CODING_AGENT_SESSION_TITLE_MAX_BYTES, CapturePolicy,
    CodingAgentEventV1, CodingAgentLlmCall, CodingAgentSession, CodingAgentSource,
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCall,
    CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
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
            title: None,
            parent: None,
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
                accounting: None,
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

/// Turn an ingested receipt back into one accepted before ingest wrote spend:
/// no `trace_usage` row and not yet materialized, so only the Tempo read-back
/// can complete it. The read-back exists for exactly these receipts now.
async fn as_legacy_receipt(server: &common::TestServer, user_id: Uuid, event_id: &str) {
    sqlx::query(
        "DELETE FROM trace_usage WHERE session_id IN \
           (SELECT session_id FROM coding_agent_telemetry_events \
             WHERE user_id = $1 AND event_id = $2)",
    )
    .bind(user_id)
    .bind(event_id)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE coding_agent_telemetry_events SET materialized_at = NULL \
          WHERE user_id = $1 AND event_id = $2",
    )
    .bind(user_id)
    .bind(event_id)
    .execute(&server.db)
    .await
    .unwrap();
}

/// Exercise the real exporter without exposing its internal payload builder.
async fn exported_trace(server: &common::TestServer) -> Value {
    use std::sync::{Arc, Mutex};
    let captured = Arc::new(Mutex::new(None::<Value>));
    let capture = captured.clone();
    let mut collector = mockito::Server::new_async().await;
    let traces = collector
        .mock("POST", "/v1/traces")
        .match_request(move |request| {
            *capture.lock().unwrap() =
                Some(serde_json::from_slice(request.body().unwrap()).unwrap());
            true
        })
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let logs = collector
        .mock("POST", "/v1/logs")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &collector.url())
        .await
        .unwrap();
    traces.assert_async().await;
    logs.assert_async().await;
    let mut payload = captured.lock().unwrap().clone().unwrap();
    // Tempo returns enum names rather than the collector's numeric input.
    for span in payload["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array_mut()
        .unwrap()
    {
        span["kind"] = if span["kind"] == 3 {
            "SPAN_KIND_CLIENT"
        } else {
            "SPAN_KIND_INTERNAL"
        }
        .into();
    }
    payload
}

#[tokio::test]
#[serial]
async fn late_receipt_retries_incomplete_trace_and_materializes_once() {
    use nasiko_server::observability::{
        receipt_materializer::materialize_once, session_resolver::PgSessionIdResolver,
    };
    let server = common::TestServer::start().await;
    let (user, _) = setup(&server).await;
    let mut source = event("late-receipt", "turn", CapturePolicy::Content);
    source.turn.llm_calls[0].provider = "unknown".into();
    post(&server, user, std::slice::from_ref(&source)).await;
    as_legacy_receipt(&server, user, &source.event_id).await;
    // Receipt time is new, but its trace predates the rolling search window.
    sqlx::query("UPDATE trace_usage_cursor SET high_water=now() WHERE id=1")
        .execute(&server.db)
        .await
        .unwrap();
    // Materialization state lives on the receipt row; neither repair-era table ships.
    for table in [
        "coding_accounting_revisions",
        "coding_receipt_materializations",
    ] {
        let present: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(table)
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert!(present.is_none(), "{table} should not exist");
    }
    let retired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger WHERE tgname='trigger_calculate_usage_cost'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(retired, 0);
    let payload = exported_trace(&server).await;
    let trace_id = payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        .as_str()
        .unwrap();
    let mut tempo = mockito::Server::new_async().await;
    let provider = nasiko_observability::TempoLokiProvider::new(
        tempo.url(),
        tempo.url(),
        std::sync::Arc::new(nasiko_pricing::PricingEngine::offline()),
    );
    let sessions = PgSessionIdResolver::new(server.db.clone());
    let mut incomplete = payload["resourceSpans"].clone();
    incomplete[0]["scopeSpans"][0]["spans"]
        .as_array_mut()
        .unwrap()
        .pop();
    let partial = tempo
        .mock("GET", format!("/api/traces/{trace_id}").as_str())
        .with_status(200)
        .with_body(json!({"batches":incomplete}).to_string())
        .create_async()
        .await;
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        0
    );
    let state: (bool, bool) = sqlx::query_as("SELECT materialized_at IS NULL,materialize_last_error IS NOT NULL FROM coding_agent_telemetry_events WHERE user_id=$1 AND event_id=$2")
        .bind(user).bind(&source.event_id).fetch_one(&server.db).await.unwrap();
    assert_eq!(state, (true, true));
    partial.remove_async().await;
    sqlx::query("UPDATE coding_agent_telemetry_events SET materialize_next_attempt_at=now() WHERE user_id=$1 AND event_id=$2")
        .bind(user).bind(&source.event_id).execute(&server.db).await.unwrap();
    let complete = tempo
        .mock("GET", format!("/api/traces/{trace_id}").as_str())
        .with_status(200)
        .with_body(json!({"batches":payload["resourceSpans"]}).to_string())
        .expect(2)
        .create_async()
        .await;
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        0
    );
    complete.assert_async().await;
    let usage: (i64,i64,i64,i64,bool) = sqlx::query_as("SELECT input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,cost_estimated FROM trace_usage WHERE trace_id=$1")
        .bind(trace_id).fetch_one(&server.db).await.unwrap();
    assert_eq!(usage, (10, 5, 2, 3, true));
    let dashboard: Value = common::as_member(server.client.get(server.url("/api/observability/finops/dashboard?start_time=2023-01-01T00%3A00%3A00Z&end_time=2024-01-01T00%3A00%3A00Z")), &user.to_string(), "admin")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(dashboard["data"]["token_usage"]["total_tokens"], 20);
    assert_eq!(dashboard["data"]["token_usage"]["cache_read_tokens"], 2);
    assert_eq!(dashboard["data"]["token_usage"]["cache_creation_tokens"], 3);
    assert_eq!(
        dashboard["data"]["summary"]["estimated_cost"],
        dashboard["data"]["summary"]["total_cost"]
    );
    assert_eq!(dashboard["data"]["summary"]["unknown_confidence_calls"], 0);
    let state: (bool,bool) = sqlx::query_as("SELECT materialized_at IS NOT NULL,materialize_last_error IS NULL FROM coding_agent_telemetry_events WHERE user_id=$1 AND event_id=$2")
        .bind(user).bind(&source.event_id).fetch_one(&server.db).await.unwrap();
    assert_eq!(state, (true, true));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tokenless_receipt_completes_without_fabricated_usage() {
    use nasiko_server::observability::{
        receipt_materializer::materialize_once, session_resolver::PgSessionIdResolver,
    };
    let server = common::TestServer::start().await;
    let (user, _) = setup(&server).await;
    let mut source = event("tokenless-receipt", "turn", CapturePolicy::Content);
    source.turn.llm_calls.clear();
    post(&server, user, std::slice::from_ref(&source)).await;
    as_legacy_receipt(&server, user, &source.event_id).await;
    let payload = exported_trace(&server).await;
    let trace_id = payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        .as_str()
        .unwrap();
    let mut tempo = mockito::Server::new_async().await;
    let complete = tempo
        .mock("GET", format!("/api/traces/{trace_id}").as_str())
        .with_status(200)
        .with_body(json!({"batches":payload["resourceSpans"]}).to_string())
        .expect(2)
        .create_async()
        .await;
    let provider = nasiko_observability::TempoLokiProvider::new(
        tempo.url(),
        tempo.url(),
        std::sync::Arc::new(nasiko_pricing::PricingEngine::offline()),
    );
    let sessions = PgSessionIdResolver::new(server.db.clone());
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        0
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trace_usage")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    complete.assert_async().await;
    server.cleanup().await;
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

    let usage: (i32, i32, i32, i32, Option<rust_decimal::Decimal>, Option<bool>) = sqlx::query_as(
        "SELECT input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, usage_estimated \
         FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(&content_session_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((usage.0, usage.1, usage.2, usage.3), (10, 5, 2, 3));
    assert!(usage.4.unwrap() > rust_decimal::Decimal::ZERO);
    assert_eq!(usage.5, Some(true));

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

async fn session_title(server: &common::TestServer, agent_id: Uuid) -> String {
    sqlx::query_scalar("SELECT title FROM chat_sessions WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .expect("session exists")
}

#[tokio::test]
#[serial]
async fn inserts_exact_external_title_and_preserves_it_on_later_turns() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let title = format!(
        "  {}  ",
        "é".repeat((CODING_AGENT_SESSION_TITLE_MAX_BYTES - 4) / 2)
    );
    let mut first = event("titled", "turn-1", CapturePolicy::Content);
    first.session.title = Some(title.clone());
    assert_eq!(
        post(&server, user_id, &[first]).await["data"]["results"][0]["status"],
        "accepted"
    );
    assert_eq!(session_title(&server, agent_id).await, title);

    let mut later = event("titled", "turn-2", CapturePolicy::Content);
    later.session.title = Some("Replacement title".into());
    let missing = event("titled", "turn-3", CapturePolicy::Content);
    let result = post(&server, user_id, &[later, missing]).await;
    assert_eq!(result["data"]["results"][0]["status"], "accepted");
    assert_eq!(result["data"]["results"][1]["status"], "accepted");
    assert_eq!(session_title(&server, agent_id).await, title);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn upgrades_only_the_coding_session_placeholder_on_a_new_event() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let metadata = event("placeholder", "turn-1", CapturePolicy::MetadataOnly);
    let missing = event("placeholder", "turn-2", CapturePolicy::Content);
    let result = post(&server, user_id, &[metadata, missing]).await;
    assert_eq!(result["data"]["results"][0]["status"], "accepted");
    assert_eq!(result["data"]["results"][1]["status"], "accepted");
    assert_eq!(session_title(&server, agent_id).await, "Coding session");

    let mut titled = event("placeholder", "turn-3", CapturePolicy::Content);
    titled.session.title = Some("Native title".into());
    assert_eq!(
        post(&server, user_id, &[titled]).await["data"]["results"][0]["status"],
        "accepted"
    );
    assert_eq!(session_title(&server, agent_id).await, "Native title");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn title_replays_cannot_rename_a_session() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let original = event("replay-title", "turn-1", CapturePolicy::Content);
    assert_eq!(
        post(&server, user_id, std::slice::from_ref(&original)).await["data"]["results"][0]["status"],
        "accepted"
    );
    assert_eq!(
        post(&server, user_id, std::slice::from_ref(&original)).await["data"]["results"][0]["status"],
        "duplicate"
    );

    let mut changed = original;
    changed.session.title = Some("Replay must not upgrade".into());
    let rejected = post(&server, user_id, &[changed]).await;
    assert_eq!(rejected["data"]["results"][0]["status"], "rejected");
    assert_eq!(
        rejected["data"]["results"][0]["error"],
        "event_id already exists with a different payload"
    );
    assert_eq!(session_title(&server, agent_id).await, "Coding session");

    let mut titled = event("replay-title", "turn-2", CapturePolicy::Content);
    titled.session.title = Some("Accepted title".into());
    assert_eq!(
        post(&server, user_id, std::slice::from_ref(&titled)).await["data"]["results"][0]["status"],
        "accepted"
    );
    // A replay cannot restore a title even if the session is now a placeholder.
    sqlx::query("UPDATE chat_sessions SET title = 'Coding session' WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .expect("reset session title");
    assert_eq!(
        post(&server, user_id, &[titled]).await["data"]["results"][0]["status"],
        "duplicate"
    );
    assert_eq!(session_title(&server, agent_id).await, "Coding session");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn conflicting_content_rolls_back_title_upgrade_and_receipt() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let metadata = event("rollback-title", "turn-1", CapturePolicy::MetadataOnly);
    assert_eq!(
        post(&server, user_id, &[metadata]).await["data"]["results"][0]["status"],
        "accepted"
    );
    // Simulate a pre-existing incomplete external turn, which rejects after receipt insertion.
    sqlx::query(
        "INSERT INTO chat_messages (session_id, external_turn_id, role, content) \
         SELECT session_id, 'turn-2', 'user', 'existing question' FROM chat_sessions WHERE agent_id = $1",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .expect("insert incomplete turn");
    let mut titled = event("rollback-title", "turn-2", CapturePolicy::Content);
    titled.session.title = Some("Must roll back".into());
    let result = post(&server, user_id, std::slice::from_ref(&titled)).await;
    assert_eq!(result["data"]["results"][0]["status"], "rejected");
    assert_eq!(
        result["data"]["results"][0]["error"],
        "external turn is incomplete"
    );
    assert_eq!(session_title(&server, agent_id).await, "Coding session");
    let receipt_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2)",
    )
    .bind(user_id)
    .bind(&titled.event_id)
    .fetch_one(&server.db)
    .await
    .expect("query rolled back receipt");
    assert!(!receipt_exists);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn title_upgrade_cannot_bypass_session_ownership_check() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    let metadata = event("ownership-title", "turn-1", CapturePolicy::MetadataOnly);
    assert_eq!(
        post(&server, user_id, &[metadata]).await["data"]["results"][0]["status"],
        "accepted"
    );
    let other_user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, 'other', 'other@test.local')")
        .bind(other_user)
        .execute(&server.db)
        .await
        .expect("create other owner");
    sqlx::query("UPDATE chat_sessions SET user_id = $2 WHERE agent_id = $1")
        .bind(agent_id)
        .bind(other_user)
        .execute(&server.db)
        .await
        .expect("change session owner");
    let mut titled = event("ownership-title", "turn-2", CapturePolicy::Content);
    titled.session.title = Some("Unauthorized title".into());
    let result = post(&server, user_id, &[titled]).await;
    assert_eq!(result["data"]["results"][0]["status"], "rejected");
    assert_eq!(
        result["data"]["results"][0]["error"],
        "session already belongs to a different user or agent"
    );
    assert_eq!(session_title(&server, agent_id).await, "Coding session");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn invalid_titles_are_rejected_before_creating_sessions() {
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    for (index, title) in [
        " \n".into(),
        "title\0nul".into(),
        "x".repeat(CODING_AGENT_SESSION_TITLE_MAX_BYTES + 1),
    ]
    .into_iter()
    .enumerate()
    {
        let mut invalid = event(
            "invalid-title",
            &format!("turn-{index}"),
            CapturePolicy::Content,
        );
        invalid.session.title = Some(title);
        assert_eq!(
            post(&server, user_id, &[invalid]).await["data"]["results"][0]["status"],
            "rejected"
        );
    }
    let mut private = event("private-title", "turn", CapturePolicy::MetadataOnly);
    private.session.title = Some("Private content".into());
    assert_eq!(
        post(&server, user_id, &[private]).await["data"]["results"][0]["status"],
        "rejected"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .expect("count sessions");
    assert_eq!(count, 0);
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
    assert!(
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .is_none()
    );
    let message = response.text().await.unwrap();
    assert!(
        message.contains("8 MiB batch limit") && message.contains("split the batch"),
        "{message}"
    );
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

// ─── session/list for coding-agent sessions ──────────────────────────────────
//
// A coding-agent session has no `session_traces` or `flows` rows and its
// `trace_usage` lags behind a Tempo backlog, so `session/list` totals it from
// the receipts. None of these tests runs the materializer: the numbers must be
// right before it has caught up.

/// The window the fixed-time fixtures fall in; the list defaults to 7 days.
const LIST_SINCE: &str = "?limit=100&start_time=2020-01-01T00:00:00Z";

/// Turn `index` of a session: starts `index` minutes after the base time and
/// lasts 10 seconds, with the fixture's 20 tokens.
fn turn_at(session: &str, index: i64, tools: Vec<CodingAgentToolCallStatus>) -> CodingAgentEventV1 {
    let mut turn = event(session, &format!("turn-{index}"), CapturePolicy::Content);
    let started_at = Utc.timestamp_opt(1_700_000_000 + index * 60, 0).unwrap();
    let ended_at = started_at + chrono::Duration::seconds(10);
    turn.captured_at = ended_at;
    turn.turn.started_at = started_at;
    turn.turn.ended_at = ended_at;
    for call in &mut turn.turn.llm_calls {
        call.started_at = started_at;
        call.ended_at = ended_at;
    }
    turn.turn.tool_calls = tools
        .into_iter()
        .enumerate()
        .map(|(i, status)| CodingAgentToolCall {
            id: format!("tool-{index}-{i}"),
            name: "bash".into(),
            kind: "shell".into(),
            model_call_id: None,
            status,
            arguments: None,
            output: None,
            raw: None,
            error: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            association: CodingAgentToolAssociation::Turn,
            timestamp_quality: CodingAgentTimestampQuality::Unknown,
        })
        .collect();
    turn
}

async fn ingest_session(
    server: &common::TestServer,
    user_id: Uuid,
    session: &str,
    turns: Vec<CodingAgentEventV1>,
) -> String {
    let response = post(server, user_id, &turns).await;
    for result in response["data"]["results"].as_array().unwrap() {
        assert_eq!(result["status"], "accepted", "{response}");
    }
    sqlx::query_scalar(
        "SELECT DISTINCT session_id FROM coding_agent_telemetry_events \
          WHERE user_id = $1 AND source_session_id = $2",
    )
    .bind(user_id)
    .bind(session)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn list_sessions(server: &common::TestServer, user_id: Uuid, query: &str) -> Value {
    let res = server
        .client
        .get(server.url(&format!("/api/observability/session/list{query}")))
        .bearer_auth(common::sign_token(
            &user_id.to_string(),
            "admin",
            true,
            "admin",
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

fn session_ids(body: &Value) -> Vec<String> {
    body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap().to_owned())
        .collect()
}

fn find<'a>(body: &'a Value, session_id: &str) -> &'a Value {
    body["data"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == session_id)
        .unwrap_or_else(|| panic!("{session_id} not listed: {body}"))
}

#[tokio::test]
#[serial]
async fn session_list_totals_a_coding_session_from_its_turns() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let turns = (0..3).map(|i| turn_at("three-turns", i, vec![])).collect();
    let session = ingest_session(&server, user_id, "three-turns", turns).await;
    // Pin the per-turn cost so the sum does not depend on the pricing table.
    sqlx::query(
        "UPDATE chat_messages SET cost_usd = 0.25 WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(&session)
    .execute(&server.db)
    .await
    .unwrap();

    let body = list_sessions(&server, user_id, LIST_SINCE).await;
    let row = find(&body, &session);
    assert_eq!(row["num_traces"], 3, "one trace per turn");
    assert_eq!(row["token_usage"]["total"], 60, "3 turns x 20 tokens");
    assert_eq!(row["cost_summary"]["total"]["cost"], 0.75);
    // First turn's start to the last turn's end: 2 minutes + 10 seconds.
    assert_eq!(row["duration_ms"], 130_000);
    assert_eq!(row["start_time"], "2023-11-14T22:13:20.000Z");
    assert_eq!(row["status"], "ok");
    assert_eq!(row["failed_flows"], 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_list_fails_a_coding_session_with_an_erroring_tool_call() {
    use CodingAgentToolCallStatus::*;
    let server = common::TestServer::start().await;
    let (user_id, agent_id) = setup(&server).await;
    // Denied and cancelled calls are the user's decision, not a failure.
    let ok = ingest_session(
        &server,
        user_id,
        "declined-tools",
        vec![turn_at(
            "declined-tools",
            0,
            vec![Succeeded, Denied, Cancelled],
        )],
    )
    .await;
    let failed = ingest_session(
        &server,
        user_id,
        "broken-tool",
        vec![
            turn_at("broken-tool", 0, vec![Succeeded]),
            turn_at("broken-tool", 1, vec![TimedOut]),
        ],
    )
    .await;
    // An ordinary chat session beside them keeps its flow-derived status and
    // its absent trace count.
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title, created_at) \
         VALUES ('plain-chat', $1, $2, 'chat', '2023-11-14T00:00:00Z')",
    )
    .bind(user_id)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, status, session_id) \
         VALUES ('plain-flow', $1, 'completed', 'plain-chat')",
    )
    .bind(user_id)
    .execute(&server.db)
    .await
    .unwrap();

    let body = list_sessions(&server, user_id, LIST_SINCE).await;
    assert_eq!(find(&body, &ok)["status"], "ok");
    assert_eq!(find(&body, &failed)["status"], "failed");
    assert_eq!(find(&body, &failed)["num_traces"], 2);
    let plain = find(&body, "plain-chat");
    assert_eq!(plain["status"], "ok");
    assert!(plain["num_traces"].is_null(), "{plain}");
    assert!(plain["token_usage"]["total"].is_null(), "{plain}");
    assert!(plain["cost_summary"]["total"]["cost"].is_null(), "{plain}");

    // The filters agree with the badges, and the counts are exact.
    let failed_page = list_sessions(&server, user_id, &format!("{LIST_SINCE}&status=failed")).await;
    assert_eq!(session_ids(&failed_page), std::slice::from_ref(&failed));
    assert_eq!(failed_page["data"]["matching_count"], 1);
    let mut ok_ids =
        session_ids(&list_sessions(&server, user_id, &format!("{LIST_SINCE}&status=ok")).await);
    ok_ids.sort();
    let mut expected = vec![ok, "plain-chat".to_owned()];
    expected.sort();
    assert_eq!(ok_ids, expected);
    let running = list_sessions(&server, user_id, &format!("{LIST_SINCE}&status=running")).await;
    assert!(session_ids(&running).is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn ingest_stores_the_turn_summary_beside_the_receipt() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let turn = turn_at(
        "summary",
        1,
        vec![
            CodingAgentToolCallStatus::Failed,
            CodingAgentToolCallStatus::Succeeded,
        ],
    );
    ingest_session(&server, user_id, "summary", vec![turn.clone()]).await;

    let (started, ended, tokens, errors): (chrono::DateTime<Utc>, chrono::DateTime<Utc>, i64, i32) =
        sqlx::query_as(
            "SELECT turn_started_at, turn_ended_at, turn_tokens, turn_tool_errors \
           FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
        )
        .bind(user_id)
        .bind(&turn.event_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        (started, ended, tokens, errors),
        (turn.turn.started_at, turn.turn.ended_at, 20, 1)
    );

    server.cleanup().await;
}

// ─── Ingest at scale: spend at ingest, protection, ordering, crash safety ────

const BATCH_URL: &str = "/api/telemetry/coding-agent/events/batch";

async fn post_raw(
    server: &common::TestServer,
    user_id: Uuid,
    events: &[CodingAgentEventV1],
) -> reqwest::Response {
    common::as_member(
        server.client.post(server.url(BATCH_URL)),
        &user_id.to_string(),
        "admin",
    )
    .json(&json!({"events": events}))
    .send()
    .await
    .unwrap()
}

/// A second user with their own registration of the same coding agent.
async fn second_user(server: &common::TestServer) -> Uuid {
    let user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, 'second', 'second@test.local')",
    )
    .bind(user)
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('coding-agent', $1, 'claude')",
    )
    .bind(user)
    .execute(&server.db)
    .await
    .unwrap();
    user
}

/// A metadata-only turn that ended `age` ago.
fn turn_ended_ago(session: &str, turn: &str, age: chrono::Duration) -> CodingAgentEventV1 {
    let mut event = event(session, turn, CapturePolicy::MetadataOnly);
    let ended_at = Utc::now() - age;
    let started_at = ended_at - chrono::Duration::seconds(2);
    event.captured_at = ended_at;
    event.turn.started_at = started_at;
    event.turn.ended_at = ended_at;
    for call in &mut event.turn.llm_calls {
        call.started_at = started_at;
        call.ended_at = ended_at;
    }
    event
}

async fn count(server: &common::TestServer, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(&server.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn spend_is_written_at_ingest_and_every_usage_view_reflects_it_without_read_back() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let turns: Vec<CodingAgentEventV1> = (0..5)
        .map(|i| {
            let session = if i < 3 { "spend-a" } else { "spend-b" };
            let policy = if i % 2 == 0 {
                CapturePolicy::Content
            } else {
                CapturePolicy::MetadataOnly
            };
            let mut turn = event(session, &format!("turn-{i}"), policy);
            turn.turn.llm_calls[0].input_tokens = 100 * (i + 1);
            turn
        })
        .collect();
    let first = post(&server, user_id, &turns).await;
    let replay = post(&server, user_id, &turns).await;
    for i in 0..turns.len() {
        assert_eq!(first["data"]["results"][i]["status"], "accepted", "{first}");
        assert_eq!(
            replay["data"]["results"][i]["status"], "duplicate",
            "{replay}"
        );
    }

    // One row per turn, however often it is sent, and no receipt left for the read-back.
    assert_eq!(count(&server, "SELECT count(*) FROM trace_usage").await, 5);
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events WHERE materialized_at IS NULL"
        )
        .await,
        0
    );
    // Fixture turns are from 2023: history, not live.
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events WHERE otlp_historical"
        )
        .await,
        5
    );
    let (tokens, cost): (i64, f64) = sqlx::query_as(
        "SELECT sum(input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)::bigint, \
                sum(cost_usd) FROM trace_usage",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    // input 100+200+...+500, plus 5 + 2 + 3 of the other classes per turn.
    assert_eq!(tokens, 1500 + 5 * 10);

    let window = "start_time=2023-01-01T00%3A00%3A00Z&end_time=2024-01-01T00%3A00%3A00Z";
    let dashboard: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/observability/finops/dashboard?{window}"))),
        &user_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(dashboard["data"]["token_usage"]["total_tokens"], tokens);
    let dashboard_cost = dashboard["data"]["summary"]["total_cost"].as_f64().unwrap();
    assert!((dashboard_cost - cost).abs() < 1e-9, "{dashboard}");

    let usage: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/observability/coding-agents/usage?{window}"))),
        &user_id.to_string(),
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let totals = &usage["data"]["totals"];
    assert_eq!(totals["turns"], 5, "{usage}");
    assert_eq!(totals["sessions"], 2, "{usage}");
    assert_eq!(totals["tokens"], tokens, "{usage}");
    assert!((totals["cost_usd"].as_f64().unwrap() - cost).abs() < 1e-9);
    server.cleanup().await;
}

/// The row ingest writes is the row the Tempo read-back derives from the
/// exported trace — so a later read-back of the same turn replaces it with the
/// same values instead of adding a second row.
#[tokio::test]
#[serial]
async fn tempo_read_back_reproduces_the_row_ingest_wrote() {
    use nasiko_server::observability::{
        receipt_materializer::materialize_once, session_resolver::PgSessionIdResolver,
    };
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let mut source = turn_at(
        "parity",
        1,
        vec![
            CodingAgentToolCallStatus::Failed,
            CodingAgentToolCallStatus::Succeeded,
            CodingAgentToolCallStatus::Denied,
        ],
    );
    let mut second_call = source.turn.llm_calls[0].clone();
    second_call.id = "call-2".into();
    second_call.model = "claude-other".into();
    second_call.output_tokens = 40;
    source.turn.llm_calls.push(second_call);
    // A failed `read` nothing retried and a denied `edit` count; the failed
    // `bash` is recovered by the successful `bash` after it.
    source.turn.tool_calls[0].name = "read".into();
    source.turn.tool_calls[2].name = "edit".into();
    source.turn.tool_calls.insert(
        0,
        CodingAgentToolCall {
            id: "bash-failed".into(),
            status: CodingAgentToolCallStatus::Failed,
            ..source.turn.tool_calls[1].clone()
        },
    );
    post(&server, user_id, std::slice::from_ref(&source)).await;

    const ROW: &str = "SELECT trace_id, session_id, agent_id, user_id, model, provider, \
         input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, tool_call_count, \
         cost_usd, prompt_cost_usd, completion_cost_usd, cost_estimated, latency_ms, started_at, \
         error_count FROM trace_usage";
    #[derive(Debug, PartialEq, sqlx::FromRow)]
    struct Row {
        trace_id: String,
        session_id: Option<String>,
        agent_id: Option<Uuid>,
        user_id: Option<Uuid>,
        model: Option<String>,
        provider: Option<String>,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
        tool_call_count: i64,
        cost_usd: f64,
        prompt_cost_usd: f64,
        completion_cost_usd: f64,
        cost_estimated: Option<bool>,
        latency_ms: Option<i64>,
        started_at: chrono::DateTime<Utc>,
        error_count: Option<i32>,
    }
    let at_ingest: Row = sqlx::query_as(ROW).fetch_one(&server.db).await.unwrap();

    let payload = exported_trace(&server).await;
    sqlx::query("UPDATE coding_agent_telemetry_events SET materialized_at = NULL")
        .execute(&server.db)
        .await
        .unwrap();
    let mut tempo = mockito::Server::new_async().await;
    tempo
        .mock(
            "GET",
            format!("/api/traces/{}", at_ingest.trace_id).as_str(),
        )
        .with_status(200)
        .with_body(json!({"batches": payload["resourceSpans"]}).to_string())
        .expect(2)
        .create_async()
        .await;
    let provider = nasiko_observability::TempoLokiProvider::new(
        tempo.url(),
        tempo.url(),
        std::sync::Arc::new(nasiko_pricing::PricingEngine::new(server.db.clone())),
    );
    let sessions = PgSessionIdResolver::new(server.db.clone());
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        1
    );

    assert_eq!(count(&server, "SELECT count(*) FROM trace_usage").await, 1);
    let read_back: Row = sqlx::query_as(ROW).fetch_one(&server.db).await.unwrap();
    assert_eq!(read_back, at_ingest);
    // Mixed models collapse to none; the denied call and the unretried failure count.
    assert_eq!(at_ingest.model, None);
    assert_eq!(at_ingest.error_count, Some(2));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn turns_older_than_tempo_accepts_skip_export_and_read_back() {
    let server = common::TestServer::start_with(|config| {
        config.coding_agent_ingest = nasiko_config::CodingAgentIngestConfig::default();
    })
    .await;
    let (user_id, _) = setup(&server).await;
    let old = event("too-old", "turn", CapturePolicy::MetadataOnly);
    let live = turn_ended_ago("live", "turn", chrono::Duration::seconds(5));
    let response = post(&server, user_id, &[old.clone(), live.clone()]).await;
    assert_eq!(response["data"]["results"][0]["status"], "accepted");
    assert_eq!(response["data"]["results"][1]["status"], "accepted");

    let state = |event_id: String| {
        let db = server.db.clone();
        async move {
            sqlx::query_as::<_, (String, bool, bool)>(
                "SELECT otlp_state, otlp_historical, materialized_at IS NOT NULL \
                   FROM coding_agent_telemetry_events WHERE event_id = $1",
            )
            .bind(event_id)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        state(old.event_id.clone()).await,
        ("skipped".into(), true, true)
    );
    assert_eq!(
        state(live.event_id.clone()).await,
        ("pending".into(), false, true)
    );
    // Its spend is in Postgres all the same.
    assert_eq!(count(&server, "SELECT count(*) FROM trace_usage").await, 2);

    let mut collector = mockito::Server::new_async().await;
    let traces = collector
        .mock("POST", "/v1/traces")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    let logs = collector
        .mock("POST", "/v1/logs")
        .with_status(200)
        .expect(1)
        .create_async()
        .await;
    for _ in 0..2 {
        nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &collector.url())
            .await
            .unwrap();
    }
    traces.assert_async().await;
    logs.assert_async().await;
    assert_eq!(state(old.event_id).await.0, "skipped");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn batches_over_one_hundred_events_are_413_with_a_message() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let events: Vec<CodingAgentEventV1> = (0..101)
        .map(|i| event("big", &format!("turn-{i}"), CapturePolicy::MetadataOnly))
        .collect();
    let response = post_raw(&server, user_id, &events).await;
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    let message = response.text().await.unwrap();
    assert!(
        message.contains("101 events") && message.contains("at most 100"),
        "{message}"
    );
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events"
        )
        .await,
        0
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn per_user_event_budget_answers_429_with_retry_after() {
    let server = common::TestServer::start_with(|config| {
        config.coding_agent_ingest.events_per_min = 150;
    })
    .await;
    let (user_id, _) = setup(&server).await;
    let other = second_user(&server).await;
    let batch = |prefix: &str| -> Vec<CodingAgentEventV1> {
        (0..100)
            .map(|i| event(prefix, &format!("turn-{i}"), CapturePolicy::MetadataOnly))
            .collect()
    };

    assert_eq!(
        post_raw(&server, user_id, &batch("one")).await.status(),
        200
    );
    // 100 more would make 200 events this minute: the whole batch is turned away.
    let limited = post_raw(&server, user_id, &batch("two")).await;
    assert_eq!(limited.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = limited.headers()[reqwest::header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry_after));
    let message = limited.text().await.unwrap();
    assert!(message.contains("150 turns per minute"), "{message}");
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events"
        )
        .await,
        100
    );
    // The budget is per user.
    assert_eq!(post_raw(&server, other, &batch("one")).await.status(), 200);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn ingest_at_capacity_answers_503_with_retry_after() {
    let server = common::TestServer::start_with(|config| {
        config.coding_agent_ingest.max_concurrent = 1;
    })
    .await;
    let (user_id, _) = setup(&server).await;
    // The first batch sends half its body and stalls. The permit is taken
    // before the body is read, so it holds the only one while the server waits
    // for the rest — without holding a database connection.
    let first = event("held", "turn", CapturePolicy::MetadataOnly);
    let body = serde_json::to_vec(&json!({ "events": [first] })).unwrap();
    let (half, rest) = body.split_at(body.len() / 2);
    let (chunks, chunk_rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    chunks.send(Ok(half.to_vec())).await.unwrap();
    let held = {
        let request = common::as_member(
            server.client.post(server.url(BATCH_URL)),
            &user_id.to_string(),
            "admin",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(reqwest::Body::wrap_stream(futures::stream::unfold(
            chunk_rx,
            |mut rx| async move { rx.recv().await.map(|chunk| (chunk, rx)) },
        )));
        tokio::spawn(async move { request.send().await.unwrap().status() })
    };

    // An empty batch is a 400 once it is past the valve; wait for the valve.
    let mut turned_away = None;
    for _ in 0..100 {
        let response = common::as_member(
            server.client.post(server.url(BATCH_URL)),
            &user_id.to_string(),
            "admin",
        )
        .json(&json!({ "events": [] }))
        .send()
        .await
        .unwrap();
        if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            turned_away = Some(response);
            break;
        }
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let turned_away =
        turned_away.expect("a second batch is turned away while the first holds the permit");
    let retry_after: u64 = turned_away.headers()[reqwest::header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((2..=5).contains(&retry_after));
    assert!(turned_away.text().await.unwrap().contains("at capacity"));

    chunks.send(Ok(rest.to_vec())).await.unwrap();
    drop(chunks);
    assert_eq!(held.await.unwrap(), reqwest::StatusCode::OK);
    // The permit is back: the turned-away batch goes through on retry.
    let retried = post(
        &server,
        user_id,
        &[event("second", "turn", CapturePolicy::MetadataOnly)],
    )
    .await;
    assert_eq!(retried["data"]["results"][0]["status"], "accepted");
    server.cleanup().await;
}

/// A batch is acknowledged only after its turns commit: a failure before a
/// turn's commit answers 5xx and leaves nothing of that turn behind, and the
/// client's resend completes it exactly once.
#[tokio::test]
#[serial]
async fn batch_is_acknowledged_only_after_commit() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    // Fails the last write of the second turn's transaction, after its receipt,
    // session and messages are already written inside it.
    sqlx::raw_sql(
        "CREATE FUNCTION fail_marked_usage() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.input_tokens = 999 THEN RAISE EXCEPTION 'injected before commit'; END IF; \
         RETURN NEW; END $$; \
         CREATE TRIGGER fail_marked_usage BEFORE INSERT ON trace_usage \
         FOR EACH ROW EXECUTE FUNCTION fail_marked_usage();",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let committed = event("commit-a", "turn", CapturePolicy::Content);
    let mut failing = event("commit-b", "turn", CapturePolicy::Content);
    failing.turn.llm_calls[0].input_tokens = 999;
    let batch = [committed.clone(), failing.clone()];

    let response = post_raw(&server, user_id, &batch).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    let receipts: Vec<String> =
        sqlx::query_scalar("SELECT event_id FROM coding_agent_telemetry_events")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(receipts, vec![committed.event_id.clone()]);
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM chat_sessions WHERE title = 'Coding session'"
        )
        .await,
        1,
        "the failed turn's new session must roll back with it"
    );
    assert_eq!(count(&server, "SELECT count(*) FROM trace_usage").await, 1);

    sqlx::raw_sql("DROP TRIGGER fail_marked_usage ON trace_usage")
        .execute(&server.db)
        .await
        .unwrap();
    let resent = post(&server, user_id, &batch).await;
    assert_eq!(resent["data"]["results"][0]["status"], "duplicate");
    assert_eq!(resent["data"]["results"][1]["status"], "accepted");
    assert_eq!(count(&server, "SELECT count(*) FROM trace_usage").await, 2);
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events"
        )
        .await,
        2
    );
    server.cleanup().await;
}

/// The read-back holds no claim between passes: a receipt a crashed server left
/// half done (a usage row written, the receipt not yet marked) is completed by
/// the next server's first pass, and stays one row.
#[tokio::test]
#[serial]
async fn read_back_resumes_a_receipt_a_restart_left_unfinished() {
    use nasiko_server::observability::{
        receipt_materializer::materialize_once, session_resolver::PgSessionIdResolver,
    };
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let source = event("restart", "turn", CapturePolicy::MetadataOnly);
    post(&server, user_id, std::slice::from_ref(&source)).await;
    let payload = exported_trace(&server).await;
    let trace_id = payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        .as_str()
        .unwrap()
        .to_owned();
    as_legacy_receipt(&server, user_id, &source.event_id).await;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, input_tokens, started_at) \
         VALUES ($1, 'coding-agent', 1, now())",
    )
    .bind(&trace_id)
    .execute(&server.db)
    .await
    .unwrap();

    let mut tempo = mockito::Server::new_async().await;
    tempo
        .mock("GET", format!("/api/traces/{trace_id}").as_str())
        .with_status(200)
        .with_body(json!({"batches": payload["resourceSpans"]}).to_string())
        .create_async()
        .await;
    let provider = nasiko_observability::TempoLokiProvider::new(
        tempo.url(),
        tempo.url(),
        std::sync::Arc::new(nasiko_pricing::PricingEngine::offline()),
    );
    let sessions = PgSessionIdResolver::new(server.db.clone());
    assert_eq!(
        materialize_once(&server.db, &provider, &sessions)
            .await
            .unwrap(),
        1
    );
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT input_tokens, output_tokens FROM trace_usage")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(rows, vec![(10, 5)]);
    assert_eq!(
        count(
            &server,
            "SELECT count(*) FROM coding_agent_telemetry_events WHERE materialized_at IS NULL"
        )
        .await,
        0
    );
    server.cleanup().await;
}

/// Live turns are exported before any history, and history is taken
/// round-robin across users, so one user's import never starves another's.
#[tokio::test]
#[serial]
async fn outbox_claims_live_turns_first_then_rotates_users() {
    let server = common::TestServer::start().await;
    let (importer, _) = setup(&server).await;
    let other = second_user(&server).await;
    let history = |prefix: &str, n: usize| -> Vec<CodingAgentEventV1> {
        (0..n)
            .map(|i| event(prefix, &format!("turn-{i}"), CapturePolicy::MetadataOnly))
            .collect()
    };
    // One request per turn so each has its own `received_at`, in order.
    for turn in history("import", 6) {
        post(&server, importer, std::slice::from_ref(&turn)).await;
    }
    for turn in history("other-history", 3) {
        post(&server, other, std::slice::from_ref(&turn)).await;
    }
    let live = turn_ended_ago("other-live", "turn", chrono::Duration::seconds(1));
    post(&server, other, std::slice::from_ref(&live)).await;

    let mut collector = mockito::Server::new_async().await;
    collector
        .mock("POST", "/v1/traces")
        .with_status(503)
        .expect(8)
        .create_async()
        .await;
    nasiko_server::coding_agent_otlp::export_once(&server.db, &server.client, &collector.url())
        .await
        .unwrap();

    // Claimed rows were attempted once; the rest were never claimed.
    let claimed: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, source_session_id FROM coding_agent_telemetry_events \
          WHERE otlp_attempts = 1 ORDER BY received_at",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(claimed.len(), 8);
    assert!(claimed.contains(&(other, "other-live".into())));
    let by = |user: Uuid| claimed.iter().filter(|(u, _)| *u == user).count();
    // Live turn + all 3 of the other user's history, then the importer's oldest 4.
    assert_eq!(by(other), 4);
    assert_eq!(by(importer), 4);
    let importer_unclaimed: Vec<String> = sqlx::query_scalar(
        "SELECT turn_id FROM coding_agent_telemetry_events \
          WHERE user_id = $1 AND otlp_attempts = 0 ORDER BY turn_id",
    )
    .bind(importer)
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(importer_unclaimed, vec!["turn-4", "turn-5"]);
    server.cleanup().await;
}

/// Receipts from before the typed turn columns are summarized by the server in
/// bounded chunks, with the rules ingest uses, until none are left.
#[tokio::test]
#[serial]
async fn turn_summary_backfill_summarizes_old_receipts_in_chunks() {
    use nasiko_server::coding_agent_turn_summary_backfill::backfill_chunk;
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    let turns: Vec<CodingAgentEventV1> = (0..3)
        .map(|i| {
            turn_at(
                "backfill",
                i,
                vec![
                    CodingAgentToolCallStatus::TimedOut,
                    CodingAgentToolCallStatus::Denied,
                ],
            )
        })
        .collect();
    ingest_session(&server, user_id, "backfill", turns).await;
    const SUMMARY: &str = "SELECT event_id, turn_started_at, turn_ended_at, turn_tokens, \
         turn_tool_errors FROM coding_agent_telemetry_events ORDER BY event_id";
    type Summary = (
        String,
        Option<chrono::DateTime<Utc>>,
        Option<chrono::DateTime<Utc>>,
        Option<i64>,
        Option<i32>,
    );
    let at_ingest: Vec<Summary> = sqlx::query_as(SUMMARY).fetch_all(&server.db).await.unwrap();
    // As an older binary left them.
    sqlx::query(
        "UPDATE coding_agent_telemetry_events SET turn_started_at = NULL, turn_ended_at = NULL, \
                turn_tokens = NULL, turn_tool_errors = NULL",
    )
    .execute(&server.db)
    .await
    .unwrap();

    while backfill_chunk(&server.db).await.unwrap() > 0 {}
    let backfilled: Vec<Summary> = sqlx::query_as(SUMMARY).fetch_all(&server.db).await.unwrap();
    assert_eq!(backfilled, at_ingest);
    assert_eq!(at_ingest[0].3, Some(20));
    assert_eq!(at_ingest[0].4, Some(1));
    assert_eq!(backfill_chunk(&server.db).await.unwrap(), 0);
    server.cleanup().await;
}

/// Found by the R14 load test: a member's finops dashboard aggregated every
/// user's `trace_usage` rows for the window and dropped other agents only
/// afterwards — so each request cost as much as the whole organization's
/// history (~250 ms at 400k turns), and the summary figures summed straight
/// from those rows (`unpriced_calls`, `estimated_cost`,
/// `unknown_confidence_calls`) counted other users' spend.
#[tokio::test]
#[serial]
async fn member_finops_summary_counts_only_the_callers_agents() {
    let server = common::TestServer::start().await;
    let (user_id, _) = setup(&server).await;
    post(
        &server,
        user_id,
        &[event("own", "turn-1", CapturePolicy::MetadataOnly)],
    )
    .await;
    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, 'other', 'other@test.local')")
        .bind(other)
        .execute(&server.db)
        .await
        .unwrap();
    let other_agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, coding_agent_integration_id) \
         VALUES ('other-coding-agent', $1, 'claude') RETURNING id",
    )
    .bind(other)
    .fetch_one(&server.db)
    .await
    .unwrap();
    // The other user's spend: one call the price book could not cost, one estimated.
    for (trace, cost, estimated) in [
        ("other-unpriced", 0.0, None),
        ("other-estimated", 1.5, Some(true)),
    ] {
        sqlx::query(
            "INSERT INTO trace_usage (trace_id, agent_name, agent_id, user_id, model, \
                                      input_tokens, output_tokens, cost_usd, cost_estimated, started_at) \
             VALUES ($1, 'other-coding-agent', $2, $3, 'no-price-model', 10, 5, $4, $5, \
                     '2023-11-14T22:13:20Z')",
        )
        .bind(trace)
        .bind(other_agent)
        .bind(other)
        .bind(cost)
        .bind(estimated)
        .execute(&server.db)
        .await
        .unwrap();
    }

    let summary = |caller: Uuid| {
        let server = &server;
        async move {
            let dashboard: Value = common::as_member(
                server.client.get(server.url(
                    "/api/observability/finops/dashboard?start_time=2023-01-01T00%3A00%3A00Z&end_time=2024-01-01T00%3A00%3A00Z",
                )),
                &caller.to_string(),
                "member",
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
            dashboard["data"]["summary"].clone()
        }
    };
    let own = summary(user_id).await;
    assert_eq!(own["unpriced_calls"], 0, "{own}");
    // The caller's own turn is on a test model, so its cost is an estimate; only that.
    assert_eq!(own["estimated_cost"], own["total_cost"], "{own}");
    assert_eq!(own["total_operations"], 1, "{own}");
    let theirs = summary(other).await;
    assert_eq!(theirs["unpriced_calls"], 1, "{theirs}");
    assert_eq!(theirs["estimated_cost"].as_f64(), Some(1.5), "{theirs}");
    assert_eq!(theirs["unknown_confidence_calls"], 1, "{theirs}");
    server.cleanup().await;
}
