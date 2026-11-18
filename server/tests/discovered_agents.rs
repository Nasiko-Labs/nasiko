//! Integration tests for A2A agents the CLI discovers on a user's machine.
//!
//! Covers:
//!   - POST /api/agents/discovered — registers a machine's running agents from their cards,
//!     keeps one row per agent across reports, marks agents no longer reported `stopped`, and
//!     never makes them `running` (so nothing routes chat to them)
//!   - GET /api/agents/discovered — reads a machine's agents back, as `nasiko status` shows them
//!   - Copies — two copies of one agent on a machine (one card name, two copy ids) are two
//!     agents, and a row registered before copy ids is taken over by the first copy
//!   - POST /api/agents/{id}/llm-token — a discovered agent gets a deployed agent's year-long key;
//!     DELETE revokes every key minted so far
//!   - POST /api/agents/{id}/mcp-token + /api/mcp — it gets an MCP gateway credential, and its
//!     flow-less `tools/call` acts as its owner, as a local coding agent's does
//!   - The LLM router — a discovered agent's call needs no traceparent and is billed to its
//!     owner, after which registration reports when it last made a call, and FinOps counts it
//!   - Sessions — each routed call is a turn of the agent's one chat session
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test discovered_agents -- --test-threads=1

mod common;

use chrono::{DateTime, Utc};
use common::TestServer;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const JWT_SECRET: &str = "discovered-agents-test-secret";

async fn seed_user(server: &TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@test.local"))
        .fetch_one(&server.db)
        .await
        .expect("seed user")
}

fn crew_card() -> Value {
    json!({
        "name": "crew-researcher",
        "description": "Find facts",
        "url": "http://127.0.0.1:8765/",
        "version": "1.0.0",
        "skills": [{"id": "researcher", "name": "Researcher", "description": "Find facts", "tags": ["research"]}]
    })
}

fn planner_card() -> Value {
    json!({
        "name": "planner",
        "supportedInterfaces": [{"url": "http://127.0.0.1:2024/a2a/a1", "protocolBinding": "JSONRPC"}],
        "skills": []
    })
}

async fn report(
    server: &TestServer,
    user: Uuid,
    username: &str,
    machine: &str,
    cards: &[Value],
) -> reqwest::Response {
    let agents: Vec<Value> = cards.iter().map(|card| json!({"card": card})).collect();
    common::as_member(
        server.client.post(server.url("/api/agents/discovered")),
        &user.to_string(),
        username,
    )
    .json(&json!({"machine": machine, "agents": agents}))
    .send()
    .await
    .expect("report discovered agents")
}

async fn report_ok(
    server: &TestServer,
    user: Uuid,
    username: &str,
    machine: &str,
    cards: &[Value],
) -> Value {
    let response = report(server, user, username, machine, cards).await;
    assert_eq!(response.status(), 200);
    response.json().await.expect("registration body")
}

fn entry<'a>(body: &'a Value, card_name: &str) -> &'a Value {
    body["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .find(|agent| agent["card_name"] == card_name)
        .unwrap_or_else(|| panic!("{card_name} in {body}"))
}

async fn status_of(server: &TestServer, id: &Value) -> String {
    sqlx::query_scalar("SELECT status FROM agents WHERE id = $1::uuid")
        .bind(id.as_str().expect("id"))
        .fetch_one(&server.db)
        .await
        .expect("agent status")
}

#[tokio::test]
#[serial]
async fn a_machines_agents_are_registered_kept_and_marked_stopped() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-owner").await;

    let first = report_ok(
        &server,
        user,
        "disc-owner",
        "laptop",
        &[crew_card(), planner_card()],
    )
    .await;
    let crew = entry(&first, "crew-researcher");
    let planner = entry(&first, "planner");
    assert_eq!(crew["created"], true);
    assert_eq!(crew["name"], "crew-researcher");
    assert!(crew["last_llm_call_at"].is_null());

    // The card is applied the way a deployed agent's is.
    let (description, discovered_on, status): (Option<String>, Option<String>, String) =
        sqlx::query_as("SELECT description, discovered_on, status FROM agents WHERE id = $1::uuid")
            .bind(crew["id"].as_str().unwrap())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(description.as_deref(), Some("Find facts"));
    assert_eq!(discovered_on.as_deref(), Some("laptop"));
    assert_eq!(
        status, "registered",
        "never 'running': nothing routes chat to it"
    );

    // A restart reports the same agent: same row. One the machine stops reporting is stopped.
    let second = report_ok(&server, user, "disc-owner", "laptop", &[crew_card()]).await;
    assert_eq!(entry(&second, "crew-researcher")["id"], crew["id"]);
    assert_eq!(entry(&second, "crew-researcher")["created"], false);
    assert_eq!(second["stopped"], 1);
    assert_eq!(status_of(&server, &planner["id"]).await, "stopped");

    // Reported again, it comes back as the same agent.
    let third = report_ok(
        &server,
        user,
        "disc-owner",
        "laptop",
        &[crew_card(), planner_card()],
    )
    .await;
    assert_eq!(entry(&third, "planner")["id"], planner["id"]);
    assert_eq!(status_of(&server, &planner["id"]).await, "registered");

    server.cleanup().await;
}

async fn listed(server: &TestServer, user: Uuid, username: &str, machine: &str) -> Value {
    let response = common::as_member(
        server
            .client
            .get(server.url("/api/agents/discovered"))
            .query(&[("machine", machine)]),
        &user.to_string(),
        username,
    )
    .send()
    .await
    .expect("list discovered agents");
    assert_eq!(response.status(), 200);
    response.json().await.expect("listing body")
}

#[tokio::test]
#[serial]
async fn a_machines_agents_are_listed_back_with_their_status() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-list").await;
    let other = seed_user(&server, "disc-list-other").await;
    report_ok(
        &server,
        user,
        "disc-list",
        "laptop",
        &[crew_card(), planner_card()],
    )
    .await;
    report_ok(&server, user, "disc-list", "laptop", &[crew_card()]).await;
    report_ok(&server, user, "disc-list", "desktop", &[planner_card()]).await;
    report_ok(&server, other, "disc-list-other", "laptop", &[crew_card()]).await;

    let laptop = listed(&server, user, "disc-list", "laptop").await;

    // Only this user's agents on this machine, the one no longer reported marked stopped.
    let agents = laptop["agents"].as_array().expect("agents");
    let summary: Vec<(&str, &str)> = agents
        .iter()
        .map(|a| {
            (
                a["card_name"].as_str().unwrap(),
                a["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [("crew-researcher", "registered"), ("planner", "stopped")]
    );
    assert!(agents[0]["last_llm_call_at"].is_null());
    assert!(agents[0]["updated_at"].is_string());

    let missing = common::as_member(
        server.client.get(server.url("/api/agents/discovered")),
        &user.to_string(),
        "disc-list",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(missing.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_same_agent_on_two_machines_is_two_agents() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-two").await;

    let laptop = report_ok(&server, user, "disc-two", "laptop", &[crew_card()]).await;
    let desktop = report_ok(&server, user, "disc-two", "desktop", &[crew_card()]).await;

    let (on_laptop, on_desktop) = (
        entry(&laptop, "crew-researcher"),
        entry(&desktop, "crew-researcher"),
    );
    assert_ne!(on_laptop["id"], on_desktop["id"]);
    assert_eq!(on_desktop["name"], "crew-researcher-desktop");
    // Each machine's report leaves the other machine's agents alone.
    assert_eq!(desktop["stopped"], 0);

    server.cleanup().await;
}

/// Report `copies` (card, copy id) from `machine`, as the CLI does.
async fn report_copies(
    server: &TestServer,
    user: Uuid,
    username: &str,
    machine: &str,
    copies: &[(Value, Option<&str>)],
) -> reqwest::Response {
    let agents: Vec<Value> = copies
        .iter()
        .map(|(card, instance)| json!({"card": card, "instance": instance}))
        .collect();
    common::as_member(
        server.client.post(server.url("/api/agents/discovered")),
        &user.to_string(),
        username,
    )
    .json(&json!({"machine": machine, "agents": agents}))
    .send()
    .await
    .expect("report discovered agents")
}

fn copy_entry<'a>(body: &'a Value, instance: &str) -> &'a Value {
    body["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .find(|agent| agent["instance"] == instance)
        .unwrap_or_else(|| panic!("copy {instance} in {body}"))
}

#[tokio::test]
#[serial]
async fn two_copies_of_an_agent_on_one_machine_are_two_agents() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-copies").await;
    let (a, b) = ("aaaa1111aaaa1111", "bbbb2222bbbb2222");
    let ok = |response: reqwest::Response| async move {
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.expect("registration body")
    };

    // Registered before copies had ids.
    let legacy = report_ok(&server, user, "disc-copies", "laptop", &[planner_card()]).await;
    let legacy_id = entry(&legacy, "planner")["id"].clone();

    let both = ok(report_copies(
        &server,
        user,
        "disc-copies",
        "laptop",
        &[(planner_card(), Some(a)), (planner_card(), Some(b))],
    )
    .await)
    .await;
    // The first copy with an id takes over the row without one: same id, key and name.
    assert_eq!(copy_entry(&both, a)["id"], legacy_id);
    assert_eq!(copy_entry(&both, a)["name"], "planner");
    assert_eq!(copy_entry(&both, b)["created"], true);
    assert_eq!(copy_entry(&both, b)["name"], "planner-bbbb");
    assert_eq!(both["stopped"], 0);

    // Restarted in any order (on any ports), each copy is the same agent.
    let again = ok(report_copies(
        &server,
        user,
        "disc-copies",
        "laptop",
        &[(planner_card(), Some(b)), (planner_card(), Some(a))],
    )
    .await)
    .await;
    assert_eq!(copy_entry(&again, a)["id"], legacy_id);
    assert_eq!(copy_entry(&again, b)["id"], copy_entry(&both, b)["id"]);

    // One copy stopped: only it is marked stopped.
    let one = ok(report_copies(
        &server,
        user,
        "disc-copies",
        "laptop",
        &[(planner_card(), Some(a))],
    )
    .await)
    .await;
    assert_eq!(one["stopped"], 1);
    assert_eq!(
        status_of(&server, &copy_entry(&both, b)["id"]).await,
        "stopped"
    );

    // Listed back with their copy ids and catalog names.
    let listing = listed(&server, user, "disc-copies", "laptop").await;
    let mut copies: Vec<(String, String, String)> = listing["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .map(|agent| {
            (
                agent["instance"].as_str().unwrap_or_default().to_string(),
                agent["name"].as_str().unwrap_or_default().to_string(),
                agent["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    copies.sort();
    assert_eq!(
        copies,
        [
            (
                a.to_string(),
                "planner".to_string(),
                "registered".to_string()
            ),
            (
                b.to_string(),
                "planner-bbbb".to_string(),
                "stopped".to_string()
            ),
        ]
    );

    let bad = report_copies(
        &server,
        user,
        "disc-copies",
        "laptop",
        &[(planner_card(), Some("../etc"))],
    )
    .await;
    assert_eq!(bad.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_discovered_agent_gets_an_mcp_credential_and_calls_tools_as_its_owner() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-mcp").await;
    let registered = report_ok(&server, user, "disc-mcp", "laptop", &[crew_card()]).await;
    let agent_id = entry(&registered, "crew-researcher")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Its owner mints it a gateway credential, as for a local coding agent.
    let minted = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}/mcp-token"))),
        &user.to_string(),
        "disc-mcp",
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap();
    assert_eq!(minted.status(), 200);
    let minted: Value = minted.json().await.unwrap();
    let token = minted["data"]["token"].as_str().expect("token").to_string();

    // No traceparent: it runs outside Nasiko, so `tools/call` acts as its owner instead of
    // being refused. The bogus tool is answered by the protocol layer, past the auth gate.
    let call = server
        .client
        .post(server.url("/api/mcp"))
        .bearer_auth(&token)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "some__tool", "arguments": {}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(call.status(), 200);
    let body: Value = call.json().await.unwrap();
    assert!(body.get("error").is_some(), "unknown tool: {body}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_card_without_a_name_or_a_report_without_a_machine_is_rejected() {
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-bad").await;

    let unnamed = report(
        &server,
        user,
        "disc-bad",
        "laptop",
        &[json!({"url": "http://x/"})],
    )
    .await;
    assert_eq!(unnamed.status(), 400);
    let no_machine = report(&server, user, "disc-bad", "  ", &[crew_card()]).await;
    assert_eq!(no_machine.status(), 400);

    server.cleanup().await;
}

/// Point the router at a stub upstream with a platform key, before the app reads its config.
async fn stub_upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,
                "model":"gpt-4o-mini",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},
                            "finish_reason":"stop"}],
                "usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
        )
        .create_async()
        .await;
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe {
        std::env::set_var("OPENAI_API_BASE", upstream.url());
        std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET);
        std::env::set_var("PLATFORM_OPENAI_API_KEY", "sk-platform-test");
    }
    upstream
}

#[tokio::test]
#[serial]
async fn a_discovered_agent_routes_llm_calls_with_its_key_and_shows_as_called() {
    let _upstream = stub_upstream().await;
    let server = TestServer::start().await;
    let user = seed_user(&server, "disc-routing").await;
    let registered = report_ok(&server, user, "disc-routing", "laptop", &[crew_card()]).await;
    let agent_id = entry(&registered, "crew-researcher")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Its key lives as long as a deployed agent's, since it is read once at startup.
    let key: Value = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent_id}/llm-token"))),
        &user.to_string(),
        "disc-routing",
    )
    .json(&json!({}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let token = key["data"]["token"].as_str().expect("token").to_string();
    let expires_at: DateTime<Utc> = key["data"]["expires_at"].as_str().unwrap().parse().unwrap();
    assert!(expires_at > Utc::now() + chrono::Duration::days(364));

    // No traceparent: the agent runs outside Nasiko. The call is accepted and billed to its owner.
    let response = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&json!({"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut billed_to = None;
    for _ in 0..50 {
        billed_to = sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM trace_usage WHERE agent_id = $1::uuid LIMIT 1",
        )
        .bind(&agent_id)
        .fetch_optional(&server.db)
        .await
        .unwrap();
        if billed_to.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(billed_to, Some(user));

    // The next report shows the call, which is what the CLI reads as LLM routing: on.
    let after = report_ok(&server, user, "disc-routing", "laptop", &[crew_card()]).await;
    assert!(!entry(&after, "crew-researcher")["last_llm_call_at"].is_null());
    let listing = listed(&server, user, "disc-routing", "laptop").await;
    assert!(!listing["agents"][0]["last_llm_call_at"].is_null());

    // FinOps reads `trace_usage`, which the router writes as it prices the call: a call with no
    // flow is its own `direct:` row.
    let in_finops: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM trace_usage \
         WHERE agent_name = 'crew-researcher' AND trace_id LIKE 'direct:%'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(in_finops, 1);

    // Each call is a turn of the agent's one session: a second call adds to the same session.
    let second = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(&token)
        .json(&json!({"model": "gpt-4o-mini", "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "and again"}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    // Ordered the way the chat UI orders it: by the millisecond, then by id.
    let mut transcript: Vec<(String, String)> = Vec::new();
    for _ in 0..50 {
        transcript = sqlx::query_as(
            "SELECT m.role, m.content FROM chat_messages m \
             JOIN chat_sessions s ON s.session_id = m.session_id \
             WHERE s.agent_id = $1::uuid \
             ORDER BY date_trunc('milliseconds', m.timestamp), m.id::text",
        )
        .bind(&agent_id)
        .fetch_all(&server.db)
        .await
        .unwrap();
        if transcript.len() == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let as_pairs: Vec<(&str, &str)> = transcript
        .iter()
        .map(|(role, content)| (role.as_str(), content.as_str()))
        .collect();
    assert_eq!(
        as_pairs,
        [
            ("user", "hello"),
            ("assistant", "hi"),
            ("user", "and again"),
            ("assistant", "hi")
        ]
    );
    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_sessions WHERE agent_id = $1::uuid")
            .bind(&agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(sessions, 1);

    // The Sessions page lists it for its owner.
    let listed: Value = common::as_member(
        server
            .client
            .get(server.url("/api/observability/session/list")),
        &user.to_string(),
        "disc-routing",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let session_ids: Vec<&str> = listed["data"]["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .filter_map(|session| session["session_id"].as_str())
        .collect();
    let expected = nasiko_server::chat::discovered_turns::session_id_for(agent_id.parse().unwrap());
    assert_eq!(session_ids, [expected.as_str()]);

    // `disconnect` revokes its keys: the key it had is refused from then on, and a key minted
    // afterwards (a later `connect`, even within the same second) works.
    let as_owner = |request: reqwest::RequestBuilder| {
        common::as_member(request, &user.to_string(), "disc-routing")
    };
    let llm_token_url = server.url(&format!("/api/agents/{agent_id}/llm-token"));
    let revoked = as_owner(server.client.delete(&llm_token_url))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 204);
    let call = |key: String| {
        server
            .client
            .post(server.url("/v1/chat/completions"))
            .bearer_auth(key)
            .json(&json!({"model": "gpt-4o-mini", "messages": [{"role": "user", "content": "after"}]}))
            .send()
    };
    assert_eq!(call(token.clone()).await.unwrap().status(), 401);
    let fresh: Value = as_owner(server.client.post(&llm_token_url))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fresh = fresh["data"]["token"].as_str().expect("token").to_string();
    assert_eq!(call(fresh).await.unwrap().status(), 200);

    server.cleanup().await;
}
