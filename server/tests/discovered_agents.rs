//! Integration tests for A2A agents the CLI discovers on a user's machine.
//!
//! Covers:
//!   - POST /api/agents/discovered — registers a machine's running agents from their cards,
//!     keeps one row per agent across reports, marks agents no longer reported `stopped`, and
//!     never makes them `running` (so nothing routes chat to them)
//!   - POST /api/agents/{id}/llm-token — a discovered agent gets a deployed agent's year-long key
//!   - The LLM router — a discovered agent's call needs no traceparent and is billed to its
//!     owner, after which registration reports when it last made a call
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
            "SELECT user_id FROM token_usage WHERE agent_id = $1::uuid LIMIT 1",
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

    // The next report shows the call, which is what `nasiko status` reads as LLM routing: on.
    let after = report_ok(&server, user, "disc-routing", "laptop", &[crew_card()]).await;
    assert!(!entry(&after, "crew-researcher")["last_llm_call_at"].is_null());

    server.cleanup().await;
}
