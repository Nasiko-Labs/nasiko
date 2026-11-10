//! The reaper's agent half: agent sandboxes are no longer acquired, so any the provider still
//! lists were left from before parking scaled agents' own workloads, and are released once they
//! can no longer be serving a call. Agents' rows are never changed by it.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test agent_sandbox_reaper -- --test-threads=1`

mod common;

use std::collections::HashMap;

use nasiko_runtime::{
    SandboxHandle, SandboxKey, SandboxProvider, SandboxSpec, SandboxState, TransportKind,
};
use nasiko_server::sessions::reaper::{ReaperMemory, SweepScope, sweep};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn server(ttl: i64) -> common::TestServer {
    common::TestServer::start_with(move |c| c.coding_agent_idle_ttl_secs = ttl).await
}

async fn init_admin(server: &common::TestServer) -> Uuid {
    let v: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    Uuid::parse_str(v["user_id"].as_str().unwrap()).unwrap()
}

/// A `running` agent whose last request was `last_request`, with a live leftover sandbox.
async fn running_agent(
    server: &common::TestServer,
    owner: Uuid,
    last_request: &str,
) -> (Uuid, SandboxHandle) {
    let id = sqlx::query_scalar::<_, Uuid>(&format!(
        "INSERT INTO agents (name, owner_id, version, image, status, last_request_at) \
         VALUES ($1, $2, '1.0.0', 'x:1', 'running', {last_request}) RETURNING id"
    ))
    .bind(format!("agent-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap();
    (id, acquire(server, id).await)
}

async fn acquire(server: &common::TestServer, agent_id: Uuid) -> SandboxHandle {
    server
        .sandbox
        .acquire(&SandboxSpec {
            key: SandboxKey::Agent { agent_id },
            image: "x:1".into(),
            command: vec![],
            env: HashMap::new(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8000],
            transport: TransportKind::Http,
            image_pull_secret: None,
        })
        .await
        .unwrap()
}

async fn status(server: &common::TestServer, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn sandbox_state(server: &common::TestServer, handle: &SandboxHandle) -> SandboxState {
    server.sandbox.status(handle).await.unwrap()
}

async fn run(server: &common::TestServer, scope: SweepScope) {
    sweep(&server.state, &mut ReaperMemory::default(), scope)
        .await
        .unwrap();
}

const HOUR_AGO: &str = "now() - interval '1 hour'";

#[tokio::test]
#[serial]
async fn a_running_leftover_is_released_once_its_agent_is_idle_past_the_call_window() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, HOUR_AGO).await;

    run(&server, SweepScope::Full).await;

    assert_eq!(sandbox_state(&server, &handle).await, SandboxState::Absent);
    assert_eq!(
        status(&server, id).await,
        "running",
        "the row is left alone"
    );
    server.cleanup().await;
}

/// A call through an earlier server may still be on its way to the sandbox: one that began within
/// the call timeout (plus the stamp throttle and a tick) keeps it.
#[tokio::test]
#[serial]
async fn a_recently_called_agents_running_leftover_is_kept() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (_, handle) = running_agent(&server, owner, "now() - interval '1 minute'").await;

    run(&server, SweepScope::Full).await;

    assert_eq!(sandbox_state(&server, &handle).await, SandboxState::Running);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_in_flight_keeps_a_running_leftover() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, HOUR_AGO).await;
    let _in_flight = server
        .state
        .genai_metrics
        .begin_agent_request(&id.to_string());

    run(&server, SweepScope::Full).await;

    assert_eq!(sandbox_state(&server, &handle).await, SandboxState::Running);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_suspended_leftover_is_released_at_once() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (_, handle) = running_agent(&server, owner, "now()").await;
    server.sandbox.suspend(&handle).await.unwrap();

    run(&server, SweepScope::Full).await;

    assert_eq!(sandbox_state(&server, &handle).await, SandboxState::Absent);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sandbox_of_a_deleted_agent_is_released() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "now()").await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();

    run(&server, SweepScope::Full).await;

    assert_eq!(sandbox_state(&server, &handle).await, SandboxState::Absent);
    server.cleanup().await;
}

/// With idle parking off (a TTL of 0) the clean-up still runs.
#[tokio::test]
#[serial]
async fn ttl_zero_still_releases_leftover_agent_sandboxes() {
    let server = server(0).await;
    let owner = init_admin(&server).await;
    let (_, idle_handle) = running_agent(&server, owner, HOUR_AGO).await;
    let (_, busy_handle) = running_agent(&server, owner, "now()").await;

    // What `spawn_reaper` runs with a TTL of 0.
    run(&server, SweepScope::RecoveryOnly).await;

    assert_eq!(
        sandbox_state(&server, &idle_handle).await,
        SandboxState::Absent
    );
    assert_eq!(
        sandbox_state(&server, &busy_handle).await,
        SandboxState::Running
    );
    server.cleanup().await;
}
