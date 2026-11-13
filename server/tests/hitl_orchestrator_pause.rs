mod common;

use serde_json::{Value, json};
use serial_test::serial;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

//  the orchestrator's
// `OrchestratorEvent::AwaitingHuman` arm in `a2a_dispatch.rs`, exercised through the real
// `/api/orchestrator/a2a` HTTP surface: a real LLM turn-0 streaming tool-call response, a real
// sub-agent that pauses, and a real `hitl_requests` insert against the actual `0007_hitl.sql`
// schema — not a unit test of the classifier in isolation.

const SUB_AGENT_TASK_ID: &str = "sub-task-999";
const SUB_AGENT_CONTEXT_ID: &str = "sub-ctx-999";
const OUTER_CONTEXT_ID: &str = "outer-ctx-fixed-for-hitl-test";

/// Insert a minimal user row — needed as the FK target for `hitl_requests.owner_user_id`
/// (and `agents.owner_id`). Mirrors `maf_flow.rs`'s `seed_user`.
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

/// Insert a `running` agent row pointing at a mock A2A endpoint (a mockito server standing in
/// for `oss/agents/github-hitl-agent`) so the orchestrator can both discover it
/// (`AgentSelector::fetch_active_agents`, which filters on `status = 'running'`) and reach it
/// over real HTTP (`agent_resolver`'s fallback to `agents.url`, since `FakeRuntime` reports no
/// live container for it).
async fn seed_running_agent(
    server: &common::TestServer,
    owner_id: Uuid,
    name: &str,
    url: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .expect("seed_running_agent")
}

/// One OpenAI-compatible streaming SSE chunk carrying a single, complete tool call — the shape
/// `rig-core-0.11.1`'s `send_compatible_streaming_request` recognizes as "entire tool call in one
/// delta" (name and arguments both present in the same delta, per
/// `providers/openai/streaming.rs`), so no follow-up chunk is needed. This is what turn 0 of
/// `run_stream_inner` goes through **when nothing asks it to buffer** — the path the tracker's
/// own Step 5 notes left without a black-box test (T5).
///
/// That caveat is load-bearing now: `use_non_streaming` is `turn_idx > 0 ||
/// policy.buffer_every_turn()`, and the enterprise delegation policy returns `true`, so on EE
/// turn 0 takes the non-streaming branch and never sees this fixture. These tests run against
/// the open-source server, whose `NoOrchestratorPolicy` configures no policy at all, so the
/// streaming branch is still the one under test here — but the EE pause path is a different
/// branch of the same loop, and it is not covered by this file.
fn streaming_tool_call_chunk(tool_name: &str, message: &str) -> String {
    let arguments = json!({ "message": message }).to_string();
    let chunk = json!({
        "id": "chatcmpl-hitl",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call-hitl",
                    "type": "function",
                    "function": { "name": tool_name, "arguments": arguments }
                }]
            }
        }]
    });
    format!("data: {chunk}\n\n")
}

/// A non-streaming A2A `SendMessage` reply in the `input_required` state — the sub-agent's own
/// task/context ids are deliberately distinct from the outer turn's, exactly what T6 requires be
/// verified rather than assumed. Mockito's default content-type is not `text/event-stream`, so
/// `send_message_streaming_dialect` takes the non-streaming fallback branch (`a2a.rs:446-482`),
/// the same branch `oss/agents/github-hitl-agent` and any agent without a live SSE stream hits.
fn sub_agent_pause_body() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": "1",
        "result": {"task": {
            "id": SUB_AGENT_TASK_ID,
            "contextId": SUB_AGENT_CONTEXT_ID,
            "status": {
                "state": "TASK_STATE_INPUT_REQUIRED",
                "message": {"parts": [{"text": "Which repository?"}]}
            }
        }}
    })
    .to_string()
}

fn orchestrator_request_body(text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "message/stream",
        "id": Uuid::new_v4().to_string(),
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "contextId": OUTER_CONTEXT_ID,
                "role": "ROLE_USER",
                "parts": [{ "text": text }]
            },
            "metadata": { "agent_id": "orchestrator" }
        }
    })
}

/// SAFETY: tests in this crate run serially (`#[serial]`), matching the same pattern
/// `common::test_config` already uses for `JWT_SECRET`/`S3_*`/etc — no concurrent env mutation.
unsafe fn set_openai_env(base_url: &str) {
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", base_url);
        std::env::set_var("OPENAI_API_KEY", "test-key-for-hitl-orchestrator-test");
    }
}

/// T6: full path through the real `/api/orchestrator/a2a` HTTP surface — a real (mocked) LLM
/// turn-0 streaming tool call, a real (mocked) sub-agent that pauses — asserts exactly one
/// `hitl_requests` row is created with the sub-agent's own task/context ids, verifiably distinct
/// from the outer turn's `context_id`, not just "a row exists".
#[tokio::test]
#[serial]
async fn hitl_pause_persists_request_and_emits_awaiting_human_event() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    let mut agent_mock_server = mockito::Server::new_async().await;
    let agent_mock = agent_mock_server
        .mock("POST", "/")
        .with_status(200)
        .with_body(sub_agent_pause_body())
        .expect(1)
        .create_async()
        .await;
    let agent_id = seed_running_agent(
        &server,
        user_id,
        "hitl-test-agent",
        &agent_mock_server.url(),
    )
    .await;

    let tool_name = format!(
        "call_agent_{}",
        "hitl-test-agent".replace(['-', ' ', '.', '/'], "_")
    );
    let mut llm_mock_server = mockito::Server::new_async().await;
    let llm_mock = llm_mock_server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_body(streaming_tool_call_chunk(&tool_name, "please open a PR"))
        .expect(1)
        .create_async()
        .await;
    unsafe { set_openai_env(&llm_mock_server.url()) };

    let resp = common::as_superuser(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&orchestrator_request_body("please help with the repo")),
        &user_id.to_string(),
        "hitl-tester",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("TASK_STATE_INPUT_REQUIRED"),
        "expected an input-required status event in the SSE body, got: {body}"
    );
    assert!(
        body.contains("awaiting_human"),
        "expected the awaiting_human data part in the SSE body, got: {body}"
    );

    llm_mock.assert_async().await;
    agent_mock.assert_async().await;

    let rows = sqlx::query(
        "SELECT id, kind, origin, task_id, context_id, chat_session_id, owner_user_id, agent_id
         FROM hitl_requests WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_all(&server.db)
    .await
    .unwrap();

    assert_eq!(rows.len(), 1, "expected exactly one hitl_requests row");
    let row = &rows[0];

    // Frontend discovery: the same stream also carries a `"type":"hitl"` data part naming this
    // exact row — the orchestrator's own version of "deliver pause metadata on the live stream,
    // not via /pending polling" (matches direct chat's `build_hitl_stream_event`, plus the
    // `agent` name direct chat never needs since the orchestrator can delegate to several agents).
    let hitl_frame = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .find_map(|event| {
            let parts = event
                .pointer("/result/statusUpdate/status/message/parts")
                .or_else(|| event.pointer("/statusUpdate/status/message/parts"))?;
            parts
                .as_array()?
                .iter()
                .find(|p| p.pointer("/data/type").and_then(|v| v.as_str()) == Some("hitl"))
                .and_then(|p| p.get("data"))
                .cloned()
        })
        .expect("expected a \"type\":\"hitl\" data part in the SSE body");
    assert_eq!(hitl_frame["agent"], "hitl-test-agent");
    assert_eq!(hitl_frame["kind"], "input_required");
    assert_eq!(hitl_frame["task_id"], SUB_AGENT_TASK_ID);
    assert_eq!(hitl_frame["context_id"], SUB_AGENT_CONTEXT_ID);
    // The id on the wire must name the row that was actually, durably committed above — proving
    // the frame is built from the real `hitl_store.create()` result, not minted or guessed.
    assert_eq!(
        hitl_frame["id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok()),
        Some(row.get::<Uuid, _>("id"))
    );
    assert_eq!(row.get::<String, _>("kind"), "input_required");
    assert_eq!(row.get::<String, _>("origin"), "orchestrator");
    assert_eq!(
        row.get::<Option<String>, _>("task_id").as_deref(),
        Some(SUB_AGENT_TASK_ID)
    );
    assert_eq!(
        row.get::<Option<String>, _>("context_id").as_deref(),
        Some(SUB_AGENT_CONTEXT_ID)
    );
    // The sub-agent's own context_id must be distinct from the outer turn's — the whole point of
    // capturing it at the A2A client layer (Step 2) instead of reusing the caller's context_id.
    assert_ne!(
        row.get::<Option<String>, _>("context_id").as_deref(),
        Some(OUTER_CONTEXT_ID)
    );
    assert_eq!(
        row.get::<Option<String>, _>("chat_session_id").as_deref(),
        Some(OUTER_CONTEXT_ID)
    );
    assert_eq!(row.get::<Uuid, _>("owner_user_id"), user_id);

    server.cleanup().await;
}

/// T13 (revised): a duplicate pause for the same `task_id` is now idempotent, not a failure.
///
/// This test used to force a `uq_hitl_pending_per_task` violation (pre-seed a conflicting
/// `pending` row for the same `task_id`) and assert it surfaced as a hard SSE error. Since Step 6
/// was switched from a raw `INSERT` to the real `HitlStore::create()` (built on
/// `feat/hitl-direct-chat`), that specific scenario is **no longer a failure at all** —
/// `create()` catches exactly this unique-violation and returns the existing pending row instead
/// of erroring (`oss/hitl/src/store.rs`'s `create`/`find_existing_pending`). That's a real,
/// deliberate behavior improvement (idempotent creation, matching the design doc's own intent),
/// not a regression — so this test now proves the idempotent behavior itself: the flow still
/// completes as an ordinary pause (no error), and the pre-existing row is left as the sole row,
/// not duplicated.
///
/// The original assertion this test made — "a real persistence failure surfaces as a hard SSE
/// error, never a fake success" — is still true of the unchanged code in `a2a_dispatch.rs`
/// (`if let Err(e) = persisted { ...emit `failed`...; break }`), but there is no longer a
/// realistic way to force `HitlStore::create()` to return `Err` through this HTTP surface without
/// either an impossible-to-time race (deleting the sub-agent's own `agents` row between routing
/// and persistence, both of which happen inside one synchronous request) or bypassing
/// authentication (tried: an unseeded calling user is rejected by `require_auth` itself —
/// "session user no longer exists" — before the handler is ever reached, let alone the insert).
/// Kept honest here rather than papered over with a contrived test.
#[tokio::test]
#[serial]
async fn hitl_pause_duplicate_for_same_task_is_idempotent_not_a_failure() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    let mut agent_mock_server = mockito::Server::new_async().await;
    let agent_mock = agent_mock_server
        .mock("POST", "/")
        .with_status(200)
        .with_body(sub_agent_pause_body())
        .expect(1)
        .create_async()
        .await;
    let agent_id = seed_running_agent(
        &server,
        user_id,
        "hitl-test-agent-2",
        &agent_mock_server.url(),
    )
    .await;

    // Pre-seed a `pending` row for the same task_id the sub-agent mock will report.
    // `uq_hitl_pending_per_task` allows only one — `HitlStore::create()` now treats hitting it as
    // "already pending, return the existing row" rather than an error.
    sqlx::query(
        r#"INSERT INTO hitl_requests (kind, origin, agent_id, owner_user_id, task_id, status, question)
           VALUES ('input_required', 'orchestrator', $1, $2, $3, 'pending', '{}'::jsonb)"#,
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(SUB_AGENT_TASK_ID)
    .execute(&server.db)
    .await
    .expect("seed conflicting hitl_requests row");

    let tool_name = format!(
        "call_agent_{}",
        "hitl-test-agent-2".replace(['-', ' ', '.', '/'], "_")
    );
    let mut llm_mock_server = mockito::Server::new_async().await;
    let llm_mock = llm_mock_server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_body(streaming_tool_call_chunk(&tool_name, "please open a PR"))
        .expect(1)
        .create_async()
        .await;
    unsafe { set_openai_env(&llm_mock_server.url()) };

    let resp = common::as_superuser(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&orchestrator_request_body("please help with the repo")),
        &user_id.to_string(),
        "hitl-tester-2",
    )
    .send()
    .await
    .unwrap();

    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, 200, "body: {body}");
    assert!(
        body.contains("TASK_STATE_INPUT_REQUIRED"),
        "a duplicate pause for the same task_id must still complete as an ordinary pause: {body}"
    );
    assert!(
        !body.contains("TASK_STATE_FAILED"),
        "idempotent duplicate creation must not surface as an error: {body}"
    );

    llm_mock.assert_async().await;
    agent_mock.assert_async().await;

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hitl_requests WHERE task_id = $1")
        .bind(SUB_AGENT_TASK_ID)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "the pre-existing row must be reused, not duplicated"
    );

    server.cleanup().await;
}

/// A completed A2A task reply, shaped for `nasiko_types::a2a::extract_text` (`task.status.message
/// .parts[].text`) — the resume dispatcher's `consume_json_to_terminal` reads exactly this shape
/// for a non-streaming reply, mockito's default content-type not being `text/event-stream`.
fn sub_agent_completed_body(reply_text: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": "1",
        "result": {"task": {
            "id": SUB_AGENT_TASK_ID,
            "contextId": SUB_AGENT_CONTEXT_ID,
            "status": {
                "state": "TASK_STATE_COMPLETED",
                "message": {"parts": [{"text": reply_text}]}
            }
        }}
    })
    .to_string()
}

/// One OpenAI-compatible streaming SSE chunk carrying a plain text final answer (no tool call) —
/// what turn 0 of the *resumed* orchestrator turn should produce once the sub-agent's reply gives
/// it enough to answer directly.
fn streaming_text_chunk(text: &str) -> String {
    let chunk = json!({
        "id": "chatcmpl-hitl",
        "model": "gpt-4o",
        "choices": [{ "index": 0, "delta": { "content": text } }]
    });
    format!("data: {chunk}\n\n")
}

/// Step 7, end to end: resolving a pending orchestrator-origin HITL request through the real
/// `/api/hitl/{id}/resolve` API must (a) resume the sub-agent's own paused task with the human's
/// answer, then (b) trigger a brand-new orchestrator turn on the same chat session — not just mark
/// the row resolved. Exercises the actual background dispatcher (`crate::hitl::run`, already
/// spawned by `TestServer::start()`, woken by `resolve()`'s best-effort notify) and
/// `trigger_new_orchestrator_turn`'s stream-draining, not a direct function call standing in for
/// either.
///
/// Two calls each to the LLM and the sub-agent mocks are distinguished by request body content
/// (`Matcher::Regex`) rather than call order, since mockito's own mock-selection order among
/// several registered mocks isn't a contract worth depending on.
#[tokio::test]
#[serial]
async fn hitl_resolve_resumes_sub_agent_then_triggers_new_orchestrator_turn() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    let mut agent_mock_server = mockito::Server::new_async().await;
    // First call: the orchestrator's initial delegation — pauses.
    let agent_pause_mock = agent_mock_server
        .mock("POST", "/")
        .match_body(mockito::Matcher::Regex("please open a PR".into()))
        .with_status(200)
        .with_body(sub_agent_pause_body())
        .expect(1)
        .create_async()
        .await;
    // Second call: the dispatcher's resume `SendMessage(taskId, answer)` — matched on the real
    // task_id `build_stream_request_for_task` embeds in the body, not on ordering.
    let agent_resume_mock = agent_mock_server
        .mock("POST", "/")
        .match_body(mockito::Matcher::Regex(SUB_AGENT_TASK_ID.into()))
        .with_status(200)
        .with_body(sub_agent_completed_body("PR opened at #42"))
        .expect(1)
        .create_async()
        .await;
    let agent_id = seed_running_agent(
        &server,
        user_id,
        "hitl-test-agent-3",
        &agent_mock_server.url(),
    )
    .await;

    let tool_name = format!(
        "call_agent_{}",
        "hitl-test-agent-3".replace(['-', ' ', '.', '/'], "_")
    );
    let mut llm_mock_server = mockito::Server::new_async().await;
    // First call: turn 0 of the original request — plans the delegation.
    let llm_tool_call_mock = llm_mock_server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("please help with the repo".into()))
        .with_status(200)
        .with_body(streaming_tool_call_chunk(&tool_name, "please open a PR"))
        .expect(1)
        .create_async()
        .await;
    // Second call: turn 0 of the *resumed* orchestrator turn — the continuation message
    // (`trigger_new_orchestrator_turn`'s own text) names the sub-agent's reply.
    let llm_final_answer_mock = llm_mock_server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("PR opened at #42".into()))
        .with_status(200)
        .with_body(streaming_text_chunk("The pull request is now open."))
        .expect(1)
        .create_async()
        .await;
    unsafe { set_openai_env(&llm_mock_server.url()) };

    let resp = common::as_superuser(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&orchestrator_request_body("please help with the repo")),
        &user_id.to_string(),
        "hitl-tester-3",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let _ = resp.text().await.unwrap();

    llm_tool_call_mock.assert_async().await;
    agent_pause_mock.assert_async().await;

    let hitl_id: Uuid = sqlx::query_scalar("SELECT id FROM hitl_requests WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();

    let resolve_resp = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/hitl/{hitl_id}/resolve")))
            .json(&json!({ "answer": "go ahead and open it" })),
        &user_id.to_string(),
        "hitl-tester-3",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resolve_resp.status(), 200, "resolve must succeed");

    // The dispatcher runs in the background (woken by resolve()'s notify, or its own 2s poll as a
    // fallback) — poll for the resumed turn's own final reply to land, rather than assuming any
    // fixed delay.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT content FROM chat_messages
             WHERE session_id = $1 AND role = 'assistant' AND content LIKE '%pull request is now open%'",
        )
        .bind(OUTER_CONTEXT_ID)
        .fetch_optional(&server.db)
        .await
        .unwrap();
        if text.is_some() {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        found,
        "the resumed orchestrator turn's final reply never landed in chat_messages"
    );

    agent_resume_mock.assert_async().await;
    llm_final_answer_mock.assert_async().await;

    let resume_status: String =
        sqlx::query_scalar("SELECT resume_status FROM hitl_requests WHERE id = $1")
            .bind(hitl_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(resume_status, "completed");

    // The continuation message itself must be visible in the resumed session's own history, not
    // just the reply — a turn that re-pauses leaves no assistant message behind, so this is the
    // only record of the resumed step. It is persisted under `INTERNAL_TRANSCRIPT_ROLE`, never
    // `user`: the platform wrote it, and as a user-role row the transcript drew a chat bubble
    // quoting the sub-agent back at the human as if they had typed it.
    let continuation_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM chat_messages
         WHERE session_id = $1 AND content LIKE '%PR opened at #42%'",
    )
    .bind(OUTER_CONTEXT_ID)
    .fetch_optional(&server.db)
    .await
    .unwrap();
    assert_eq!(
        continuation_role.as_deref(),
        Some("system"),
        "the continuation must be persisted for history, but never as the human's own turn"
    );

    server.cleanup().await;
}
