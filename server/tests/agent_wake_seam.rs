//! The `AgentWake` seam (`oss/server/src/agent_wake.rs`) on every path that forwards a call to an
//! agent (the agent proxy, direct dispatch, a routed orchestrator leg, the HITL resume, and the
//! `ServerTargetResolver` MAF steps and HITL nudges resolve through): a seam that refuses must stop
//! the call before anything reaches the agent, it is asked before the agent's address is resolved
//! (a parked agent has none until it is woken), and it is asked only about an agent that is really
//! about to be called, never about one the orchestrator merely lists.
//!
//! Requires infra (Postgres, Redis, S3) like the rest of the suite:
//!   cargo test -p nasiko-server --test agent_wake_seam -- --test-threads=1

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, extract::Request, response::IntoResponse, routing::post};
use nasiko_flow::FlowContext;
use nasiko_hitl::{AgentTargetResolver, ResolveCaller, ResolveFailure};
use nasiko_react_agent::CallGuard;
use nasiko_server::acl::{CpCallGuard, TurnCandidates, TurnLegs};
use nasiko_server::agent_resolver::ServerTargetResolver;
use nasiko_server::agent_wake::AgentWake;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const REFUSAL: &str = "the agent is not ready";

/// Refuses while `refuse` is set, and records every agent it was asked about.
struct ScriptedWake {
    refuse: AtomicBool,
    asked: Mutex<Vec<Uuid>>,
}

impl ScriptedWake {
    fn refusing() -> Arc<Self> {
        Arc::new(Self {
            refuse: AtomicBool::new(true),
            asked: Mutex::default(),
        })
    }

    fn allowing() -> Arc<Self> {
        Arc::new(Self {
            refuse: AtomicBool::new(false),
            asked: Mutex::default(),
        })
    }

    fn asked(&self) -> Vec<Uuid> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentWake for ScriptedWake {
    async fn before_forward(&self, agent_id: Uuid, _flow_id: Option<&str>) -> Result<(), String> {
        self.asked.lock().unwrap().push(agent_id);
        if self.refuse.load(Ordering::SeqCst) {
            return Err(REFUSAL.to_string());
        }
        Ok(())
    }
}

/// A stub agent that counts the requests that reach it.
async fn start_counting_agent() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let a2a = post(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Json(json!({
                "jsonrpc": "2.0", "id": "1",
                "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]}
            }))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", a2a))
            .await
            .unwrap();
    });
    (format!("http://127.0.0.1:{port}"), hits)
}

async fn seed_owner(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("owner_{}", Uuid::new_v4().simple()))
        .bind(format!("{}@test.example", Uuid::new_v4().simple()))
        .fetch_one(&server.db)
        .await
        .expect("seed owner")
}

/// A running agent whose stored `url` is the stub: the fake runtime has no container for it, so
/// the forward falls back to that address.
async fn seed_running_agent(server: &common::TestServer, owner: Uuid, url: &str) -> Uuid {
    seed_named_agent(
        server,
        owner,
        &format!("wake-seam-{}", Uuid::new_v4().simple()),
        url,
    )
    .await
}

async fn seed_named_agent(server: &common::TestServer, owner: Uuid, name: &str, url: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url) \
         VALUES ($1, $2, 'x:1.0.0', 'running', $3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

/// A running agent with no address at all: no stored URL, and no container in the fake runtime.
/// Resolving it fails, so a refusal (rather than that failure) proves the seam was asked first.
async fn seed_agent_without_address(server: &common::TestServer, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'x:1.0.0', 'running') RETURNING id",
    )
    .bind(format!("wake-seam-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

fn message(method: &str, metadata: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": method,
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "contextId": format!("ctx-{}", Uuid::new_v4().simple()),
                "role": "ROLE_USER",
                "parts": [{"text": "hello"}]
            },
            "metadata": metadata
        }
    })
}

async fn post_as(
    server: &common::TestServer,
    owner: Uuid,
    path: &str,
    body: &Value,
) -> reqwest::Response {
    common::as_superuser(
        server.client.post(server.url(path)).json(body),
        &owner.to_string(),
        "wake-seam",
    )
    .send()
    .await
    .expect("request")
}

#[tokio::test]
#[serial]
async fn a_refusing_seam_answers_503_before_the_agent_proxy_forwards() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let (url, hits) = start_counting_agent().await;
    let agent = seed_running_agent(&server, owner, &url).await;
    let body = message("message/stream", json!({}));
    let path = format!("/api/agents/{agent}");

    let refused = post_as(&server, owner, &path, &body).await;

    assert_eq!(refused.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing reaches the agent");
    assert_eq!(
        wake.asked(),
        vec![agent],
        "the seam is asked about this agent's UUID"
    );

    // Positive control: the same request goes through once the seam allows it.
    wake.refuse.store(false, Ordering::SeqCst);
    let allowed = post_as(&server, owner, &path, &body).await;
    assert!(allowed.status().is_success(), "{}", allowed.status());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.asked(), vec![agent, agent]);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_refusing_seam_stops_a_direct_dispatch_before_the_agent_is_called() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let (url, hits) = start_counting_agent().await;
    let agent = seed_running_agent(&server, owner, &url).await;
    let body = message("message/stream", json!({"agent_id": agent.to_string()}));

    let refused = post_as(&server, owner, "/api/orchestrator/a2a", &body).await;

    assert_eq!(refused.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = refused.json().await.expect("json-rpc error body");
    assert_eq!(refused["error"]["message"], REFUSAL);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing reaches the agent");
    assert_eq!(
        wake.asked(),
        vec![agent],
        "the seam is asked about this agent's UUID"
    );

    // Positive control: the same dispatch goes through once the seam allows it.
    wake.refuse.store(false, Ordering::SeqCst);
    let allowed = post_as(&server, owner, "/api/orchestrator/a2a", &body).await;
    assert!(allowed.status().is_success(), "{}", allowed.status());
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    server.cleanup().await;
}

/// A parked agent has no endpoint until it is woken, so dispatch must ask the seam before it
/// resolves where the call goes: had it resolved first, this would be the resolver's failure.
#[tokio::test]
#[serial]
async fn a_direct_dispatch_asks_the_seam_before_resolving_the_agent() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let agent = seed_agent_without_address(&server, owner).await;
    let body = message("message/stream", json!({"agent_id": agent.to_string()}));

    let refused = post_as(&server, owner, "/api/orchestrator/a2a", &body).await;

    assert_eq!(refused.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = refused.json().await.expect("json-rpc error body");
    assert_eq!(refused["error"]["message"], REFUSAL);
    assert_eq!(wake.asked(), vec![agent]);

    server.cleanup().await;
}

/// MAF steps and HITL nudges reach agents only through `ServerTargetResolver`, so it asks the seam
/// before it resolves; a refusal there is a retryable 503 carrying the seam's message.
#[tokio::test]
#[serial]
async fn the_target_resolver_asks_the_seam_before_resolving_the_agent() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let agent = seed_agent_without_address(&server, owner).await;
    let resolver = ServerTargetResolver::new(server.state.clone());
    let caller = ResolveCaller {
        user_id: Some(owner),
        flow_id: Some("wake-seam-flow".into()),
    };

    let refused = resolver.resolve(agent, &caller, None).await.unwrap_err();

    assert_eq!(refused, ResolveFailure::new(503, REFUSAL));
    assert!(!refused.is_permanent());
    assert_eq!(wake.asked(), vec![agent]);

    // Positive control: once the seam allows it, the call goes on to resolution, which has
    // nowhere to send it.
    wake.refuse.store(false, Ordering::SeqCst);
    let unresolved = resolver.resolve(agent, &caller, None).await.unwrap_err();
    assert_eq!(unresolved.status, ResolveFailure::NO_ENDPOINT);
    assert_eq!(wake.asked(), vec![agent, agent]);

    server.cleanup().await;
}

/// A routed leg's guard asks the seam itself, ahead of spending flow budget, so the resolver it
/// hands the leg to must not ask again.
#[tokio::test]
#[serial]
async fn the_target_resolver_for_a_guarded_leg_leaves_the_seam_to_the_guard() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let (url, _hits) = start_counting_agent().await;
    let agent = seed_running_agent(&server, owner, &url).await;
    let resolver = ServerTargetResolver::after_caller_wake(server.state.clone());

    let resolved = resolver
        .resolve(agent, &ResolveCaller::default(), None)
        .await
        .expect("resolved without asking the seam");

    assert_eq!(resolved.url, url);
    assert!(wake.asked().is_empty());

    server.cleanup().await;
}

/// An OpenAI-compatible model: the first (streaming) turn calls `tool_name`, any later
/// (non-streaming) turn answers in text.
async fn start_tool_calling_llm(tool_name: String) -> String {
    let app = Router::new().fallback(move |req: Request| {
        let tool_name = tool_name.clone();
        async move {
            let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
                .await
                .unwrap_or_default();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or_default();
            if body["stream"] == json!(true) {
                let arguments = json!({ "message": "do the thing" }).to_string();
                let chunk = json!({"id": "cmpl-1", "model": "test", "choices": [{
                    "index": 0,
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": tool_name, "arguments": arguments}
                    }]},
                    "finish_reason": "tool_calls"
                }]});
                return format!("data: {chunk}\n\ndata: [DONE]\n\n").into_response();
            }
            Json(json!({
                "id": "cmpl-1", "object": "chat.completion", "created": 0, "model": "test",
                "choices": [{"index": 0, "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "All done."}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
            .into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

/// SAFETY: tests in this crate run serially (`#[serial]`), as `common::test_config` assumes.
unsafe fn set_openai_env(base_url: &str) {
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", base_url);
        std::env::set_var("OPENAI_API_KEY", "test-key-for-agent-wake-seam");
    }
}

/// A routed orchestrator turn builds a list of every agent the caller may use, then calls one.
/// Only the called agent is about to receive traffic, so only it may be put to the seam.
#[tokio::test]
#[serial]
async fn a_routed_turn_asks_the_seam_only_for_the_agent_it_calls() {
    let wake = ScriptedWake::allowing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let (called_url, called_hits) = start_counting_agent().await;
    let (first_url, first_hits) = start_counting_agent().await;
    let (second_url, second_hits) = start_counting_agent().await;
    let called = seed_named_agent(&server, owner, "wake-seam-called", &called_url).await;
    seed_named_agent(&server, owner, "wake-seam-listed-one", &first_url).await;
    seed_named_agent(&server, owner, "wake-seam-listed-two", &second_url).await;
    let llm_url = start_tool_calling_llm("call_agent_wake_seam_called".into()).await;
    unsafe { set_openai_env(&llm_url) };

    let turn = post_as(
        &server,
        owner,
        "/api/orchestrator/a2a",
        &message("message/stream", json!({"agent_id": "orchestrator"})),
    )
    .await;
    let status = turn.status();
    let body = turn.text().await.expect("turn body");

    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(called_hits.load(Ordering::SeqCst), 1, "the turn: {body}");
    assert_eq!(first_hits.load(Ordering::SeqCst), 0);
    assert_eq!(second_hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        wake.asked(),
        vec![called],
        "the seam is asked about the called agent once, and about no listed agent"
    );

    server.cleanup().await;
}

/// The routed leg is where `CpCallGuard::before_call` admits a call: a refusal there is an
/// ordinary refusal, and it must not have spent any flow budget, or a retry of the same agent in
/// the same flow would be refused as a cycle.
#[tokio::test]
#[serial]
async fn a_refused_routed_call_is_refused_without_spending_flow_budget() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start().await;
    let owner = seed_owner(&server).await;
    let name = "wake-seam-guarded";
    let agent = seed_named_agent(&server, owner, name, "http://127.0.0.1:1").await;
    let flow_ctx = FlowContext::new_root();
    server
        .state
        .flow_guard
        .init_flow(&flow_ctx, "orchestrator")
        .await;
    let guard = CpCallGuard::new(
        server.db.clone(),
        server.state.flow_guard.clone(),
        flow_ctx,
        None,
        server.state.spend_gate.clone(),
        owner,
        "wake-seam-session".to_string(),
        server.state.genai_metrics.agent_call_meter(),
        wake.clone(),
        TurnLegs {
            targets: Arc::new(ServerTargetResolver::new(server.state.clone())),
            user_id: owner,
            candidates: TurnCandidates::new(std::iter::once((name, agent))),
        },
    );

    let refused = guard.before_call(name).await.err();
    assert_eq!(refused.as_deref(), Some(REFUSAL));
    assert_eq!(wake.asked(), vec![agent]);

    // Positive control, and the proof the refusal spent nothing: had it recorded the call, this
    // second call to the same agent would be refused as a cycle.
    wake.refuse.store(false, Ordering::SeqCst);
    let admitted = guard.before_call(name).await;
    assert!(admitted.is_ok(), "{:?}", admitted.err());
    assert_eq!(wake.asked(), vec![agent, agent]);

    // A name that is not one of this turn's candidates is refused before the wake seam, so it is
    // never asked about (the candidate lookup now gates the leg, replacing the old name->DB resolve).
    let unresolved = guard.before_call("wake-seam-nobody").await;
    assert!(unresolved.is_err());
    assert_eq!(wake.asked(), vec![agent, agent]);

    server.cleanup().await;
}

async fn seed_pending_resume(server: &common::TestServer, agent: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, expires_at)
         VALUES ('input_required', 'direct_chat', $1, $2, $3, $4, $5, now() + interval '1 day')
         RETURNING id",
    )
    .bind(agent)
    .bind(owner)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("ctx-{}", Uuid::new_v4()))
    .bind(json!({"message": "what should I do?"}))
    .fetch_one(&server.db)
    .await
    .expect("seed hitl request")
}

/// The human's answer is delivered to the agent by the HITL dispatcher, which also forwards to an
/// agent. A refusal fails that delivery attempt with the seam's message (the row is retried), and
/// once the seam allows it the same row is delivered.
#[tokio::test]
#[serial]
async fn a_hitl_resume_asks_the_seam_before_delivering_to_the_agent() {
    let wake = ScriptedWake::refusing();
    let server = common::TestServer::start_with_agent_wake(wake.clone()).await;
    let owner = seed_owner(&server).await;
    let (url, hits) = start_counting_agent().await;
    let agent = seed_running_agent(&server, owner, &url).await;
    let hitl_id = seed_pending_resume(&server, agent, owner).await;

    let resolved = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/hitl/{hitl_id}/resolve")))
            .json(&json!({"answer": "go ahead"})),
        &owner.to_string(),
        "wake-seam",
    )
    .send()
    .await
    .expect("resolve");
    assert!(resolved.status().is_success(), "{}", resolved.status());

    let mut row: (String, i32, Option<String>) = Default::default();
    for _ in 0..100 {
        row = sqlx::query_as(
            "SELECT resume_status, resume_dispatch_attempts, resume_last_error \
             FROM hitl_requests WHERE id = $1",
        )
        .bind(hitl_id)
        .fetch_one(&server.db)
        .await
        .expect("read the resume row");
        if row.2.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let last_error = row
        .2
        .clone()
        .unwrap_or_else(|| panic!("the refused delivery was never recorded: {row:?}"));
    assert!(last_error.contains(REFUSAL), "{last_error}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing reaches the agent");
    assert_eq!(wake.asked().first(), Some(&agent));

    // Positive control: the dispatcher retries the row, and the allowed delivery reaches the agent.
    wake.refuse.store(false, Ordering::SeqCst);
    for _ in 0..200 {
        if hits.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the retried delivery");

    server.cleanup().await;
}
