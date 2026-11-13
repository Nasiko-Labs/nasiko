//! HTTP-level tests for the HITL resolve API — `GET /api/hitl/pending`,
//! `POST /api/hitl/{id}/resolve` (`oss/server/src/router/hitl.rs`, M5).
//!
//!   cargo test -p nasiko-server --test hitl -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

/// Seed a `users` row directly — the resolve API only needs a valid JWT
/// `sub`; it doesn't require any particular role.
async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(username)
        .bind(format!("{username}@test.local"))
        .execute(&server.db)
        .await
        .unwrap();
    id
}

async fn seed_agent(server: &common::TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (id, name, owner_id) VALUES (gen_random_uuid(), $1, $2) RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Seeds a minimal, real `mcp_connectors` row and returns its id — a session-scope approval
/// (`scope=session`) calls `create_session_grant`, whose `connector_id` gained a real FK to this
/// table (`0026_mcp_session_tool_grants_fk.sql`); a synthetic `Uuid::new_v4()` connector id (fine
/// for `hitl_requests.connector_id`, which has no FK) now violates that constraint.
///
/// `url` is not optional padding: `source_kind` defaults to `external_url`, and
/// `chk_connectors_provider_fields` (`0003_mcp.sql`) requires `url IS NOT NULL` for that
/// combination, so a `(provider_type, name)`-only insert fails the CHECK. Never dialed — this row
/// exists only to satisfy the FK above.
async fn seed_connector(server: &common::TestServer, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, name, url) \
         VALUES ('mcp_server', $1, 'http://127.0.0.1:1/mcp') RETURNING id",
    )
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Insert a pending `hitl_requests` row directly (bypassing `nasiko_hitl::repo`,
/// which the HTTP layer under test also calls into — inserting independently
/// here keeps this an actual test of the HTTP surface, not a round-trip
/// through the same code).
#[allow(clippy::too_many_arguments)]
async fn seed_pending_tool_approval(
    server: &common::TestServer,
    agent_id: Uuid,
    owner_user_id: Uuid,
    connector_id: Uuid,
    tool_name: &str,
    context_id: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, connector_id, tool_name, context_id, question)
        VALUES
            ('tool_approval', 'mcp_tool', $1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(agent_id)
    .bind(owner_user_id)
    .bind(connector_id)
    .bind(tool_name)
    .bind(context_id)
    .bind(json!({"tool_name": tool_name}))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Insert a `chat_sessions` row directly — the FK target `chat_messages.session_id` needs, so a
/// resolve's answer-persistence insert (see the new test below) has somewhere real to land.
async fn seed_chat_session(
    server: &common::TestServer,
    session_id: &str,
    user_id: Uuid,
    agent_id: Uuid,
) {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, $3, '/api/agents/x', 'test session')",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();
}

/// Insert a pending `input_required` row directly, with an arbitrary `question` — used for both
/// the pre-existing plain-text shape and the selectable-options extension (`question.options`).
async fn seed_pending_input_required(
    server: &common::TestServer,
    agent_id: Uuid,
    owner_user_id: Uuid,
    context_id: &str,
    question: Value,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question)
        VALUES
            ('input_required', 'direct_chat', $1, $2, $3, $3, $4)
        RETURNING id
        "#,
    )
    .bind(agent_id)
    .bind(owner_user_id)
    .bind(context_id)
    .bind(question)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn list_pending_returns_only_the_callers_own_rows() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-1").await;
    let other = seed_user(&server, "hitl-other-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-1").await;

    let owned_request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;
    seed_pending_tool_approval(
        &server,
        agent_id,
        other,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-2",
    )
    .await;

    let res = common::as_member(
        server.client.get(server.url("/api/hitl/pending")),
        &owner.to_string(),
        "hitl-owner-1",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.json::<Value>().await.unwrap();
    let rows = body["data"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "must only see the caller's own pending row: {rows:?}"
    );
    // `owner_user_id` isn't part of the response DTO — `list_pending_for`'s own DB-level
    // scoping is what's under test here, so identity is confirmed via `id` instead.
    assert_eq!(rows[0]["id"], json!(owned_request_id.to_string()));
    assert_eq!(rows[0]["status"], json!("pending"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn resolve_by_a_non_owner_is_forbidden() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-2").await;
    let other = seed_user(&server, "hitl-other-2").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-2").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &other.to_string(),
        "hitl-other-2",
    )
    .json(&json!({"decision": "approve"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "pending",
        "a forbidden attempt must not mutate the row"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn owner_can_approve_a_pending_request() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-3").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-3").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-3",
    )
    .json(&json!({"decision": "approve", "note": "looks fine"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.json::<Value>().await.unwrap();
    // Unlike `list_pending`, `resolve`'s response is the DTO directly — no `data` wrapper —
    // and `resolved_by` isn't part of it; verified via the direct DB query below instead.
    assert_eq!(body["status"], json!("resolved"));

    let (status, resolved_by, human_response): (String, Option<Uuid>, Value) = sqlx::query_as(
        "SELECT status, resolved_by, human_response FROM hitl_requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(status, "resolved");
    assert_eq!(resolved_by, Some(owner));
    assert_eq!(human_response["decision"], json!("approve"));
    assert_eq!(human_response["note"], json!("looks fine"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn owner_can_reject_a_pending_request() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-4").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-4").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-4",
    )
    .json(&json!({"decision": "reject"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "rejected");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn resolving_an_already_resolved_request_is_idempotent() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-5").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-5").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let first = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-5",
    )
    .json(&json!({"decision": "approve"}))
    .send()
    .await
    .unwrap();
    assert_eq!(first.status(), 200);

    let second = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-5",
    )
    .json(&json!({"decision": "reject"}))
    .send()
    .await
    .unwrap();
    // A row that's already `resolved` (not `expired`) is an idempotent 200, not a conflict —
    // matching `resolve`'s own convention for a lost double-resolve race. `already_resolved`
    // in the body is what distinguishes this from a fresh decision being applied.
    assert_eq!(second.status(), 200);
    let second_body = second.json::<Value>().await.unwrap();
    assert_eq!(second_body["already_resolved"], json!(true));

    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "resolved",
        "the second (conflicting) attempt must not overwrite the first decision"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn resolving_an_unknown_id_returns_not_found() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-6").await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{}/resolve", Uuid::new_v4()))),
        &owner.to_string(),
        "hitl-owner-6",
    )
    .json(&json!({"decision": "approve"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── M7: scope=session creates a session grant ──────────────────────────────

#[tokio::test]
#[serial]
async fn approve_with_session_scope_creates_a_session_grant() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-7").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-7").await;
    let connector_id = seed_connector(&server, "session-scope-connector-7").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-7",
    )
    .json(&json!({"decision": "approve", "scope": "session"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let grant_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mcp_session_tool_grants \
         WHERE agent_id = $1 AND connector_id = $2 AND tool_name = $3 AND context_id = $4",
    )
    .bind(agent_id)
    .bind(connector_id)
    .bind("GITHUB_DELETE_REPO")
    .bind("ctx-1")
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        grant_count, 1,
        "approving with scope=session must record exactly one session grant"
    );

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(human_response["scope"], json!("session"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn approve_without_scope_defaults_to_once_and_creates_no_grant() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-8").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-8").await;
    let request_id = seed_pending_tool_approval(
        &server,
        agent_id,
        owner,
        Uuid::new_v4(),
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-8",
    )
    .json(&json!({"decision": "approve"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(human_response["scope"], json!("once"));

    let grant_count: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_session_tool_grants")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        grant_count, 0,
        "the default once-scope approval must never create a session grant"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_scope_is_rejected_for_a_non_tool_approval_kind() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-owner-9").await;
    let agent_id = seed_agent(&server, owner, "hitl-test-agent-9").await;

    let request_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, connector_id, context_id, question)
        VALUES
            ('auth_required', 'mcp_tool', $1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(agent_id)
    .bind(owner)
    .bind(Uuid::new_v4())
    .bind("ctx-1")
    .bind(json!({"connector": "github"}))
    .fetch_one(&server.db)
    .await
    .unwrap();

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-owner-9",
    )
    .json(&json!({"decision": "approve", "scope": "session"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "pending",
        "a rejected scope must not mutate the row"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn unauthenticated_requests_are_rejected() {
    let server = common::TestServer::start().await;

    let res = server
        .client
        .get(server.url("/api/hitl/pending"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

// ─── Selectable-options extension to `input_required` ──────────────────────────────────────
//
// The structured single-select "click to resolve, no Submit button" / multi-select "toggle,
// Submit resolves" behavior described in the request is a frontend interaction pattern — there is
// no frontend UI in this repo to exercise it against. What's tested here is the backend contract
// that interaction is built on: whatever `/resolve` request either interaction pattern eventually
// sends is validated and persisted correctly.

fn format_options_question() -> Value {
    json!({
        "message": "How should I format the output?",
        "header": "Format",
        "options": [
            {"label": "Summary", "description": "Brief overview"},
            {"label": "Detailed", "description": "Full explanation"},
        ],
        "multi_select": false,
        "allow_custom_input": true,
    })
}

fn sections_multiselect_question() -> Value {
    json!({
        "message": "Which sections should I include?",
        "options": [
            {"label": "Introduction"},
            {"label": "Architecture"},
            {"label": "Security"},
            {"label": "Conclusion"},
        ],
        "multi_select": true,
        "allow_custom_input": true,
    })
}

#[tokio::test]
#[serial]
async fn plain_input_required_without_options_still_works_unchanged() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-plain-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-plain-agent-1").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-plain-1",
        json!({ "message": "Please provide the environment name." }),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-plain-1",
    )
    .json(&json!({"answer": "production"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(human_response, json!({ "answer": "production" }));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn plain_input_required_still_rejects_an_empty_answer() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-plain-2").await;
    let agent_id = seed_agent(&server, owner, "hitl-plain-agent-2").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-plain-2",
        json!({ "message": "Please provide the environment name." }),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-plain-2",
    )
    .json(&json!({"answer": "   "}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn single_select_predefined_option_resolves_with_the_label() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ss-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-ss-agent-1").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ss-1",
        format_options_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ss-1",
    )
    .json(&json!({"answer": "Summary"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.json::<Value>().await.unwrap();
    assert_eq!(body["status"], json!("resolved"));
    assert_eq!(body["human_response"], json!({ "answer": "Summary" }));

    // Round-trips through the resolve DTO too — `question.header`/`options` are passed through
    // verbatim (`to_response`), not stripped.
    assert_eq!(body["question"]["header"], json!("Format"));
    assert_eq!(body["question"]["options"][0]["label"], json!("Summary"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn single_select_rejects_an_option_that_was_never_offered() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ss-2").await;
    let agent_id = seed_agent(&server, owner, "hitl-ss-agent-2").await;
    // allow_custom_input = false — an unlisted answer must be rejected, not silently accepted
    // as custom text.
    let mut question = format_options_question();
    question["allow_custom_input"] = json!(false);
    let request_id =
        seed_pending_input_required(&server, agent_id, owner, "ctx-ss-2", question).await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ss-2",
    )
    .json(&json!({"answer": "Delete everything"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    let status: String = sqlx::query_scalar("SELECT status FROM hitl_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "pending",
        "a rejected answer must not resolve the row"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn single_select_custom_input_is_accepted_when_allowed() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ss-3").await;
    let agent_id = seed_agent(&server, owner, "hitl-ss-agent-3").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ss-3",
        format_options_question(), // allow_custom_input: true
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ss-3",
    )
    .json(&json!({"answer": "Give me a concise executive summary"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        human_response,
        json!({ "answer": "Give me a concise executive summary" })
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn multi_select_zero_selections_is_valid() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ms-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-ms-agent-1").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ms-1",
        sections_multiselect_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ms-1",
    )
    .json(&json!({"answer": [], "custom_answer": "Only discuss security implications"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        human_response,
        json!({ "answer": [], "custom_answer": "Only discuss security implications" })
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn multi_select_one_selection_is_valid() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ms-2").await;
    let agent_id = seed_agent(&server, owner, "hitl-ms-agent-2").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ms-2",
        sections_multiselect_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ms-2",
    )
    .json(&json!({"answer": ["Introduction"]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(human_response, json!({ "answer": ["Introduction"] }));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn multi_select_multiple_selections_plus_custom_answer_is_valid() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ms-3").await;
    let agent_id = seed_agent(&server, owner, "hitl-ms-agent-3").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ms-3",
        sections_multiselect_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ms-3",
    )
    .json(&json!({
        "answer": ["Introduction", "Security"],
        "custom_answer": "Also include deployment risks",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let human_response: Value =
        sqlx::query_scalar("SELECT human_response FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        human_response,
        json!({
            "answer": ["Introduction", "Security"],
            "custom_answer": "Also include deployment risks",
        })
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn multi_select_rejects_an_option_that_was_never_offered() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ms-4").await;
    let agent_id = seed_agent(&server, owner, "hitl-ms-agent-4").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ms-4",
        sections_multiselect_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ms-4",
    )
    .json(&json!({"answer": ["Not a real section"]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn multi_select_zero_selections_and_no_custom_answer_is_rejected() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-ms-5").await;
    let agent_id = seed_agent(&server, owner, "hitl-ms-agent-5").await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        "ctx-ms-5",
        sections_multiselect_question(),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-ms-5",
    )
    .json(&json!({"answer": []}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn custom_input_is_rejected_when_the_question_disallows_it() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-custom-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-custom-agent-1").await;
    let mut question = format_options_question();
    question["allow_custom_input"] = json!(false);
    let request_id =
        seed_pending_input_required(&server, agent_id, owner, "ctx-custom-1", question).await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-custom-1",
    )
    .json(&json!({"answer": "Something not on the list"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

// ─── Answer persisted into chat_messages (session-visible history) ─────────────────────────────

/// Regression test: resolving `input_required` previously only wrote the human's answer into
/// `hitl_requests.human_response` — nothing ever appended it to `chat_messages`, so a session
/// with an answered pause showed the agent's question, then jumped straight to whatever it said
/// after resuming, with the human's own reply invisible in both the CLI and the web UI's session
/// view. `router::hitl::resolve` now does this itself (fire-and-forget, once per resolution).
#[tokio::test]
#[serial]
async fn resolving_input_required_persists_the_answer_as_a_chat_message() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-history-1").await;
    let agent_id = seed_agent(&server, owner, "hitl-history-agent-1").await;
    let context_id = "ses_history_1";
    seed_chat_session(&server, context_id, owner, agent_id).await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        context_id,
        json!({"message": "What repo?"}),
    )
    .await;

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &owner.to_string(),
        "hitl-history-1",
    )
    .json(&json!({"answer": "nasiko-bishnu/test"}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // The insert is fire-and-forget (`tokio::spawn`) — give it a moment to land rather than
    // asserting on the very next tick.
    let content: Option<String> = poll_for_chat_message(&server, context_id).await;
    assert_eq!(
        content.as_deref(),
        Some("nasiko-bishnu/test"),
        "the human's answer must show up as a real chat_messages turn, not just human_response"
    );
    // Tagged with its request, so the web UI shows it on the request's card instead of as a new turn.
    let tagged: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'hitl_request_id' FROM chat_messages WHERE session_id = $1 AND role = 'user'",
    )
    .bind(context_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(tagged, Some(request_id.to_string()));

    server.cleanup().await;
}

/// Two different requests in one chat, answered with the same text in quick succession, each keep
/// their own answer row: an earlier text-based dedup window dropped the second one.
#[tokio::test]
#[serial]
async fn same_answer_to_two_requests_persists_both_answers() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-history-2").await;
    // Two agents: pending requests are unique per (owner, agent, task), and the seed uses the
    // context id as the task id.
    let first_agent = seed_agent(&server, owner, "hitl-history-agent-2a").await;
    let second_agent = seed_agent(&server, owner, "hitl-history-agent-2b").await;
    let context_id = "ses_history_2";
    seed_chat_session(&server, context_id, owner, first_agent).await;
    let first = seed_pending_input_required(
        &server,
        first_agent,
        owner,
        context_id,
        json!({"message": "Deploy now?"}),
    )
    .await;
    let second = seed_pending_input_required(
        &server,
        second_agent,
        owner,
        context_id,
        json!({"message": "Notify the team?"}),
    )
    .await;

    for id in [first, second] {
        let res = common::as_member(
            server
                .client
                .post(server.url(&format!("/api/hitl/{id}/resolve"))),
            &owner.to_string(),
            "hitl-history-2",
        )
        .json(&json!({"answer": "yes"}))
        .send()
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
    }

    // Fire-and-forget inserts: poll until both have landed.
    let mut tags: Vec<String> = Vec::new();
    for _ in 0..20 {
        tags = sqlx::query_scalar(
            "SELECT metadata->>'hitl_request_id' FROM chat_messages \
             WHERE session_id = $1 AND role = 'user' AND content = 'yes'",
        )
        .bind(context_id)
        .fetch_all(&server.db)
        .await
        .unwrap();
        if tags.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tags.sort();
    let mut expected = vec![first.to_string(), second.to_string()];
    expected.sort();
    assert_eq!(tags, expected, "each request keeps its own answer row");

    server.cleanup().await;
}

/// Idempotent double-resolve (same row, resolved again) must not append the answer a second
/// time — `already_resolved: true` responses are excluded from the persistence write.
#[tokio::test]
#[serial]
async fn resolving_an_already_resolved_request_does_not_duplicate_the_chat_message() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "hitl-history-2").await;
    let agent_id = seed_agent(&server, owner, "hitl-history-agent-2").await;
    let context_id = "ses_history_2";
    seed_chat_session(&server, context_id, owner, agent_id).await;
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        owner,
        context_id,
        json!({"message": "What repo?"}),
    )
    .await;

    let resolve = || {
        common::as_member(
            server
                .client
                .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
            &owner.to_string(),
            "hitl-history-2",
        )
        .json(&json!({"answer": "nasiko-bishnu/test"}))
        .send()
    };
    assert_eq!(resolve().await.unwrap().status(), 200);
    poll_for_chat_message(&server, context_id).await;
    let second = resolve().await.unwrap();
    assert_eq!(second.status(), 200);
    assert_eq!(
        second.json::<Value>().await.unwrap()["already_resolved"],
        true
    );

    // Give any (incorrect) second insert the same grace period the first one needed, then count.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'user' AND content = $2",
    )
    .bind(context_id)
    .bind("nasiko-bishnu/test")
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "an already-resolved duplicate resolve must not append the answer again"
    );

    server.cleanup().await;
}

/// Raised in review (PR #383, `oss/hitl/src/authz.rs`): "an agent is uploaded by User A, shared
/// across team/dept, User B wants to chat with this agent — this action will return 403".
///
/// It does not, because `hitl_requests.owner_user_id` is the user the paused execution is
/// attributed to, never the agent's owner — every creation site binds the caller
/// (`agent_proxy.rs`'s `claims.sub`, `a2a_dispatch.rs`/`worker.rs`'s `user_id`, the MCP gateway's
/// traceparent-resolved flow user). Access to the *agent* is a separate check
/// (`acl.rs::user_can_access_agent`), which is where grants are honoured.
///
/// Both directions are asserted together on purpose: the allow case alone would still pass if
/// someone "fixed" this by widening the check to agent access, which would hand A the ability to
/// answer B's question — and for `tool_approval` that means spending B's own connector
/// credentials. The grant row is seeded even though `resolve` never reads it, so the fixture is
/// the reviewer's actual scenario rather than two unrelated users.
#[tokio::test]
#[serial]
async fn a_grantee_resolves_their_own_pause_on_someone_elses_agent_but_the_agent_owner_cannot() {
    let server = common::TestServer::start().await;
    let agent_owner = seed_user(&server, "hitl-agent-owner-shared").await;
    let chatter = seed_user(&server, "hitl-grantee-shared").await;
    let agent_id = seed_agent(&server, agent_owner, "hitl-shared-agent").await;

    // A shares the agent with B, exactly as `nasiko-ee access grant` would.
    sqlx::query(
        "INSERT INTO agent_grants (agent_id, grant_type, grantee_id, granted_by) \
         VALUES ($1, 'user', $2, $3)",
    )
    .bind(agent_id)
    .bind(chatter.to_string())
    .bind(agent_owner)
    .execute(&server.db)
    .await
    .unwrap();

    // B chats with A's agent and it pauses: the row is attributed to B, on A's agent.
    let request_id = seed_pending_input_required(
        &server,
        agent_id,
        chatter,
        "ctx-shared-agent",
        json!({"message": "which repo?"}),
    )
    .await;

    // The agent's owner must NOT be able to answer someone else's question on their own agent.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &agent_owner.to_string(),
        "hitl-agent-owner-shared",
    )
    .json(&json!({"answer": "nasiko-cloud-rs"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "owning the agent must not grant the right to answer another user's pause on it"
    );

    // B answers their own question and is allowed, despite not owning the agent.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{request_id}/resolve"))),
        &chatter.to_string(),
        "hitl-grantee-shared",
    )
    .json(&json!({"answer": "nasiko-cloud-rs"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "a grantee answering their OWN pause on a shared agent must succeed, not 403"
    );

    let (status, resolved_by): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, resolved_by FROM hitl_requests WHERE id = $1")
            .bind(request_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(status, "resolved");
    assert_eq!(
        resolved_by,
        Some(chatter),
        "the answer must be attributed to the grantee who gave it"
    );

    server.cleanup().await;
}

/// Polls `chat_messages` briefly for the fire-and-forget answer-persistence insert to land,
/// rather than asserting on the very next tick after the HTTP response returns.
async fn poll_for_chat_message(server: &common::TestServer, session_id: &str) -> Option<String> {
    for _ in 0..20 {
        let content: Option<String> =
            sqlx::query_scalar("SELECT content FROM chat_messages WHERE session_id = $1 AND role = 'user' ORDER BY timestamp DESC LIMIT 1")
                .bind(session_id)
                .fetch_optional(&server.db)
                .await
                .unwrap();
        if content.is_some() {
            return content;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    None
}
