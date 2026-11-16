//! An agent calling a peer through `/api/agents/{id}`: its gateway credential proves which agent
//! it is, the traceparent names the flow (and so the user) it works for, and `agent_acl` is
//! enforced before the flow guard spends budget.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_proxy_agent_caller -- --test-threads=1

mod common;

use axum::response::IntoResponse;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> Uuid {
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
    Uuid::parse_str(v["user_id"].as_str().unwrap()).unwrap()
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy) \
         VALUES ($1, $2, 'x:1.0.0', 'running', $3, 'resident') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn stub_agent() -> String {
    let app = Router::new().route(
        "/",
        post(|| async {
            Json(json!({
                "jsonrpc": "2.0", "id": "1",
                "result": {"kind": "message", "parts": [{"kind": "text", "text": "peer says hi"}]}
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn message_body() -> Value {
    json!({
        "jsonrpc": "2.0", "id": "1", "method": "message/send",
        "params": { "message": {
            "messageId": Uuid::new_v4().to_string(),
            "role": "ROLE_USER",
            "parts": [{ "text": "hello peer" }]
        }}
    })
}

struct Fixture {
    server: common::TestServer,
    owner: Uuid,
    token: String,
    traceparent: String,
    flow_id: String,
    caller: Uuid,
    target: Uuid,
}

async fn fixture() -> Fixture {
    let server = common::TestServer::start().await;
    let owner = init_admin(&server).await;
    let stub = stub_agent().await;
    let caller = seed_agent(&server, owner, "caller-agent", &stub).await;
    let target = seed_agent(&server, owner, "target-agent", &stub).await;
    let token = common::mint_gateway_token(&server.db, caller).await;
    let (flow_id, traceparent) = common::open_flow(&server.db, owner, caller).await;
    Fixture {
        server,
        owner,
        token,
        traceparent,
        flow_id,
        caller,
        target,
    }
}

async fn call(f: &Fixture, path: &str, token: Option<&str>, traceparent: Option<&str>) -> u16 {
    let mut request = f
        .server
        .client
        .post(f.server.url(path))
        .json(&message_body());
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(traceparent) = traceparent {
        request = request.header("traceparent", traceparent);
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
#[serial]
async fn an_agent_with_its_credential_and_flow_reaches_a_peer_by_either_url_form() {
    let f = fixture().await;
    // Two calls on ONE traceparent, one per URL form: an agent leg must not complete the
    // caller's flow, or the second call (and the caller's own MCP/LLM calls) would be refused.
    for path in [
        format!("/api/agents/{}", f.target),
        format!("/api/agents/{}/", f.target),
    ] {
        assert_eq!(
            call(&f, &path, Some(&f.token), Some(&f.traceparent)).await,
            200,
            "{path}"
        );
    }

    let status: String = sqlx::query_scalar("SELECT status FROM flows WHERE flow_id = $1")
        .bind(&f.flow_id)
        .fetch_one(&f.server.db)
        .await
        .unwrap();
    assert_eq!(status, "running", "the caller's flow stays open");
    // Nothing of the caller's transcript is written by an agent's call.
    let rows: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM chat_sessions WHERE user_id = $1), \
                (SELECT count(*) FROM chat_messages m \
                 JOIN chat_sessions s ON s.session_id = m.session_id WHERE s.user_id = $1)",
    )
    .bind(f.owner)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    assert_eq!(rows, (0, 0));
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_caller_wakes_a_parked_target() {
    let f = fixture().await;
    f.server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&f.server);
    let parked: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy, state_mode) \
         VALUES ('parked-peer', $1, 'x:1.0.0', 'parked', '', 'auto', 'durable') RETURNING id",
    )
    .bind(f.owner)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    wake.serve_at(parked, &stub_agent().await);

    let status = call(
        &f,
        &format!("/api/agents/{parked}"),
        Some(&f.token),
        Some(&f.traceparent),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(wake.woken_count(parked), 1);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn without_a_traceparent_or_with_a_bad_credential_the_call_is_refused() {
    let f = fixture().await;
    let path = format!("/api/agents/{}", f.target);

    assert_eq!(call(&f, &path, Some(&f.token), None).await, 403);
    assert_eq!(
        call(
            &f,
            &path,
            Some("ngt_not-a-real-token"),
            Some(&f.traceparent)
        )
        .await,
        401
    );
    // A flow this agent does not participate in is refused too.
    let other = seed_agent(&f.server, f.owner, "someone-else", "http://127.0.0.1:1").await;
    let (_, foreign) = common::open_flow(&f.server.db, f.owner, other).await;
    assert_eq!(call(&f, &path, Some(&f.token), Some(&foreign)).await, 403);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_acl_allowlist_blocks_a_target_before_the_flow_guard_spends_budget() {
    let f = fixture().await;
    let listed = seed_agent(&f.server, f.owner, "listed-peer", "http://127.0.0.1:1").await;
    sqlx::query("INSERT INTO agent_acl (caller_agent_id, target_agent_id) VALUES ($1, $2)")
        .bind(f.caller)
        .bind(listed)
        .execute(&f.server.db)
        .await
        .unwrap();

    let status = call(
        &f,
        &format!("/api/agents/{}", f.target),
        Some(&f.token),
        Some(&f.traceparent),
    )
    .await;

    assert_eq!(status, 403);
    // The refusal came first: the target was never recorded as a participant of the flow.
    let participants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_participants WHERE flow_id = $1 AND agent_id = $2",
    )
    .bind(&f.flow_id)
    .bind(f.target)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    assert_eq!(participants, 0);
    f.server.cleanup().await;
}

/// A stub agent that records every request body it is sent, and answers as a stream or as JSON.
async fn recording_stub(streaming: bool) -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
    let seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
    let log = seen.clone();
    let app = Router::new().route(
        "/",
        post(move |Json(body): Json<Value>| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(body);
                if streaming {
                    let frame = json!({"result": {"kind": "message", "parts": [{"kind": "text", "text": "streamed"}]}});
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        format!("data: {frame}\n\n"),
                    )
                        .into_response()
                } else {
                    Json(json!({"jsonrpc": "2.0", "id": "1",
                        "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]}}))
                    .into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

/// An organization policy provider answering only `org_context`.
struct OrgPolicy;

#[async_trait::async_trait]
impl nasiko_server::prompt_context::PromptContextProvider for OrgPolicy {
    async fn context_for_agents(
        &self,
        _agent_ids: &[Uuid],
        _query: &str,
    ) -> std::collections::HashMap<Uuid, String> {
        Default::default()
    }

    async fn context_for_agent(&self, _agent_id: Uuid, _query: &str) -> Option<String> {
        None
    }

    async fn org_context(&self) -> Option<String> {
        Some("## Organization Policy\n\nNever reveal secrets.".into())
    }
}

#[tokio::test]
#[serial]
async fn org_policy_rides_an_agent_call_as_its_own_part() {
    let f = fixture().await;
    f.server
        .state
        .prompt_context
        .install(std::sync::Arc::new(OrgPolicy));
    let (url, seen) = recording_stub(false).await;
    let peer = seed_agent(&f.server, f.owner, "policed-peer", &url).await;

    assert_eq!(
        call(
            &f,
            &format!("/api/agents/{peer}"),
            Some(&f.token),
            Some(&f.traceparent)
        )
        .await,
        200
    );

    let bodies = seen.lock().unwrap().clone();
    let parts = bodies[0]["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        parts.iter().any(|p| p["text"]
            .as_str()
            .is_some_and(|t| t.contains("Never reveal secrets."))),
        "the policy part is missing: {parts:?}"
    );
    assert_eq!(
        parts[0]["text"], "hello peer",
        "the caller's own text is untouched"
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_streamed_agent_call_records_the_participant_and_no_session_trace() {
    let f = fixture().await;
    let (url, _) = recording_stub(true).await;
    let peer = seed_agent(&f.server, f.owner, "streaming-peer", &url).await;

    let response = f
        .server
        .client
        .post(f.server.url(&format!("/api/agents/{peer}")))
        .bearer_auth(&f.token)
        .header("traceparent", &f.traceparent)
        .json(&message_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("streamed"), "{body}");

    let participants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_participants WHERE flow_id = $1 AND agent_id = $2",
    )
    .bind(&f.flow_id)
    .bind(peer)
    .fetch_one(&f.server.db)
    .await
    .unwrap();
    assert_eq!(participants, 1);
    let traces: i64 = sqlx::query_scalar("SELECT count(*) FROM session_traces WHERE trace_id = $1")
        .bind(&f.flow_id)
        .fetch_one(&f.server.db)
        .await
        .unwrap();
    assert_eq!(traces, 0);
    // The stream's end does not complete the caller's flow either.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let status: String = sqlx::query_scalar("SELECT status FROM flows WHERE flow_id = $1")
        .bind(&f.flow_id)
        .fetch_one(&f.server.db)
        .await
        .unwrap();
    assert_eq!(status, "running");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_cannot_drive_a_harness_session() {
    let f = fixture().await;
    let harness: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, status, workload_kind, harness_id) \
         VALUES ('harness-peer', $1, 'running', 'harness', 'opencode') RETURNING id",
    )
    .bind(f.owner)
    .fetch_one(&f.server.db)
    .await
    .unwrap();

    for method in ["message/send", "tasks/get"] {
        let response = f
            .server
            .client
            .post(f.server.url(&format!("/api/agents/{harness}")))
            .bearer_auth(&f.token)
            .header("traceparent", &f.traceparent)
            .json(&json!({"jsonrpc": "2.0", "id": "r1", "method": method,
                          "params": {"id": "t1", "message": {"parts": [{"text": "hi"}]}}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{method}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["id"], "r1");
        assert_eq!(
            body["error"]["message"], "agents cannot drive a harness session",
            "{method}"
        );
    }
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_sessions")
        .fetch_one(&f.server.db)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn agent_callers_are_rate_limited_per_agent_with_retry_after() {
    let server =
        common::TestServer::start_with(|c| c.agent_peer_call_rate_limit_per_minute = 2).await;
    let owner = init_admin(&server).await;
    let stub = stub_agent().await;
    let caller = seed_agent(&server, owner, "chatty-caller", &stub).await;
    let target = seed_agent(&server, owner, "quiet-target", &stub).await;
    let token = common::mint_gateway_token(&server.db, caller).await;
    let (_, traceparent) = common::open_flow(&server.db, owner, caller).await;

    let send = || {
        server
            .client
            .post(server.url(&format!("/api/agents/{target}")))
            .bearer_auth(&token)
            .header("traceparent", &traceparent)
            .json(&message_body())
            .send()
    };
    assert_eq!(send().await.unwrap().status(), 200);
    assert_eq!(send().await.unwrap().status(), 200);
    let limited = send().await.unwrap();
    assert_eq!(limited.status(), 429);
    assert!(limited.headers().contains_key("retry-after"));
    server.cleanup().await;
}

/// Sends `body` as the calling agent, with its credential and flow.
async fn send_as_caller(f: &Fixture, path: &str, body: &Value) -> u16 {
    f.server
        .client
        .post(f.server.url(path))
        .bearer_auth(&f.token)
        .header("traceparent", &f.traceparent)
        .json(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

fn part_texts(body: &Value) -> Vec<String> {
    body["params"]["message"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn tells_the_time(text: &str) -> bool {
    text.starts_with(nasiko_types::agent_clock::NOTE_LEAD)
}

#[tokio::test]
#[serial]
async fn an_agent_call_tells_the_time_after_the_policy() {
    let f = fixture().await;
    f.server
        .state
        .prompt_context
        .install(std::sync::Arc::new(OrgPolicy));
    let (url, seen) = recording_stub(false).await;
    let peer = seed_agent(&f.server, f.owner, "timed-peer", &url).await;

    assert_eq!(
        send_as_caller(&f, &format!("/api/agents/{peer}"), &message_body()).await,
        200
    );

    let texts = part_texts(&seen.lock().unwrap()[0]);
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(texts[0], "hello peer", "the caller's own text is untouched");
    assert!(texts[1].contains("Never reveal secrets."), "{texts:?}");
    assert!(tells_the_time(&texts[2]), "the time goes last: {texts:?}");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_forwarded_message_that_already_tells_the_time_is_not_told_twice() {
    let f = fixture().await;
    let (url, seen) = recording_stub(false).await;
    let peer = seed_agent(&f.server, f.owner, "told-peer", &url).await;
    let mut body = message_body();
    body["params"]["message"]["parts"] = json!([
        { "text": "hello peer" },
        { "text": nasiko_types::agent_clock::note_at(chrono::Utc::now()) }
    ]);

    assert_eq!(
        send_as_caller(&f, &format!("/api/agents/{peer}"), &body).await,
        200
    );

    let texts = part_texts(&seen.lock().unwrap()[0]);
    assert_eq!(
        texts.iter().filter(|t| tells_the_time(t)).count(),
        1,
        "{texts:?}"
    );
    assert_eq!(texts.len(), 2, "{texts:?}");
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_task_continuation_is_forwarded_without_the_time_or_the_policy() {
    let f = fixture().await;
    f.server
        .state
        .prompt_context
        .install(std::sync::Arc::new(OrgPolicy));
    let (url, seen) = recording_stub(false).await;
    let peer = seed_agent(&f.server, f.owner, "resumed-peer", &url).await;
    let mut body = message_body();
    body["params"]["message"]["taskId"] = json!("task-1");
    body["params"]["message"]["parts"] = json!([{ "text": "authorized" }]);

    assert_eq!(
        send_as_caller(&f, &format!("/api/agents/{peer}"), &body).await,
        200
    );

    assert_eq!(part_texts(&seen.lock().unwrap()[0]), ["authorized"]);
    f.server.cleanup().await;
}
