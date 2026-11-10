//! Sandbox-minutes metering: one meter tick over a Running fake sandbox, then the FinOps
//! dashboard's `total_sandbox_minutes` / per-agent `sandbox_minutes`.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test sandbox_minutes -- --test-threads=1

mod common;

use async_trait::async_trait;
use nasiko_runtime::{
    RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider, SandboxProviderKind,
    SandboxSpec, SandboxState, SandboxStats, TransportKind,
};
use nasiko_server::agents::hours_meter::{self, sandbox};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

#[tokio::test]
#[serial]
async fn running_sandbox_bills_minutes_not_container_hours() {
    let server = common::TestServer::start().await;
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = admin["user_id"].as_str().unwrap();
    let agent: Value =
        common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
            .json(&json!({"name": "sandbox-minutes-agent", "version": "1.0.0"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let key = SandboxKey::Agent { agent_id };
    let handle = server
        .sandbox
        .acquire(&SandboxSpec {
            key,
            image: "img:1".into(),
            command: vec![],
            env: Default::default(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8080],
            transport: TransportKind::Http,
            image_pull_secret: None,
        })
        .await
        .unwrap();

    let stats = sandbox::reconcile(&server.db, server.sandbox.as_ref())
        .await
        .unwrap();
    assert_eq!((stats.observed, stats.upserted, stats.closed), (1, 1, 0));

    // Pretend it has been running for three minutes.
    sqlx::query(
        "UPDATE agent_instance_sessions
         SET started_at = started_at - interval '3 minutes'
         WHERE agent_id = $1 AND kind = 'sandbox'",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let dashboard: Value = common::as_superuser(
        server
            .client
            .get(server.url("/api/observability/finops/dashboard")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let summary = &dashboard["data"]["summary"];
    let minutes = summary["total_sandbox_minutes"].as_f64().unwrap();
    assert!((minutes - 3.0).abs() < 0.1, "summary minutes: {minutes}");
    assert_eq!(summary["total_container_hours"].as_f64().unwrap(), 0.0);
    let row = dashboard["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent_id"] == agent_id.to_string())
        .expect("agent row present");
    assert!((row["sandbox_minutes"].as_f64().unwrap() - 3.0).abs() < 0.1);

    // Suspending closes the open row on the next tick.
    server.sandbox.suspend(&handle).await.unwrap();
    let stats = sandbox::reconcile(&server.db, server.sandbox.as_ref())
        .await
        .unwrap();
    assert_eq!((stats.observed, stats.closed), (0, 1));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_sandbox_with_non_uuid_context_id_bills_its_harness_agent() {
    let server = common::TestServer::start().await;
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = admin["user_id"].as_str().unwrap();
    let agent: Value =
        common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
            .json(&json!({"name": "sandbox-session-agent", "version": "1.0.0"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    // The key is a hash of the contextId; only the stored sandbox_ref ties it to the row.
    let handle = server
        .sandbox
        .acquire(&SandboxSpec {
            key: SandboxKey::Session {
                session_id: Uuid::new_v4(),
            },
            image: "img:1".into(),
            command: vec![],
            env: Default::default(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8080],
            transport: TransportKind::Http,
            image_pull_secret: None,
        })
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title, sandbox_ref)
         VALUES ('ses_not_a_uuid', $1, $2, 't', $3)",
    )
    .bind(Uuid::parse_str(uid).unwrap())
    .bind(agent_id)
    .bind(handle.sandbox_ref())
    .execute(&server.db)
    .await
    .unwrap();

    let stats = sandbox::reconcile(&server.db, server.sandbox.as_ref())
        .await
        .unwrap();
    assert_eq!((stats.observed, stats.upserted), (1, 1));
    let billed: Uuid =
        sqlx::query_scalar("SELECT agent_id FROM agent_instance_sessions WHERE kind = 'sandbox'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(billed, agent_id);

    server.cleanup().await;
}

/// Admin user id, plus an agent created through the API.
async fn admin_and_agent(server: &common::TestServer, name: &str) -> (Uuid, Uuid) {
    let admin: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let uid = admin["user_id"].as_str().unwrap();
    let agent: Value =
        common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
            .json(&json!({"name": name, "version": "1.0.0"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    (
        uid.parse().unwrap(),
        agent["id"].as_str().unwrap().parse().unwrap(),
    )
}

fn spec(key: SandboxKey) -> SandboxSpec {
    SandboxSpec {
        key,
        image: "img:1".into(),
        command: vec![],
        env: Default::default(),
        resources: None,
        workspace: None,
        runtime_class: None,
        ports: vec![8080],
        transport: TransportKind::Http,
        image_pull_secret: None,
    }
}

async fn open_sandbox_rows(server: &common::TestServer) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM agent_instance_sessions WHERE kind = 'sandbox' AND ended_at IS NULL",
    )
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn container_tick_leaves_open_sandbox_rows_alone() {
    let server = common::TestServer::start().await;
    let (_, agent_id) = admin_and_agent(&server, "sandbox-vs-container").await;
    server
        .sandbox
        .acquire(&spec(SandboxKey::Agent { agent_id }))
        .await
        .unwrap();
    sandbox::reconcile(&server.db, server.sandbox.as_ref())
        .await
        .unwrap();
    assert_eq!(open_sandbox_rows(&server).await, 1);

    // The fake runtime reports no containers, which closes every open *container* row.
    hours_meter::reconcile_once(&server.db, server.runtime.as_ref(), "docker")
        .await
        .unwrap();
    assert_eq!(open_sandbox_rows(&server).await, 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn session_sandbox_with_uuid_context_id_falls_back_to_its_key() {
    let server = common::TestServer::start().await;
    let (uid, agent_id) = admin_and_agent(&server, "sandbox-uuid-session").await;
    let session_id = Uuid::new_v4();
    // No sandbox_ref stored: only the key, which is the contextId itself, ties it to the agent.
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) VALUES ($1, $2, $3, 't')",
    )
    .bind(session_id.to_string())
    .bind(uid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();
    server
        .sandbox
        .acquire(&spec(SandboxKey::Session { session_id }))
        .await
        .unwrap();

    let stats = sandbox::reconcile(&server.db, server.sandbox.as_ref())
        .await
        .unwrap();
    assert_eq!(stats.observed, 1);
    let billed: Uuid =
        sqlx::query_scalar("SELECT agent_id FROM agent_instance_sessions WHERE kind = 'sandbox'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(billed, agent_id);

    server.cleanup().await;
}

/// Lists one Running sandbox with no backend start time; everything else is refused.
struct NoStartProvider(SandboxInfo);

#[async_trait]
impl SandboxProvider for NoStartProvider {
    fn kind(&self) -> SandboxProviderKind {
        SandboxProviderKind::Fake
    }
    async fn acquire(&self, _: &SandboxSpec) -> nasiko_runtime::Result<SandboxHandle> {
        Err(RuntimeError::Internal("stub".into()))
    }
    async fn attach(
        &self,
        _: &SandboxHandle,
    ) -> nasiko_runtime::Result<nasiko_runtime::SandboxTransport> {
        Err(RuntimeError::Internal("stub".into()))
    }
    async fn status(&self, _: &SandboxHandle) -> nasiko_runtime::Result<SandboxState> {
        Ok(SandboxState::Running)
    }
    async fn suspend(&self, _: &SandboxHandle) -> nasiko_runtime::Result<()> {
        Ok(())
    }
    async fn release(&self, _: &SandboxHandle) -> nasiko_runtime::Result<()> {
        Ok(())
    }
    async fn resume_takes_new_env(&self, _: &SandboxHandle) -> nasiko_runtime::Result<bool> {
        Ok(false)
    }
    async fn stats(&self, _: &SandboxHandle) -> nasiko_runtime::Result<SandboxStats> {
        Err(RuntimeError::Internal("stub".into()))
    }
    async fn list(&self) -> nasiko_runtime::Result<Vec<SandboxInfo>> {
        Ok(vec![self.0.clone()])
    }
}

#[tokio::test]
#[serial]
async fn sandbox_without_start_time_keeps_one_open_row_across_replicas() {
    let server = common::TestServer::start().await;
    let (_, agent_id) = admin_and_agent(&server, "sandbox-no-start").await;
    let key = SandboxKey::Agent { agent_id };
    let provider = NoStartProvider(SandboxInfo {
        handle: SandboxHandle {
            key,
            provider: SandboxProviderKind::Fake,
            backend_ref: "stub-sandbox".into(),
        },
        state: SandboxState::Running,
        started_at: None,
    });

    // Two meter instances ticking at once, then again.
    for _ in 0..2 {
        let (a, b) = tokio::join!(
            sandbox::reconcile(&server.db, &provider),
            sandbox::reconcile(&server.db, &provider)
        );
        a.unwrap();
        b.unwrap();
    }
    assert_eq!(open_sandbox_rows(&server).await, 1);
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_instance_sessions WHERE kind = 'sandbox'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(total, 1);

    server.cleanup().await;
}
