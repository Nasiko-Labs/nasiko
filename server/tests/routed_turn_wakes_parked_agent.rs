//! A routed (ReAct) turn offers parked agents as candidates and wakes only the one the model
//! picks: `CpCallGuard::before_call`, past the ACL and flow guard, resolves the leg's target and
//! hands it to the call through its handle. The wake is the state's (`common::RowWake` here),
//! which the server resolver asks before it resolves.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test routed_turn_wakes_parked_agent -- --test-threads=1

mod common;

use std::sync::Arc;

use axum::{Json, Router, routing::post};
use nasiko_flow::FlowContext;
use nasiko_react_agent::CallGuard;
use nasiko_server::acl::{CpCallGuard, TurnCandidates, TurnLegs};
use nasiko_server::agent_resolver::ServerTargetResolver;
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

async fn seed_parked(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy, state_mode) \
         VALUES ($1, $2, 'x:1.0.0', 'parked', '', 'auto', 'durable') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

struct Unmetered;

impl nasiko_types::agent_call::AgentCallMeter for Unmetered {
    fn begin(&self, _agent_id: Uuid) -> Box<dyn nasiko_types::agent_call::AgentCallHandle> {
        Box::new(nasiko_types::agent_call::UnmeteredCall)
    }
}

async fn guard_for(server: &common::TestServer, owner: Uuid, root: Uuid) -> Arc<CpCallGuard> {
    guard_with(server, owner, root, &[]).await
}

/// A guard whose turn offered `candidates` (`name`, `id`) plus the `root` agent by its own name.
async fn guard_with(
    server: &common::TestServer,
    owner: Uuid,
    root: Uuid,
    candidates: &[(&str, Uuid)],
) -> Arc<CpCallGuard> {
    let root_name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
        .bind(root)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let (flow_id, _) = common::open_flow(&server.db, owner, root).await;
    let ctx = FlowContext {
        flow_id,
        parent_span_id: FlowContext::generate_span_id(),
    };
    server
        .state
        .flow_guard
        .init_flow(&ctx, "orchestrator")
        .await;
    CpCallGuard::new(
        server.db.clone(),
        server.state.flow_guard.clone(),
        ctx,
        None,
        Arc::new(nasiko_server::spend_gate::NoSpendGate),
        owner,
        String::new(),
        Arc::new(Unmetered),
        Arc::new(nasiko_server::agent_wake::NoAgentWake),
        TurnLegs {
            targets: Arc::new(ServerTargetResolver::new(server.state.clone())),
            user_id: owner,
            candidates: TurnCandidates::new(
                candidates
                    .iter()
                    .copied()
                    .chain(std::iter::once((root_name.as_str(), root))),
            ),
        },
    )
}

#[tokio::test]
#[serial]
async fn only_the_agent_the_model_picks_is_woken_and_its_address_rides_the_handle() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let picked = seed_parked(&server, owner, "picked-agent").await;
    let idle = seed_parked(&server, owner, "idle-agent").await;
    let stub = {
        let app = Router::new().route("/", post(|| async { Json(json!({})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    };
    wake.serve_at(picked, &stub);
    wake.serve_at(idle, &stub);
    let guard = guard_for(&server, owner, picked).await;

    let handle = guard.before_call("picked-agent").await.expect("admitted");

    let target = handle.target().expect("the resolved address");
    assert_eq!(target.url, stub);
    assert_eq!(wake.woken_count(picked), 1);
    assert_eq!(wake.woken_count(idle), 0);
    let status: String = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(picked)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_leg_whose_agent_cannot_be_woken_is_blocked_with_the_reason() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    // No address for the agent: the wake cannot start it.
    common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_parked(&server, owner, "stuck-agent").await;
    let guard = guard_for(&server, owner, agent).await;

    let blocked = guard
        .before_call("stuck-agent")
        .await
        .err()
        .expect("blocked");

    assert!(blocked.contains("unavailable"), "{blocked}");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_blocked_leg_undoes_its_flow_guard_invocation() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_parked(&server, owner, "retry-agent").await;
    let guard = guard_for(&server, owner, agent).await;

    // The first wake fails: the agent has nowhere to serve yet.
    assert!(guard.before_call("retry-agent").await.is_err());
    wake.serve_at(agent, "http://woken.agent:8000");
    // Had the first attempt's invocation stayed on the call stack, this one would be a cycle.
    assert!(
        guard.before_call("retry-agent").await.is_ok(),
        "the failed leg's invocation was not unwound"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_same_named_agent_of_another_owner_is_never_reached() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let mine_owner = init_admin(&server).await;
    let other_owner: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('other', 'other@t.io') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    // Names are unique only per owner: both own an agent called "shared-name".
    let theirs = seed_parked(&server, other_owner, "shared-name").await;
    let mine = seed_parked(&server, mine_owner, "shared-name").await;
    wake.serve_at(mine, "http://mine.agent:8000");
    wake.serve_at(theirs, "http://theirs.agent:8000");
    // The user's turn was offered only their own agent.
    let guard = guard_with(&server, mine_owner, mine, &[]).await;

    let handle = guard.before_call("shared-name").await.expect("admitted");

    assert_eq!(handle.target().unwrap().url, "http://mine.agent:8000");
    assert_eq!(wake.woken_count(mine), 1);
    assert_eq!(wake.woken_count(theirs), 0);
    // Something that was not offered is refused outright, never looked up by name.
    assert!(guard.before_call("not-offered").await.is_err());
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_whose_name_has_underscores_and_dots_is_reachable() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_parked(&server, owner, "my_agent.v2").await;
    wake.serve_at(agent, "http://dotted.agent:8000");
    let guard = guard_for(&server, owner, agent).await;

    // The ReAct loop hands the guard `tool_name` with its prefix stripped and `_` read as `-`.
    let display = nasiko_react_agent::A2aTool::agent_display_name("my_agent.v2");
    assert_eq!(display, "my-agent-v2");
    let handle = guard.before_call(&display).await.expect("admitted");

    assert_eq!(handle.target().unwrap().url, "http://dotted.agent:8000");
    server.cleanup().await;
}

#[test]
fn two_candidates_folding_to_one_display_name_are_both_refused() {
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let candidates = TurnCandidates::new([("a.b", a), ("a-b", b), ("c", c)]);
    assert_eq!(candidates.get("a-b"), None);
    assert!(candidates.is_ambiguous("a.b") && candidates.is_ambiguous("a-b"));
    assert_eq!(candidates.get("c"), Some(c));
    assert!(!candidates.is_ambiguous("c"));
}

#[tokio::test]
#[serial]
async fn an_ambiguous_name_gets_its_own_refusal_and_a_continuation_reaches_nobody() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_parked(&server, owner, "solo").await;
    wake.serve_at(agent, "http://solo.agent:8000");
    let guard = guard_with(&server, owner, agent, &[("a.b", agent), ("a-b", agent)]).await;
    let refusal = guard.before_call("a-b").await.err().expect("refused");
    assert!(refusal.contains("ambiguous"), "{refusal}");

    // A resumed turn is given an empty candidate table: nothing it names can be called.
    let (flow_id, _) = common::open_flow(&server.db, owner, agent).await;
    let continuation = CpCallGuard::new(
        server.db.clone(),
        server.state.flow_guard.clone(),
        FlowContext {
            flow_id,
            parent_span_id: FlowContext::generate_span_id(),
        },
        None,
        Arc::new(nasiko_server::spend_gate::NoSpendGate),
        owner,
        String::new(),
        Arc::new(Unmetered),
        Arc::new(nasiko_server::agent_wake::NoAgentWake),
        TurnLegs {
            targets: Arc::new(ServerTargetResolver::new(server.state.clone())),
            user_id: owner,
            candidates: TurnCandidates::default(),
        },
    );
    assert!(continuation.before_call("solo").await.is_err());
    assert_eq!(wake.woken_count(agent), 0);
    server.cleanup().await;
}

/// One streamed OpenAI chunk carrying a complete tool call.
fn tool_call_chunk(tool_name: &str, message: &str) -> String {
    let arguments = json!({ "message": message }).to_string();
    let chunk = json!({ "id": "c1", "object": "chat.completion.chunk", "created": 0, "model": "gpt-4o", "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [{
        "index": 0, "id": "call_1", "type": "function",
        "function": { "name": tool_name, "arguments": arguments }
    }]}, "finish_reason": "tool_calls" }]});
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

fn text_chunk(text: &str) -> String {
    let chunk = json!({ "id": "c2", "object": "chat.completion.chunk", "created": 0, "model": "gpt-4o", "choices": [{ "index": 0, "delta": { "role": "assistant", "content": text }, "finish_reason": "stop" }] });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// The whole routed path: `orchestrator_stream` lists a parked agent as a `nasiko-leg://`
/// candidate, the (mocked) model picks it, the guard wakes it and the call lands on its workload.
#[tokio::test]
#[serial]
async fn a_routed_turn_offers_a_parked_candidate_and_the_call_reaches_its_workload() {
    let mut llm = mockito::Server::new_async().await;
    let picked = llm
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(tool_call_chunk("call_agent_sleepy_agent", "wake up"))
        .expect(1)
        .create_async()
        .await;
    let _answer = llm
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(text_chunk("all done"))
        .create_async()
        .await;
    // SAFETY: tests here run serially (`#[serial]`), so no other thread reads the environment.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", llm.url());
        std::env::set_var("OPENAI_API_KEY", "test-key-routed-turn");
    }
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let wake = common::RowWake::install(&server);
    let owner = init_admin(&server).await;
    let agent = seed_parked(&server, owner, "sleepy-agent").await;

    let mut woken_agent = mockito::Server::new_async().await;
    let reached = woken_agent
        .mock("POST", "/")
        .with_status(200)
        .with_body(
            json!({"jsonrpc": "2.0", "id": "1",
                   "result": {"kind": "message", "parts": [{"kind": "text", "text": "awake"}]}})
            .to_string(),
        )
        .expect_at_least(1)
        .create_async()
        .await;
    wake.serve_at(agent, &woken_agent.url());

    let response = common::as_superuser(
        server.client.post(server.url("/api/orchestrator/a2a")),
        &owner.to_string(),
        "admin",
    )
    .json(&json!({
        "jsonrpc": "2.0", "method": "message/stream", "id": "1",
        "params": {
            "message": {
                "messageId": Uuid::new_v4().to_string(),
                "contextId": "routed-ctx-1",
                "role": "ROLE_USER",
                "parts": [{ "text": "please wake the sleepy agent" }]
            },
            "metadata": { "agent_id": "orchestrator" }
        }
    }))
    .send()
    .await
    .unwrap();
    let body = response.text().await.unwrap();

    picked.assert_async().await;
    reached.assert_async().await;
    assert_eq!(wake.woken_count(agent), 1, "{body}");
    let status: String = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "running");
    server.cleanup().await;
}
