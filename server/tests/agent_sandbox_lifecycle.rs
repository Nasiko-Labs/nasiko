//! A parked agent's sandbox over its life: credentials minted once and kept, a peek that never
//! acquires, a live workload that keeps serving, the card refreshed after a parked deploy, and
//! the idle-clock throttle.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_sandbox_lifecycle -- --test-threads=1

mod common;

use std::collections::HashMap;

use axum::{Json, Router, routing::get};
use nasiko_runtime::SandboxKey;
use nasiko_server::agent_resolver::{Caller, ResolveError, ResolvedTarget, resolve_target};
use nasiko_server::agents::sandbox_credentials::clear_agent_sandbox_credentials;
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

async fn seed_durable_agent(server: &common::TestServer, owner: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy, state_mode) \
         VALUES ($1, $2, 'x:1.0.0', $3, '', 'auto', 'durable') RETURNING id",
    )
    .bind(format!("lifecycle-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(status)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

fn key(agent: Uuid) -> SandboxKey {
    SandboxKey::Agent { agent_id: agent }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn env_of(server: &common::TestServer, agent: Uuid) -> HashMap<String, String> {
    server
        .sandbox
        .last_acquire_spec(&key(agent))
        .expect("an acquire was recorded")
        .env
}

async fn resolve(server: &common::TestServer, agent: Uuid) -> Result<ResolvedTarget, ResolveError> {
    resolve_target(&server.state, agent, &Caller::default(), None).await
}

#[tokio::test]
#[serial]
async fn credentials_are_minted_once_and_stay_valid_across_requests() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let agent = seed_durable_agent(&server, owner, "parked").await;

    resolve(&server, agent).await.unwrap();
    let first = env_of(&server, agent);
    resolve(&server, agent).await.unwrap();
    resolve(&server, agent).await.unwrap();
    let third = env_of(&server, agent);

    // Byte-equal env, so a sandbox that keeps its first env keeps tokens the platform still honours.
    assert_eq!(first, third);
    let token = first
        .get("MCP_GATEWAY_TOKEN")
        .expect("a gateway credential in the env");
    assert_eq!(
        nasiko_mcp_gateway::agent_tokens::authenticate(&server.db, token)
            .await
            .unwrap(),
        Some(agent)
    );

    // The stored blob is encrypted: the plaintext token is nowhere in it.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT env_enc FROM agent_sandbox_credentials WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(
        !String::from_utf8_lossy(&stored).contains(token.as_str()),
        "env_enc holds the token in the clear"
    );

    // An update clears the stored credentials; the next wake mints new ones.
    clear_agent_sandbox_credentials(&server.db, agent)
        .await
        .unwrap();
    resolve(&server, agent).await.unwrap();
    assert_ne!(env_of(&server, agent)["MCP_GATEWAY_TOKEN"], *token);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_live_workload_keeps_serving_a_parkable_agent() {
    let server = common::TestServer::start().await;
    // The fake runtime reports every agent's workload as Running, as for a first resident deploy.
    let owner = init_admin(&server).await;
    let agent = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, url, parking_policy, state_mode) \
         VALUES ('has-workload', $1, 'x:1.0.0', 'running', 'http://workload.test:8000', 'auto', 'durable') \
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap();

    let target = resolve(&server, agent).await.unwrap();

    assert!(
        matches!(&target, ResolvedTarget::Container(t) if t.base.url == "http://workload.test:8000")
    );
    assert_eq!(server.sandbox.acquire_count(&key(agent)), 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_first_wake_after_a_deploy_refreshes_the_agent_card() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let agent = seed_durable_agent(&server, owner, "parked").await;
    let stub = serve(Router::new().route(
        "/.well-known/agent-card.json",
        get(|| async {
            Json(json!({ "name": "fresh", "description": "card from the new image", "skills": [] }))
        }),
    ))
    .await;
    server.sandbox.set_transport_url(&key(agent), stub);

    resolve(&server, agent).await.unwrap();

    // The fetch runs off the request path (its first attempt waits a few seconds).
    let mut description: Option<String> = None;
    for _ in 0..30 {
        description = sqlx::query_scalar("SELECT description FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
        if description.as_deref() == Some("card from the new image") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert_eq!(description.as_deref(), Some("card from the new image"));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_idle_clock_is_stamped_at_most_every_few_seconds() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let agent = seed_durable_agent(&server, owner, "parked").await;
    let stamp = |server: &common::TestServer| {
        let db = server.db.clone();
        async move {
            sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
                "SELECT last_request_at FROM agents WHERE id = $1",
            )
            .bind(agent)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };

    resolve(&server, agent).await.unwrap();
    let first = stamp(&server).await.expect("stamped on the wake");
    resolve(&server, agent).await.unwrap();

    assert_eq!(
        stamp(&server).await,
        Some(first),
        "a second request within the window is not a write"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_wake_that_races_the_reaper_is_retried_once() {
    let server = common::TestServer::start().await;
    server.runtime.set_status_follows_deploys(true);
    let owner = init_admin(&server).await;
    let agent = seed_durable_agent(&server, owner, "parked").await;

    server
        .sandbox
        .fail_next_acquire(nasiko_runtime::RuntimeError::SandboxNotRunning(
            "exiting".into(),
        ));
    resolve(&server, agent)
        .await
        .expect("the second attempt succeeds");
    assert_eq!(server.sandbox.acquire_count(&key(agent)), 2);

    // A wake that fails outright still stamped the request first, so the reaper sees it.
    let other = seed_durable_agent(&server, owner, "parked").await;
    server
        .sandbox
        .fail_next_acquire(nasiko_runtime::RuntimeError::Timeout("slow".into()));
    assert!(resolve(&server, other).await.is_err());
    let stamped: bool =
        sqlx::query_scalar("SELECT last_request_at IS NOT NULL FROM agents WHERE id = $1")
            .bind(other)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(stamped);
    server.cleanup().await;
}
