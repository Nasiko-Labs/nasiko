//! The `agent_lost_task` resume outcome and the HITL pause / resume metrics.
//!
//! A paused A2A task lives in the agent's own memory. When the agent restarts, or the human's
//! answer reaches a replica that never saw the task, the agent replies with A2A
//! `TaskNotFoundError` instead of continuing. The resume dispatcher
//! (`oss/server/src/hitl/mod.rs`) must record that as the terminal `agent_lost_task` outcome,
//! not as a delivered answer (`completed`) and not as a retryable failure.
//!
//! Agents report "no such task" with a JSON-RPC `error` of code -32001, in an SSE frame or a JSON
//! body at HTTP 200 (what the Python a2a-sdk sends), or as the body of an HTTP 404 (the status the
//! Rust a2a-server maps `TaskNotFoundError` to, `oss/docs/A2A_PROTOCOL.md`, "HTTP Status Code
//! Mapping"). A 404 whose body is not that error (a proxy's HTML page) is an ordinary retryable
//! failure. One such answer does not conclude anything: the resume is re-sent
//! `HITL_LOST_TASK_RETRIES` times first, so the row is lost only after every one of the N+1 POSTs
//! was answered "task not found".
//!
//! The metric tests install a process-wide OTel meter provider backed by a `ManualReader` before
//! any server starts, so the real instruments (`nasiko.cp` meter) can be read back. Every test
//! uses its own agent id, which is also the attribute the metrics are keyed by, so the shared
//! provider never mixes two tests' data points.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test hitl_agent_lost_task -- --test-threads=1`

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::{Histogram, ResourceMetrics, Sum};
use opentelemetry_sdk::metrics::reader::MetricReader;
use opentelemetry_sdk::metrics::{
    InstrumentKind, ManualReader, MetricResult, Pipeline, SdkMeterProvider, Temporality,
};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── The fake agent ──────────────────────────────────────────────────────────

/// How the fake agent answers the call that resumes the paused task.
#[derive(Clone, Copy)]
enum ResumeReply {
    /// JSON-RPC `TaskNotFoundError` as an SSE frame at HTTP 200 (Python a2a-sdk, `message/stream`).
    TaskNotFoundFrame,
    /// The same error as a JSON body at HTTP 200 (a non-streaming agent).
    TaskNotFoundJsonBody,
    /// The same error with HTTP 404 (Rust a2a-server).
    TaskNotFoundHttp404,
    /// HTTP 404 with an HTML body, as a proxy or ingress answers: says nothing about the task.
    Html404,
    /// "Task not found" (SSE frame) for the first `n` resume POSTs, then the task completes: a
    /// round-robin Service that reached a replica without the task, then one with it.
    LostThenCompleted(usize),
    /// A JSON-RPC error that is not "task not found": must keep today's handling.
    OtherJsonRpcError,
    /// The task continues and completes.
    Completed,
    /// HTTP 200 `text/event-stream`, one partial frame, then the connection drops mid-body: the
    /// agent may already have acted, so the outcome cannot be classified.
    StreamBreaksMidBody,
}

#[derive(Clone)]
struct FakeAgent {
    resume_reply: ResumeReply,
    /// `true` when the first request the agent sees is the original turn, which pauses (the
    /// end-to-end tests); `false` when the test seeds the paused row and the first request is
    /// already the resume.
    pauses_first: bool,
    calls: Arc<AtomicUsize>,
}

fn task_not_found_error(request_id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": { "code": -32001, "message": "Task not found" },
    })
}

fn sse_response(data: &Value) -> Response {
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!("data: {data}\n\n")))
        .unwrap()
}

fn json_response(status: u16, data: &Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(data.to_string()))
        .unwrap()
}

fn status_frame(status: nasiko_types::a2a::TaskStatusUpdateEvent) -> Value {
    serde_json::to_value(nasiko_types::a2a::status_event(status)).unwrap()
}

async fn fake_agent_handler(State(agent): State<FakeAgent>, body: Bytes) -> Response {
    let call_index = agent.calls.fetch_add(1, Ordering::SeqCst);
    let request: Value = serde_json::from_slice(&body).unwrap_or_default();
    let message = &request["params"]["message"];
    let task_id = message["taskId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map_or_else(|| format!("agent-task-{}", Uuid::new_v4()), str::to_string);
    let context_id = message["contextId"].as_str().unwrap_or("ctx").to_string();

    if agent.pauses_first && call_index == 0 {
        return sse_response(&status_frame(nasiko_types::a2a::input_required(
            &task_id,
            &context_id,
            "Which region?",
        )));
    }

    let request_id = &request["id"];
    match agent.resume_reply {
        ResumeReply::LostThenCompleted(lost_posts) if call_index < lost_posts => {
            sse_response(&task_not_found_error(request_id))
        }
        ResumeReply::LostThenCompleted(_) => sse_response(&status_frame(
            nasiko_types::a2a::completed(&task_id, &context_id),
        )),
        ResumeReply::Html404 => Response::builder()
            .status(404)
            .header("content-type", "text/html")
            .body(axum::body::Body::from(
                "<html><h1>404 Not Found</h1></html>",
            ))
            .unwrap(),
        ResumeReply::TaskNotFoundFrame => sse_response(&task_not_found_error(request_id)),
        ResumeReply::TaskNotFoundJsonBody => json_response(200, &task_not_found_error(request_id)),
        ResumeReply::TaskNotFoundHttp404 => json_response(404, &task_not_found_error(request_id)),
        ResumeReply::OtherJsonRpcError => sse_response(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": { "code": -32603, "message": "Internal error" },
        })),
        ResumeReply::StreamBreaksMidBody => Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(async_stream::stream! {
                yield Ok::<_, std::io::Error>(Bytes::from_static(b"data: {\"jsonrpc\":"));
                // Lets the headers and the partial frame reach the client before the drop, so
                // the failure surfaces while the body is read, not while the request is sent.
                tokio::time::sleep(Duration::from_millis(200)).await;
                yield Err(std::io::Error::other("connection dropped"));
            }))
            .unwrap(),
        ResumeReply::Completed => sse_response(&status_frame(nasiko_types::a2a::completed(
            &task_id,
            &context_id,
        ))),
    }
}

/// Starts the fake agent and returns its URL and a counter of the requests it has served.
async fn start_fake_agent(
    resume_reply: ResumeReply,
    pauses_first: bool,
) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/", post(fake_agent_handler))
        .with_state(FakeAgent {
            resume_reply,
            pauses_first,
            calls: calls.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), calls)
}

// ─── Metric reading ──────────────────────────────────────────────────────────

/// Lets the test keep a handle on the reader after handing it to the provider, which takes
/// ownership of whatever reader it is given.
#[derive(Debug)]
struct SharedReader(Arc<ManualReader>);

impl MetricReader for SharedReader {
    fn register_pipeline(&self, pipeline: Weak<Pipeline>) {
        self.0.register_pipeline(pipeline)
    }
    fn collect(&self, rm: &mut ResourceMetrics) -> MetricResult<()> {
        self.0.collect(rm)
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }
    fn shutdown(&self) -> OTelSdkResult {
        self.0.shutdown()
    }
    fn temporality(&self, kind: InstrumentKind) -> Temporality {
        self.0.temporality(kind)
    }
}

/// The reader behind the process-wide meter provider. Installed on first use and before any
/// `AppState` is built: `GenAiMetrics::new` resolves its instruments from the global provider
/// once, so a provider installed afterwards would leave them as no-ops.
fn metrics_reader() -> &'static Arc<ManualReader> {
    static READER: OnceLock<Arc<ManualReader>> = OnceLock::new();
    READER.get_or_init(|| {
        let reader = Arc::new(ManualReader::builder().build());
        opentelemetry::global::set_meter_provider(
            SdkMeterProvider::builder()
                .with_reader(SharedReader(reader.clone()))
                .build(),
        );
        reader
    })
}

fn collect_metrics() -> ResourceMetrics {
    let mut collected = ResourceMetrics {
        resource: Resource::builder_empty().build(),
        scope_metrics: Vec::new(),
    };
    metrics_reader()
        .collect(&mut collected)
        .expect("collect metrics");
    collected
}

fn has_attributes(point_attributes: &[KeyValue], wanted: &[(&str, &str)]) -> bool {
    wanted.iter().all(|(key, value)| {
        point_attributes
            .iter()
            .any(|kv| kv.key.as_str() == *key && kv.value.as_str() == *value)
    })
}

/// The summed value of counter `name` over every data point carrying all of `wanted`.
fn counter_total(name: &str, wanted: &[(&str, &str)]) -> u64 {
    collect_metrics()
        .scope_metrics
        .iter()
        .flat_map(|scope| &scope.metrics)
        .filter(|metric| metric.name == name)
        .filter_map(|metric| metric.data.as_any().downcast_ref::<Sum<u64>>())
        .flat_map(|sum| &sum.data_points)
        .filter(|point| has_attributes(&point.attributes, wanted))
        .map(|point| point.value)
        .sum()
}

/// `(recorded values, their sum)` of histogram `name` over every data point carrying `wanted`.
fn histogram_totals(name: &str, wanted: &[(&str, &str)]) -> (u64, f64) {
    collect_metrics()
        .scope_metrics
        .iter()
        .flat_map(|scope| &scope.metrics)
        .filter(|metric| metric.name == name)
        .filter_map(|metric| metric.data.as_any().downcast_ref::<Histogram<f64>>())
        .flat_map(|histogram| &histogram.data_points)
        .filter(|point| has_attributes(&point.attributes, wanted))
        .fold((0, 0.0), |(count, sum), point| {
            (count + point.count, sum + point.sum)
        })
}

/// Polls `read` until it returns `Some`, or panics. The outcome is metered right after the
/// delivery attempt that recorded it returns, a moment after the row itself turns terminal.
async fn wait_for<T>(what: &str, mut read: impl FnMut() -> Option<T>) -> T {
    for _ in 0..100 {
        if let Some(value) = read() {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

// ─── Fixtures ────────────────────────────────────────────────────────────────

async fn start_server() -> common::TestServer {
    // Before the server: see `metrics_reader`.
    metrics_reader();
    common::TestServer::start().await
}

fn auth(rb: reqwest::RequestBuilder, user_id: Uuid) -> reqwest::RequestBuilder {
    let user_id_str = user_id.to_string();
    common::as_member(rb, &user_id_str, &format!("user_{}", &user_id_str[..8]))
}

async fn seed_user(server: &common::TestServer) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("user_{}", &user_id.to_string()[..8]))
        .bind(format!("user_{}@test.example", &user_id.to_string()[..8]))
        .execute(&server.db)
        .await
        .expect("seed_user");
    user_id
}

async fn seed_running_agent(server: &common::TestServer, owner_id: Uuid, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url, status) VALUES ($1, $2, $3, 'running') RETURNING id",
    )
    .bind(format!("lost-task-agent-{}", Uuid::new_v4()))
    .bind(owner_id)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .expect("seed_running_agent")
}

/// A `pending` `input_required` row on a direct chat, as `persist_direct_chat_pause` writes it,
/// for the tests that do not need to exercise pause detection.
async fn seed_pending_request(server: &common::TestServer, agent_id: Uuid, owner_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, expires_at)
         VALUES ('input_required', 'direct_chat', $1, $2, $3, $4, $5, now() + interval '1 day')
         RETURNING id",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("ctx-{}", Uuid::new_v4()))
    .bind(json!({"message": "Which region?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed_pending_request")
}

async fn answer(server: &common::TestServer, user_id: Uuid, hitl_id: Uuid) {
    let response = auth(
        server
            .client
            .post(server.url(&format!("/api/hitl/{hitl_id}/resolve")))
            .json(&json!({"answer": "eu-west-1"})),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
}

struct ResumeRow {
    resume_status: String,
    resume_last_error: Option<String>,
    resume_dispatch_attempts: i32,
    leased: bool,
}

/// Polls until the dispatcher has recorded a terminal outcome (`resume_status` leaves
/// `not_started`), or panics. Budget: a retryable failure runs out its five attempts in about
/// ten seconds (each retry waits for the dispatcher's two second tick).
async fn wait_for_terminal_resume(server: &common::TestServer, id: Uuid) -> ResumeRow {
    for _ in 0..200 {
        let row = fetch_resume_row(server, id).await;
        if row.resume_status != "not_started" {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("hitl request {id} never left resume_status = not_started");
}

async fn fetch_resume_row(server: &common::TestServer, id: Uuid) -> ResumeRow {
    let (resume_status, resume_last_error, resume_dispatch_attempts, leased): (
        String,
        Option<String>,
        i32,
        bool,
    ) = sqlx::query_as(
        "SELECT resume_status, resume_last_error, resume_dispatch_attempts,
                resume_claimed_at IS NOT NULL
           FROM hitl_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    ResumeRow {
        resume_status,
        resume_last_error,
        resume_dispatch_attempts,
        leased,
    }
}

/// Seeds a paused direct-chat task, answers it, and returns the terminal row the dispatcher
/// recorded for the given way of answering the resume.
async fn resume_with(
    reply: ResumeReply,
) -> (ResumeRow, Uuid, Arc<AtomicUsize>, common::TestServer) {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let (agent_url, calls) = start_fake_agent(reply, false).await;
    let agent_id = seed_running_agent(&server, user_id, &agent_url).await;
    let hitl_id = seed_pending_request(&server, agent_id, user_id).await;

    answer(&server, user_id, hitl_id).await;
    let row = wait_for_terminal_resume(&server, hitl_id).await;
    (row, hitl_id, calls, server)
}

// ─── The outcome ─────────────────────────────────────────────────────────────

/// `HITL_LOST_TASK_RETRIES` in the test config (`common::test_config`) plus the first POST.
const LOST_TASK_POSTS: usize = 4;

/// Re-delivery: the agent that said "no such task" twice has the task on the third try.
#[tokio::test]
#[serial]
async fn a_task_found_on_a_later_delivery_completes_the_resume() {
    let (row, _, calls, server) = resume_with(ResumeReply::LostThenCompleted(2)).await;

    assert_eq!(row.resume_status, "completed");
    assert_eq!(row.resume_last_error, None);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    server.cleanup().await;
}

/// A bare 404 (an HTML page from a proxy) is not the agent reporting anything about the task:
/// the row keeps the old retryable-failure handling and is never concluded lost.
#[tokio::test]
#[serial]
async fn an_html_404_is_retried_as_an_ordinary_failure_not_concluded_lost() {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let (agent_url, calls) = start_fake_agent(ResumeReply::Html404, false).await;
    let agent_id = seed_running_agent(&server, user_id, &agent_url).await;
    let hitl_id = seed_pending_request(&server, agent_id, user_id).await;

    answer(&server, user_id, hitl_id).await;
    // The dispatcher's next tick claims the row again after a retryable failure.
    wait_for("the HTML 404 to be retried", || {
        (calls.load(Ordering::SeqCst) >= 2).then_some(())
    })
    .await;

    let row = fetch_resume_row(&server, hitl_id).await;
    assert_ne!(row.resume_status, "agent_lost_task");
    assert!(
        calls.load(Ordering::SeqCst) <= row.resume_dispatch_attempts as usize,
        "at most one POST per claim: no re-delivery loop for a 404 that is not task-not-found"
    );
    assert!(
        row.resume_last_error
            .as_deref()
            .is_some_and(|e| e.contains("agent HTTP 404")),
        "{:?}",
        row.resume_last_error
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_task_not_found_stream_frame_is_recorded_as_agent_lost_task() {
    let (row, _, calls, server) = resume_with(ResumeReply::TaskNotFoundFrame).await;

    assert_eq!(row.resume_status, "agent_lost_task");
    assert!(!row.leased, "a terminal row must not keep its lease");
    assert!(
        row.resume_last_error
            .as_deref()
            .is_some_and(|e| e.contains("-32001") && e.contains("4 delivery attempts")),
        "the signal and the attempt count should be recorded: {:?}",
        row.resume_last_error
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        LOST_TASK_POSTS,
        "lost only after the initial POST plus every re-delivery was answered task-not-found"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_task_not_found_json_body_is_recorded_as_agent_lost_task() {
    let (row, _, calls, server) = resume_with(ResumeReply::TaskNotFoundJsonBody).await;

    assert_eq!(row.resume_status, "agent_lost_task");
    assert!(!row.leased);
    // Exactly one POST per attempt: the first -32001 body must not be answered with the
    // `message/send` retry (a different request that could fail differently and mask it).
    assert_eq!(calls.load(Ordering::SeqCst), LOST_TASK_POSTS);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_http_404_carrying_task_not_found_is_recorded_as_agent_lost_task_and_not_reclaimed() {
    let (row, hitl_id, calls, server) = resume_with(ResumeReply::TaskNotFoundHttp404).await;

    assert_eq!(row.resume_status, "agent_lost_task");
    assert!(!row.leased);
    assert_eq!(
        row.resume_dispatch_attempts, 1,
        "the re-deliveries happen inside one claim; the row is never claimed again"
    );
    assert_eq!(calls.load(Ordering::SeqCst), LOST_TASK_POSTS);

    // Let the dispatcher tick past where a retry would have happened: the row stays put and the
    // agent is not asked again.
    let calls_at_terminal = calls.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(5)).await;
    let later = fetch_resume_row(&server, hitl_id).await;
    assert_eq!(later.resume_status, "agent_lost_task");
    assert_eq!(later.resume_dispatch_attempts, 1);
    assert_eq!(calls.load(Ordering::SeqCst), calls_at_terminal);

    server.cleanup().await;
}

/// What `orchestrator_stream` leaves behind when a delegated agent pauses, seeded directly: the
/// chat session, the paused turn's flow and its step stuck at `awaiting_human`, and the
/// orchestrator-origin request. Returns `(request id, chat session id, flow step id)`.
async fn seed_paused_orchestrator_turn(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
) -> (Uuid, String, Uuid) {
    let agent_name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let session_id = format!("ses_{}", Uuid::new_v4().simple());
    let flow_id = Uuid::new_v4().simple().to_string();

    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, NULL, '/api/orchestrator/a2a', 'lost task')",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(&server.db)
    .await
    .expect("seed chat_sessions");
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, session_id) \
         VALUES ($1, $2, 'orchestrator', 'paused turn', 'paused', $3)",
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(&session_id)
    .execute(&server.db)
    .await
    .expect("seed flows");
    let step_id: Uuid = sqlx::query_scalar(
        "INSERT INTO flow_steps \
            (flow_id, step_order, depth, agent_name, caller_agent_name, input_summary, status) \
         VALUES ($1, 1, 1, $2, 'orchestrator', 'what is the weather?', 'awaiting_human') \
         RETURNING id",
    )
    .bind(&flow_id)
    .bind(nasiko_react_agent::A2aTool::agent_display_name(&agent_name))
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
    .bind(&session_id)
    .bind(json!({"message": "Which region?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed hitl_requests");

    (hitl_id, session_id, step_id)
}

/// A completed resume of an orchestrator-origin request hands the agent's reply back to the
/// orchestrator for a follow-up turn, seeded with a note the user never typed ("the agent has
/// already completed the step"). When the agent lost the task there is no reply and the step did
/// not happen, so that turn must not run; the paused step still must not be left waiting on a
/// human who has answered.
#[tokio::test]
#[serial]
async fn an_orchestrator_resume_that_lost_its_task_starts_no_follow_up_turn() {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let (agent_url, _) = start_fake_agent(ResumeReply::TaskNotFoundFrame, false).await;
    let agent_id = seed_running_agent(&server, user_id, &agent_url).await;
    let (hitl_id, session_id, step_id) =
        seed_paused_orchestrator_turn(&server, user_id, agent_id).await;

    answer(&server, user_id, hitl_id).await;
    let row = wait_for_terminal_resume(&server, hitl_id).await;
    assert_eq!(row.resume_status, "agent_lost_task");

    let step_status = async || -> String {
        sqlx::query_scalar("SELECT status FROM flow_steps WHERE id = $1")
            .bind(step_id)
            .fetch_one(&server.db)
            .await
            .unwrap()
    };
    for _ in 0..100 {
        if step_status().await != "awaiting_human" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // `failed`, not `resumed`: the answer reached no task, so the step never ran on.
    assert_eq!(step_status().await, "failed");

    // The only transcript row is the human's own answer, which `resolve` saves. A follow-up turn
    // would have added its note (role `system`) before doing anything else.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT role FROM chat_messages WHERE session_id = $1 ORDER BY timestamp",
    )
    .bind(&session_id)
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(roles, ["user"]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_json_rpc_error_that_is_not_task_not_found_is_still_recorded_as_delivered() {
    let (row, _, _, server) = resume_with(ResumeReply::OtherJsonRpcError).await;

    assert_eq!(
        row.resume_status, "completed",
        "only a lost task gets the new outcome; any other agent error keeps its handling"
    );

    server.cleanup().await;
}

/// The reply broke after HTTP 200, so the agent may have acted: the row is quarantined rather than
/// retried, and the transport error is kept so an operator can see why.
#[tokio::test]
#[serial]
async fn a_stream_that_breaks_mid_body_records_why_the_outcome_is_unknown() {
    let (row, _, calls, server) = resume_with(ResumeReply::StreamBreaksMidBody).await;

    assert_eq!(row.resume_status, "delivery_outcome_unknown");
    let reason = row.resume_last_error.expect("the reason must be recorded");
    assert!(
        reason.contains("agent stream broke"),
        "unexpected reason: {reason}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an unknown outcome is never retried"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_hitl_api_reports_the_outcome() {
    let (row, hitl_id, _, server) = resume_with(ResumeReply::TaskNotFoundFrame).await;
    assert_eq!(row.resume_status, "agent_lost_task");
    let user_id: Uuid = sqlx::query_scalar("SELECT owner_user_id FROM hitl_requests WHERE id = $1")
        .bind(hitl_id)
        .fetch_one(&server.db)
        .await
        .unwrap();

    let body: Value = auth(
        server
            .client
            .get(server.url(&format!("/api/hitl/{hitl_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["status"], "resolved");
    assert_eq!(body["resume_status"], "agent_lost_task");

    server.cleanup().await;
}

/// `GET /api/hitl/{id}/stream` closes once the row reaches a terminal state. A status the stream
/// does not know is terminal would leave every client that reconnects to a lost task waiting
/// forever.
#[tokio::test]
#[serial]
async fn the_hitl_stream_closes_on_a_lost_task() {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let agent_id = seed_running_agent(&server, user_id, "http://127.0.0.1:1").await;
    let hitl_id = seed_pending_request(&server, agent_id, user_id).await;
    sqlx::query(
        "UPDATE hitl_requests
            SET status = 'resolved', resolved_at = now(), resume_status = 'agent_lost_task'
          WHERE id = $1",
    )
    .bind(hitl_id)
    .execute(&server.db)
    .await
    .unwrap();

    let body = tokio::time::timeout(
        Duration::from_secs(10),
        auth(
            server
                .client
                .get(server.url(&format!("/api/hitl/{hitl_id}/stream"))),
            user_id,
        )
        .send()
        .await
        .unwrap()
        .text(),
    )
    .await
    .expect("the stream must end on its own once the row is terminal")
    .unwrap();

    assert!(
        body.contains("\"resume_status\":\"agent_lost_task\""),
        "the stream should have reported the outcome before closing: {body}"
    );

    server.cleanup().await;
}

// ─── The metrics ─────────────────────────────────────────────────────────────

/// Starts a direct chat that pauses, answers the pause, and returns the row's id once the
/// dispatcher has recorded a terminal outcome for the resume.
async fn pause_and_answer(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
) -> (Uuid, ResumeRow) {
    let session_id = format!("ses_{}", Uuid::new_v4().simple());
    let turn = json!({
        "jsonrpc": "2.0", "id": Uuid::new_v4().to_string(), "method": "message/stream",
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(), "contextId": session_id,
                "role": "ROLE_USER", "parts": [{"text": "deploy it"}],
            },
            "metadata": {"agent_id": agent_id.to_string(), "session_id": session_id},
        },
    });
    // Reading the whole body is what drives the stream to the pause and persists the row.
    let response = auth(
        server
            .client
            .post(server.url("/api/orchestrator/a2a"))
            .json(&turn),
        user_id,
    )
    .send()
    .await
    .unwrap();
    response.text().await.unwrap();

    let hitl_id: Uuid = sqlx::query_scalar("SELECT id FROM hitl_requests WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .expect("the pause must have created exactly one hitl_requests row");

    answer(server, user_id, hitl_id).await;
    (hitl_id, wait_for_terminal_resume(server, hitl_id).await)
}

#[tokio::test]
#[serial]
async fn a_lost_task_is_metered_from_the_pause_to_the_outcome() {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let (agent_url, _) = start_fake_agent(ResumeReply::TaskNotFoundFrame, true).await;
    let agent_id = seed_running_agent(&server, user_id, &agent_url).await;
    let agent = agent_id.to_string();

    let (_, row) = pause_and_answer(&server, user_id, agent_id).await;
    assert_eq!(row.resume_status, "agent_lost_task");

    let pause = [
        ("nasiko.agent.id", agent.as_str()),
        ("kind", "input_required"),
        ("origin", "direct_chat"),
    ];
    assert_eq!(counter_total("nasiko.hitl.pauses", &pause), 1);

    let outcome = [
        ("nasiko.agent.id", agent.as_str()),
        ("outcome", "agent_lost_task"),
    ];
    wait_for("the resume outcome to be metered", || {
        (counter_total("nasiko.hitl.resume_outcomes", &outcome) == 1).then_some(())
    })
    .await;

    let (answers, answer_secs) = histogram_totals(
        "nasiko.hitl.human_response_duration",
        &[("nasiko.agent.id", agent.as_str())],
    );
    assert_eq!(answers, 1);
    assert!(
        (0.0..120.0).contains(&answer_secs),
        "pause to answer took {answer_secs}s in this test"
    );

    let (resumes, resume_secs) = histogram_totals(
        "nasiko.hitl.resume_duration",
        &[
            ("nasiko.agent.id", agent.as_str()),
            ("outcome", "agent_lost_task"),
        ],
    );
    assert_eq!(resumes, 1);
    assert!(
        (0.0..120.0).contains(&resume_secs),
        "answer to outcome took {resume_secs}s in this test"
    );

    server.cleanup().await;
}

/// The outcome counter is not specific to a lost task: a resume that completes is metered under
/// its own `outcome`, and nothing is counted under the lost-task one.
#[tokio::test]
#[serial]
async fn a_completed_resume_is_metered_as_completed() {
    let server = start_server().await;
    let user_id = seed_user(&server).await;
    let (agent_url, _) = start_fake_agent(ResumeReply::Completed, true).await;
    let agent_id = seed_running_agent(&server, user_id, &agent_url).await;
    let agent = agent_id.to_string();

    let (_, row) = pause_and_answer(&server, user_id, agent_id).await;
    assert_eq!(row.resume_status, "completed");

    let completed = [
        ("nasiko.agent.id", agent.as_str()),
        ("outcome", "completed"),
    ];
    wait_for("the resume outcome to be metered", || {
        (counter_total("nasiko.hitl.resume_outcomes", &completed) == 1).then_some(())
    })
    .await;
    let lost = [
        ("nasiko.agent.id", agent.as_str()),
        ("outcome", "agent_lost_task"),
    ];
    assert_eq!(counter_total("nasiko.hitl.resume_outcomes", &lost), 0);

    server.cleanup().await;
}
