//! The reaper's agent half: parks idle framework-agent sandboxes and cleans up inconsistent ones.
//!
//! `just infra` then
//! `cargo test -p nasiko-server --test agent_sandbox_reaper -- --test-threads=1`

mod common;

use std::collections::HashMap;

use nasiko_runtime::{
    SandboxHandle, SandboxKey, SandboxProvider, SandboxSpec, SandboxState, TransportKind,
};
use nasiko_server::sessions::reaper::agents::{claim_park, suspend_claimed};
use nasiko_server::sessions::reaper::{ReaperMemory, SweepScope, sweep};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn server(ttl: i64) -> common::TestServer {
    common::TestServer::start_with(move |c| {
        c.coding_agent_idle_ttl_secs = ttl;
        c.session_idle_grace_secs = 0;
        c.session_cpu_quiescent_millicores = 20;
    })
    .await
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

/// A `running` auto-parking agent whose last request was `last_request`, with a live fake
/// sandbox.
async fn running_agent(
    server: &common::TestServer,
    owner: Uuid,
    policy: &str,
    last_request: &str,
) -> (Uuid, SandboxHandle) {
    let id = sqlx::query_scalar::<_, Uuid>(&format!(
        "INSERT INTO agents (name, owner_id, version, image, status, url, state_mode, \
                             parking_policy, last_request_at) \
         VALUES ($1, $2, '1.0.0', 'x:1', 'running', NULL, 'durable', $3, {last_request}) \
         RETURNING id"
    ))
    .bind(format!("agent-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(policy)
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

async fn run(server: &common::TestServer, scope: SweepScope) {
    sweep(&server.state, &mut ReaperMemory::default(), scope)
        .await
        .unwrap();
}

const HOUR_AGO: &str = "now() - interval '1 hour'";

#[tokio::test]
#[serial]
async fn an_idle_quiet_agent_is_suspended_and_parked() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;

    run(&server, SweepScope::Full).await;

    assert_eq!(status(&server, id).await, "parked");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_recent_request_leaves_the_agent_running() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", "now() - interval '5 seconds'").await;

    run(&server, SweepScope::Full).await;

    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_model_call_inside_the_ttl_leaves_the_agent_running() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, agent_id, provider, model, \
         started_at, materialized_at) \
         VALUES ('reaper-trace', 'agent', $1, $2, 'p', 'm', now() - interval '10 seconds', \
         now() - interval '10 seconds')",
    )
    .bind(owner)
    .bind(id)
    .execute(&server.db)
    .await
    .unwrap();

    run(&server, SweepScope::Full).await;

    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_in_flight_leaves_the_agent_running() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    let _in_flight = server
        .state
        .genai_metrics
        .begin_agent_request(&id.to_string());

    run(&server, SweepScope::Full).await;

    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_between_the_read_and_the_write_wins_the_fence() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    let read: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_request_at FROM agents WHERE id = $1")
            .bind(id)
            .fetch_one(&server.db)
            .await
            .unwrap();

    // The resolver serves a request after the sweep read the row.
    sqlx::query("UPDATE agents SET last_request_at = now() WHERE id = $1")
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();

    assert!(!claim_park(&server.db, id, read).await.unwrap());
    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_sandbox_of_a_deleted_agent_is_released() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();

    run(&server, SweepScope::Full).await;

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn ttl_zero_parks_nothing_but_still_releases_a_deleted_agents_sandbox() {
    let server = server(0).await;
    let owner = init_admin(&server).await;
    let (idle, idle_handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    let (gone, gone_handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(gone)
        .execute(&server.db)
        .await
        .unwrap();

    // What `spawn_reaper` runs with a TTL of 0.
    run(&server, SweepScope::RecoveryOnly).await;

    assert_eq!(status(&server, idle).await, "running");
    assert_eq!(
        server.sandbox.status(&idle_handle).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(
        server.sandbox.status(&gone_handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resident_agent_with_a_live_sandbox_is_released_and_stopped() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "resident", HOUR_AGO).await;

    run(&server, SweepScope::Full).await;

    assert_eq!(status(&server, id).await, "stopped");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    server.cleanup().await;
}

async fn set_agent(server: &common::TestServer, id: Uuid, assignments: &str) {
    sqlx::query(&format!("UPDATE agents SET {assignments} WHERE id = $1"))
        .bind(id)
        .execute(&server.db)
        .await
        .unwrap();
}

async fn url_of(server: &common::TestServer, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT url FROM agents WHERE id = $1")
        .bind(id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn an_idle_sandbox_of_a_workload_agent_is_suspended_and_the_row_is_left_alone() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    set_agent(&server, id, "url = 'http://10.0.0.7:8000'").await;

    run(&server, SweepScope::Full).await;

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        url_of(&server, id).await.as_deref(),
        Some("http://10.0.0.7:8000")
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resident_agents_suspended_sandbox_is_released_at_once_but_a_busy_one_waits() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (suspended_id, suspended) = running_agent(&server, owner, "resident", "now()").await;
    server.sandbox.suspend(&suspended).await.unwrap();
    let (busy_id, busy) = running_agent(&server, owner, "resident", HOUR_AGO).await;
    let _in_flight = server
        .state
        .genai_metrics
        .begin_agent_request(&busy_id.to_string());

    run(&server, SweepScope::Full).await;

    assert_eq!(
        server.sandbox.status(&suspended).await.unwrap(),
        SandboxState::Absent
    );
    assert_eq!(status(&server, suspended_id).await, "stopped");
    assert_eq!(
        server.sandbox.status(&busy).await.unwrap(),
        SandboxState::Running
    );
    assert_eq!(status(&server, busy_id).await, "running");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_resident_row_with_an_address_keeps_its_status() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "resident", HOUR_AGO).await;
    set_agent(&server, id, "url = 'http://10.0.0.7:8000'").await;

    run(&server, SweepScope::Full).await;

    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        url_of(&server, id).await.as_deref(),
        Some("http://10.0.0.7:8000")
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_wake_between_the_claim_and_the_suspend_leaves_the_sandbox_running() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    let read: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_request_at FROM agents WHERE id = $1")
            .bind(id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(claim_park(&server.db, id, read).await.unwrap());

    // The resolver wakes the agent after the claim.
    set_agent(&server, id, "status = 'running', last_request_at = now()").await;
    let suspended = suspend_claimed(&server.state, id, std::slice::from_ref(&handle))
        .await
        .unwrap();

    assert!(!suspended);
    assert_eq!(status(&server, id).await, "running");
    assert_eq!(
        server.sandbox.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_wake_during_a_slow_suspend_is_not_overwritten() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (id, handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    server
        .sandbox
        .delay_next_suspend(std::time::Duration::from_millis(400));
    let state = server.state.clone();
    let sweeping = tokio::spawn(async move {
        sweep(&state, &mut ReaperMemory::default(), SweepScope::Full)
            .await
            .unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    set_agent(&server, id, "status = 'running', last_request_at = now()").await;
    sweeping.await.unwrap();

    assert_eq!(
        status(&server, id).await,
        "running",
        "the wake's write stands"
    );
    let _ = handle;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_stray_sandbox_under_a_parked_or_stopped_row_is_suspended() {
    let server = server(60).await;
    let owner = init_admin(&server).await;
    let (parked, parked_handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    set_agent(&server, parked, "status = 'parked'").await;
    let (stopped, stopped_handle) = running_agent(&server, owner, "auto", HOUR_AGO).await;
    set_agent(&server, stopped, "status = 'stopped'").await;
    for handle in [&parked_handle, &stopped_handle] {
        server.sandbox.set_started_at(
            handle,
            Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        );
    }

    run(&server, SweepScope::Full).await;

    assert_eq!(
        server.sandbox.status(&parked_handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert_eq!(
        server.sandbox.status(&stopped_handle).await.unwrap(),
        SandboxState::Suspended
    );
    server.cleanup().await;
}
