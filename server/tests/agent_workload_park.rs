//! The reaper's workload pass against a real database: a running framework agent whose parking
//! policy is `auto` and that has been idle for `AGENT_PARK_IDLE_SECS` has its own workload scaled to
//! zero and its row parked, unless a request in flight, a recent model call or an open pause younger
//! than two hours holds it up; a request that lands between the read and the park wins; resident,
//! harness and coding agents are never parked (a resident one is, where the edition does not honour
//! `resident`); and an agent the wake brought back parks again once it has been idle for the window.
//!
//! Requires infra (Postgres, Redis, S3) like the rest of the suite:
//!   just infra
//!   cargo test -p nasiko-server --test agent_workload_park -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Utc};
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, Result as RuntimeResult,
    RuntimeError, RuntimeState,
};
use nasiko_server::agent_resolver::{
    Caller, ResolveError, ResolvedTarget, load_agent, resolve_loaded,
};
use nasiko_server::agent_scaling::{AgentDeploy, AgentScaling};
use nasiko_server::agent_wake::AgentWake;
use nasiko_server::agent_wake::parked::{ParkedAgentWake, WakeDeps};
use nasiko_server::sessions::reaper::workloads::{Park, park_workload};
use nasiko_server::sessions::reaper::{ReaperMemory, SweepScope, sweep};
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

/// Two hours: past the default one-hour park window.
const TWO_HOURS: i64 = 2 * 60 * 60;

/// A runtime whose agents' workloads scale to and from zero, as Docker and Kubernetes do; an agent
/// with no entry has no workload at all.
#[derive(Default)]
struct WorkloadRuntime {
    /// Where a running workload answers.
    address: String,
    replicas: Mutex<HashMap<ContainerId, u32>>,
    /// Every scale to zero fails, as a runtime that cannot be reached does.
    fail_scale_down: AtomicBool,
    /// The `(code, message)` every scale up is refused with, as a layer that declines to start
    /// workloads answers.
    decline_scale_up: Mutex<Option<(String, String)>>,
}

impl WorkloadRuntime {
    fn answering_at(address: String) -> Arc<Self> {
        Arc::new(Self {
            address,
            ..Self::default()
        })
    }

    /// The agent has a workload, running one replica.
    fn run_workload(&self, agent_id: Uuid) {
        self.replicas
            .lock()
            .unwrap()
            .insert(ContainerId::from_uuid(agent_id), 1);
    }

    fn replicas_of(&self, agent_id: Uuid) -> Option<u32> {
        self.replicas
            .lock()
            .unwrap()
            .get(&ContainerId::from_uuid(agent_id))
            .copied()
    }

    fn running(&self, id: &ContainerId) -> bool {
        self.replicas
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|n| *n > 0)
    }
}

#[async_trait]
impl ContainerRuntime for WorkloadRuntime {
    async fn deploy(&self, spec: &DeploymentSpec) -> RuntimeResult<DeploymentStatus> {
        self.replicas
            .lock()
            .unwrap()
            .insert(spec.container_id.clone(), 1);
        self.status(&spec.container_id).await
    }

    async fn destroy(&self, id: &ContainerId) -> RuntimeResult<()> {
        self.replicas.lock().unwrap().remove(id);
        Ok(())
    }

    async fn scale(&self, id: &ContainerId, replicas: u32) -> RuntimeResult<()> {
        if replicas == 0 && self.fail_scale_down.load(Ordering::SeqCst) {
            return Err(RuntimeError::BackendUnreachable("runtime down".into()));
        }
        if replicas > 0
            && let Some((code, message)) = self.decline_scale_up.lock().unwrap().clone()
        {
            return Err(RuntimeError::Declined { code, message });
        }
        match self.replicas.lock().unwrap().get_mut(id) {
            Some(current) => {
                *current = replicas;
                Ok(())
            }
            None => Err(RuntimeError::ContainerNotFound(id.clone())),
        }
    }

    async fn restart(&self, _id: &ContainerId) -> RuntimeResult<()> {
        Ok(())
    }

    async fn status(&self, id: &ContainerId) -> RuntimeResult<DeploymentStatus> {
        let replicas = self.replicas.lock().unwrap().get(id).copied();
        let state = match replicas {
            None => RuntimeState::Unknown,
            Some(0) => RuntimeState::Stopped,
            Some(_) => RuntimeState::Running,
        };
        Ok(DeploymentStatus {
            container_id: id.clone(),
            state,
            replicas_live: replicas.unwrap_or(0),
            endpoint: None,
            message: None,
            restart_count: 0,
        })
    }

    async fn list(&self) -> RuntimeResult<Vec<DeploymentStatus>> {
        Ok(Vec::new())
    }

    async fn endpoint(&self, id: &ContainerId) -> RuntimeResult<String> {
        if self.running(id) {
            Ok(self.address.clone())
        } else {
            Err(RuntimeError::ContainerNotFound(id.clone()))
        }
    }

    async fn logs(&self, _id: &ContainerId, _tail: u32) -> RuntimeResult<Vec<String>> {
        Ok(Vec::new())
    }

    async fn build(
        &self,
        _tar_context: &[u8],
        image_tag: &str,
    ) -> RuntimeResult<nasiko_runtime::BuildReport> {
        Ok(nasiko_runtime::BuildReport::image_only(image_tag))
    }
}

/// A server on loopback that answers every request (with a 404): what the wake's probe needs.
async fn start_answering_agent() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, Router::new()).await.unwrap();
    });
    address
}

async fn server_over(runtime: &Arc<WorkloadRuntime>) -> common::TestServer {
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    assert_eq!(server.state.config.agent_park_idle_secs, 3600);
    server
}

async fn seed_owner(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("owner_{}", Uuid::new_v4().simple()))
        .bind(format!("{}@test.example", Uuid::new_v4().simple()))
        .fetch_one(&server.db)
        .await
        .expect("seed owner")
}

/// A `running` agent row as a deploy leaves it.
struct Seed<'a> {
    kind: &'a str,
    policy: &'a str,
    /// Seconds since its last request; `None` for an agent never called.
    last_request_secs_ago: Option<i64>,
    /// Seconds since its row last changed (a deploy, a wake).
    changed_secs_ago: i64,
}

impl Seed<'_> {
    /// A framework agent left at `auto`, called last two hours ago and unchanged since.
    fn idle() -> Self {
        Self {
            kind: "agent",
            policy: "auto",
            last_request_secs_ago: Some(TWO_HOURS),
            changed_secs_ago: TWO_HOURS,
        }
    }
}

/// Inserts the row (an INSERT sets `updated_at` as given; the table's trigger moves it only on
/// UPDATE) and gives it a running workload.
async fn seed_agent(
    server: &common::TestServer,
    runtime: &WorkloadRuntime,
    seed: Seed<'_>,
) -> Uuid {
    let owner = seed_owner(server).await;
    let id = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, workload_kind, parking_policy, \
                             last_request_at, updated_at) \
         VALUES ($1, $2, 'x:1.0.0', 'running', 'http://10.0.0.7:8000', $3, $4, \
                 now() - make_interval(secs => $5), now() - make_interval(secs => $6)) \
         RETURNING id",
    )
    .bind(format!("park-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(seed.kind)
    .bind(seed.policy)
    .bind(seed.last_request_secs_ago.map(|secs| secs as f64))
    .bind(seed.changed_secs_ago as f64)
    .fetch_one(&server.db)
    .await
    .expect("seed agent");
    runtime.run_workload(id);
    id
}

/// An old deployment row and the newest one, live as a running agent's are.
async fn seed_deployments(server: &common::TestServer, agent: Uuid) {
    let build: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'x:1.0.0') RETURNING id",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .expect("seed build");
    sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, status, created_at) VALUES \
         ($1, $2, 'stopped', now() - interval '1 day'), ($1, $2, 'running', now())",
    )
    .bind(agent)
    .bind(build)
    .execute(&server.db)
    .await
    .expect("seed deployments");
}

async fn deployment_statuses(server: &common::TestServer, agent: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT status::text FROM agent_deployments WHERE agent_id = $1 ORDER BY created_at DESC",
    )
    .bind(agent)
    .fetch_all(&server.db)
    .await
    .expect("read deployments")
}

async fn status_and_url(server: &common::TestServer, agent: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, url FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .expect("read the agent row")
}

async fn status(server: &common::TestServer, agent: Uuid) -> String {
    status_and_url(server, agent).await.0
}

async fn last_request(server: &common::TestServer, agent: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT last_request_at FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .expect("read last_request_at")
}

/// Sets the agent's idle clock: its last request `last_request_secs_ago` (`None`: never called) and
/// its last row change `changed_secs_ago`. The window starts at the later of the two, and the
/// table's trigger stamps `updated_at` on every UPDATE, so it is held off for this one write.
async fn set_idle_clock(
    server: &common::TestServer,
    agent: Uuid,
    last_request_secs_ago: Option<i64>,
    changed_secs_ago: i64,
) {
    let mut tx = server.db.begin().await.unwrap();
    sqlx::query("ALTER TABLE agents DISABLE TRIGGER trg_agents_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE agents SET last_request_at = now() - make_interval(secs => $2), \
                updated_at = now() - make_interval(secs => $3) \
         WHERE id = $1",
    )
    .bind(agent)
    .bind(last_request_secs_ago.map(|secs| secs as f64))
    .bind(changed_secs_ago as f64)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE agents ENABLE TRIGGER trg_agents_updated_at")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn run_reaper(server: &common::TestServer) {
    sweep(
        &server.state,
        &mut ReaperMemory::default(),
        SweepScope::Full,
    )
    .await
    .expect("sweep");
}

#[tokio::test]
#[serial]
async fn an_idle_agent_is_scaled_to_zero_and_parked() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    seed_deployments(&server, agent).await;

    run_reaper(&server).await;

    assert_eq!(runtime.replicas_of(agent), Some(0));
    assert_eq!(
        status_and_url(&server, agent).await,
        ("parked".to_owned(), None),
        "no workload serves a parked agent; the wake writes its address back"
    );
    assert_eq!(
        deployment_statuses(&server, agent).await,
        vec!["stopped".to_owned(), "stopped".to_owned()],
        "the newest deployment is stopped with it"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_used_within_the_window_stays_running() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let called = seed_agent(
        &server,
        &runtime,
        Seed {
            last_request_secs_ago: Some(30 * 60),
            ..Seed::idle()
        },
    )
    .await;
    let redeployed = seed_agent(
        &server,
        &runtime,
        Seed {
            changed_secs_ago: 60,
            ..Seed::idle()
        },
    )
    .await;

    run_reaper(&server).await;

    for agent in [called, redeployed] {
        assert_eq!(runtime.replicas_of(agent), Some(1));
        assert_eq!(status(&server, agent).await, "running");
    }
    server.cleanup().await;
}

/// A never-called agent (no `last_request_at`) is idle from its last change: its deploy.
#[tokio::test]
#[serial]
async fn a_never_called_agent_is_idle_from_its_deploy() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let deployed_long_ago = seed_agent(
        &server,
        &runtime,
        Seed {
            last_request_secs_ago: None,
            ..Seed::idle()
        },
    )
    .await;
    let just_deployed = seed_agent(
        &server,
        &runtime,
        Seed {
            last_request_secs_ago: None,
            changed_secs_ago: 60,
            ..Seed::idle()
        },
    )
    .await;

    run_reaper(&server).await;

    assert_eq!(status(&server, deployed_long_ago).await, "parked");
    assert_eq!(runtime.replicas_of(deployed_long_ago), Some(0));
    assert_eq!(status(&server, just_deployed).await, "running");
    assert_eq!(runtime.replicas_of(just_deployed), Some(1));
    server.cleanup().await;
}

/// The park is fenced on the `last_request_at` it read, NULL included: a request stamped since
/// wins and the workload is not touched, even when the agent is still idle for the window by the
/// new stamp. Read again, the same agent parks.
#[tokio::test]
#[serial]
async fn a_request_between_the_read_and_the_park_wins() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let called = seed_agent(&server, &runtime, Seed::idle()).await;
    let never_called = seed_agent(
        &server,
        &runtime,
        Seed {
            last_request_secs_ago: None,
            ..Seed::idle()
        },
    )
    .await;

    for agent in [called, never_called] {
        let read = last_request(&server, agent).await;
        set_idle_clock(&server, agent, Some(90 * 60), TWO_HOURS).await;

        let park = park_workload(&server.state, agent, read).await.unwrap();

        assert_eq!(park, Park::Moved, "read {read:?}");
        assert_eq!(status(&server, agent).await, "running");
        assert_eq!(runtime.replicas_of(agent), Some(1));

        let reread = last_request(&server, agent).await;
        let park = park_workload(&server.state, agent, reread).await.unwrap();
        assert_eq!(park, Park::Parked);
        assert_eq!(runtime.replicas_of(agent), Some(0));
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_request_in_flight_holds_the_park() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    let _in_flight = server
        .state
        .genai_metrics
        .begin_agent_request(&agent.to_string());

    run_reaper(&server).await;

    assert_eq!(status(&server, agent).await, "running");
    assert_eq!(runtime.replicas_of(agent), Some(1));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_recent_model_call_holds_the_park() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, agent_id, provider, model, \
                                  started_at, materialized_at) \
         VALUES ('park-trace', 'agent', $1, 'p', 'm', now() - interval '10 seconds', \
                 now() - interval '10 seconds')",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .unwrap();

    run_reaper(&server).await;

    assert_eq!(status(&server, agent).await, "running");
    assert_eq!(runtime.replicas_of(agent), Some(1));
    server.cleanup().await;
}

async fn open_task(server: &common::TestServer, agent: Uuid, secs_ago: i64) {
    sqlx::query(
        "INSERT INTO agent_open_tasks (agent_id, task_id, state, updated_at) \
         VALUES ($1, $2, 'input-required', now() - make_interval(secs => $3))",
    )
    .bind(agent)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(secs_ago as f64)
    .execute(&server.db)
    .await
    .expect("seed open task");
}

async fn pending_hitl(server: &common::TestServer, agent: Uuid, secs_ago: i64) {
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO hitl_requests (kind, origin, agent_id, owner_user_id, task_id, context_id, \
                                    question, created_at) \
         VALUES ('input_required', 'agent_proxy', $1, $2, $3, $4, $5, \
                 now() - make_interval(secs => $6))",
    )
    .bind(agent)
    .bind(owner)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("ctx-{}", Uuid::new_v4()))
    .bind(json!({"message": "which one?"}))
    .bind(secs_ago as f64)
    .execute(&server.db)
    .await
    .expect("seed pending hitl request");
}

#[tokio::test]
#[serial]
async fn an_open_pause_holds_the_park_for_two_hours_at_most() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let open_task_young = seed_agent(&server, &runtime, Seed::idle()).await;
    open_task(&server, open_task_young, 30 * 60).await;
    let hitl_young = seed_agent(&server, &runtime, Seed::idle()).await;
    pending_hitl(&server, hitl_young, 30 * 60).await;
    let open_task_old = seed_agent(&server, &runtime, Seed::idle()).await;
    open_task(&server, open_task_old, TWO_HOURS + 60).await;
    let hitl_old = seed_agent(&server, &runtime, Seed::idle()).await;
    pending_hitl(&server, hitl_old, TWO_HOURS + 60).await;

    run_reaper(&server).await;

    for held in [open_task_young, hitl_young] {
        assert_eq!(status(&server, held).await, "running");
        assert_eq!(runtime.replicas_of(held), Some(1));
    }
    for parked in [open_task_old, hitl_old] {
        assert_eq!(status(&server, parked).await, "parked");
        assert_eq!(runtime.replicas_of(parked), Some(0));
    }
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn resident_harness_and_coding_agents_are_never_parked() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let resident = seed_agent(
        &server,
        &runtime,
        Seed {
            policy: "resident",
            ..Seed::idle()
        },
    )
    .await;
    let harness = seed_agent(
        &server,
        &runtime,
        Seed {
            kind: "harness",
            ..Seed::idle()
        },
    )
    .await;
    let coding = seed_agent(
        &server,
        &runtime,
        Seed {
            kind: "acp_coding_agent",
            ..Seed::idle()
        },
    )
    .await;

    run_reaper(&server).await;

    for agent in [resident, harness, coding] {
        assert_eq!(status(&server, agent).await, "running");
        assert_eq!(runtime.replicas_of(agent), Some(1));
    }
    server.cleanup().await;
}

/// Session suspension off (`CODING_AGENT_IDLE_TTL_SECS=0`, a recovery-only sweep) does not turn
/// agent parking off: `AGENT_PARK_IDLE_SECS` alone does.
#[tokio::test]
#[serial]
async fn with_session_suspension_off_an_idle_agent_is_still_parked() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;

    sweep(
        &server.state,
        &mut ReaperMemory::default(),
        SweepScope::RecoveryOnly,
    )
    .await
    .expect("sweep");

    assert_eq!(status(&server, agent).await, "parked");
    assert_eq!(runtime.replicas_of(agent), Some(0));
    server.cleanup().await;
}

/// An edition that parks every framework agent, whatever its owner chose: the scaling seam the
/// test server wires, with `resident` not honoured.
struct ParksEveryAgent(Arc<dyn AgentScaling>);

#[async_trait]
impl AgentScaling for ParksEveryAgent {
    async fn begin_deploy(&self, agent_id: Uuid) -> Box<dyn AgentDeploy> {
        self.0.begin_deploy(agent_id).await
    }

    async fn is_autoscaled(&self, agent_id: Uuid) -> sqlx::Result<bool> {
        self.0.is_autoscaled(agent_id).await
    }

    fn agent_became_durable(&self, agent_id: Uuid) {
        self.0.agent_became_durable(agent_id)
    }

    fn observe_hitl_store(
        &self,
        store: Arc<dyn nasiko_hitl::HitlStore>,
    ) -> Arc<dyn nasiko_hitl::HitlStore> {
        self.0.observe_hitl_store(store)
    }

    fn honours_resident_policy(&self) -> bool {
        false
    }
}

/// Where an edition does not honour `resident`, an idle resident framework agent is parked like
/// any other; harness and coding agents still never are.
#[tokio::test]
#[serial]
async fn where_resident_is_not_honoured_a_resident_agent_is_parked() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let resident = seed_agent(
        &server,
        &runtime,
        Seed {
            policy: "resident",
            ..Seed::idle()
        },
    )
    .await;
    let harness = seed_agent(
        &server,
        &runtime,
        Seed {
            kind: "harness",
            policy: "resident",
            ..Seed::idle()
        },
    )
    .await;
    let mut state = server.state.clone();
    state.agent_scaling = Arc::new(ParksEveryAgent(state.agent_scaling.clone()));

    sweep(&state, &mut ReaperMemory::default(), SweepScope::Full)
        .await
        .expect("sweep");

    assert_eq!(status(&server, resident).await, "parked");
    assert_eq!(runtime.replicas_of(resident), Some(0));
    assert_eq!(status(&server, harness).await, "running");
    assert_eq!(runtime.replicas_of(harness), Some(1));
    server.cleanup().await;
}

/// Makes the owner of `agent` a superuser: the agent becomes one of the platform's own.
async fn owned_by_a_superuser(server: &common::TestServer, agent: Uuid) {
    sqlx::query(
        "UPDATE users SET is_superuser = true \
         WHERE id = (SELECT owner_id FROM agents WHERE id = $1)",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .expect("make the owner a superuser");
}

/// The platform's own agents (a superuser's, as `SEED_AGENTS` registers them) are shared by every
/// user: where `resident` is not honoured, a superuser's resident agent still stays up, and one
/// left at `auto` is parked like any other.
#[tokio::test]
#[serial]
async fn where_resident_is_not_honoured_a_superusers_resident_agent_stays_up() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let platform_resident = seed_agent(
        &server,
        &runtime,
        Seed {
            policy: "resident",
            ..Seed::idle()
        },
    )
    .await;
    owned_by_a_superuser(&server, platform_resident).await;
    let platform_auto = seed_agent(&server, &runtime, Seed::idle()).await;
    owned_by_a_superuser(&server, platform_auto).await;
    let mut state = server.state.clone();
    state.agent_scaling = Arc::new(ParksEveryAgent(state.agent_scaling.clone()));

    sweep(&state, &mut ReaperMemory::default(), SweepScope::Full)
        .await
        .expect("sweep");

    assert_eq!(status(&server, platform_resident).await, "running");
    assert_eq!(runtime.replicas_of(platform_resident), Some(1));
    assert!(
        !nasiko_server::agent_scaling::runs_parkable(
            state.agent_scaling.as_ref(),
            &server.db,
            platform_resident
        )
        .await
        .expect("runs_parkable")
    );
    assert_eq!(status(&server, platform_auto).await, "parked");
    assert_eq!(runtime.replicas_of(platform_auto), Some(0));
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_scale_that_fails_leaves_the_agent_running() {
    let runtime = WorkloadRuntime::answering_at(String::new());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    seed_deployments(&server, agent).await;
    runtime.fail_scale_down.store(true, Ordering::SeqCst);

    run_reaper(&server).await;

    assert_eq!(
        status_and_url(&server, agent).await,
        (
            "running".to_owned(),
            Some("http://10.0.0.7:8000".to_owned())
        ),
        "the claim rolled back with the failed scale"
    );
    assert_eq!(
        deployment_statuses(&server, agent).await,
        vec!["running".to_owned(), "stopped".to_owned()]
    );
    assert_eq!(runtime.replicas_of(agent), Some(1));
    server.cleanup().await;
}

/// The whole cycle: parked, woken by the next call, and parked again only once it has been idle
/// for the window since that call.
#[tokio::test]
#[serial]
async fn a_woken_agent_parks_again_once_idle_for_the_window() {
    let runtime = WorkloadRuntime::answering_at(start_answering_agent().await);
    let server = server_over(&runtime).await;
    install_platform_wake(&server, &runtime);
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    run_reaper(&server).await;
    assert_eq!(status(&server, agent).await, "parked");

    server
        .state
        .agent_wake
        .before_forward(agent, None)
        .await
        .expect("the next call wakes it");
    assert_eq!(runtime.replicas_of(agent), Some(1));
    assert_eq!(status(&server, agent).await, "running");

    run_reaper(&server).await;
    assert_eq!(
        status(&server, agent).await,
        "running",
        "it was just called"
    );
    assert_eq!(runtime.replicas_of(agent), Some(1));

    set_idle_clock(&server, agent, Some(TWO_HOURS), TWO_HOURS).await;
    run_reaper(&server).await;
    assert_eq!(status(&server, agent).await, "parked");
    assert_eq!(runtime.replicas_of(agent), Some(0));
    server.cleanup().await;
}

/// The platform's wake over `runtime`, as the composition root wires it (the test server installs
/// none by default).
fn install_platform_wake(server: &common::TestServer, runtime: &Arc<WorkloadRuntime>) {
    let (deployer, requests) = nasiko_server::agents::catalog_deploy::deployer();
    tokio::spawn(nasiko_server::agents::catalog_deploy::run(
        server.state.clone(),
        requests,
    ));
    server
        .state
        .agent_wake
        .install(Arc::new(ParkedAgentWake::new(
            WakeDeps {
                db: server.db.clone(),
                runtime: runtime.clone(),
                http: server.state.http_client.clone(),
                flow_events: server.flow_events.clone(),
                deployer: Arc::new(deployer),
            },
            std::time::Duration::from_secs(5),
        )));
}

/// The park committed after this request read the row as running and resolved its workload,
/// which in the race is still up. The row is marked parked here as the park leaves it.
async fn park_row_only(server: &common::TestServer, agent: Uuid) {
    sqlx::query(
        "UPDATE agents SET status = 'parked', url = NULL, last_request_at = NULL WHERE id = $1",
    )
    .bind(agent)
    .execute(&server.db)
    .await
    .expect("mark the row parked");
}

/// A request that read the row as running and resolved the workload just before the park
/// committed finds the row parked when it stamps it: the agent is woken again and served, never
/// forwarded to a workload going down.
#[tokio::test]
#[serial]
async fn a_request_that_resolved_a_workload_as_it_was_parked_wakes_it_again() {
    let address = start_answering_agent().await;
    let runtime = WorkloadRuntime::answering_at(address.clone());
    let server = server_over(&runtime).await;
    install_platform_wake(&server, &runtime);
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    let read_as_running = load_agent(&server.state, agent).await.unwrap();
    park_row_only(&server, agent).await;

    let resolved = resolve_loaded(&server.state, &read_as_running, &Caller::default())
        .await
        .expect("resolved after the wake");

    assert!(
        matches!(&resolved, ResolvedTarget::Container(t) if t.base.url == address),
        "{resolved:?}"
    );
    assert_eq!(
        status_and_url(&server, agent).await,
        ("running".to_owned(), Some(address)),
        "the wake undid the park"
    );
    server.cleanup().await;
}

/// The same race when the wake cannot bring the agent back in this request (no platform wake is
/// installed here): a 503 to retry on, and the row stays parked for the next call to wake.
#[tokio::test]
#[serial]
async fn a_request_that_lost_the_park_race_gets_a_503_to_retry_when_the_wake_fails() {
    let runtime = WorkloadRuntime::answering_at("http://127.0.0.1:1".to_owned());
    let server = server_over(&runtime).await;
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    let read_as_running = load_agent(&server.state, agent).await.unwrap();
    park_row_only(&server, agent).await;

    let refused = resolve_loaded(&server.state, &read_as_running, &Caller::default()).await;

    let err = refused.expect_err("nothing woke it");
    assert!(matches!(err, ResolveError::ParkedMidRequest(_)), "{err:?}");
    assert_eq!(err.http_status().as_u16(), 503);
    assert!(err.client_message().contains("try again"));
    assert_eq!(status(&server, agent).await, "parked");
    server.cleanup().await;
}

/// The same race when the wake is refused on its merits (a layer declined to start the agent): the
/// 503 carries the refusal's own words, still with a `Retry-After`, and the row stays parked.
#[tokio::test]
#[serial]
async fn a_request_that_lost_the_park_race_is_told_why_when_the_wake_is_declined() {
    const MESSAGE: &str =
        "This workspace cannot start more agents. Ask an admin to change the plan.";
    let runtime = WorkloadRuntime::answering_at(start_answering_agent().await);
    *runtime.decline_scale_up.lock().unwrap() =
        Some(("plan_refused".to_owned(), MESSAGE.to_owned()));
    let server = server_over(&runtime).await;
    install_platform_wake(&server, &runtime);
    let agent = seed_agent(&server, &runtime, Seed::idle()).await;
    let read_as_running = load_agent(&server.state, agent).await.unwrap();
    park_row_only(&server, agent).await;

    let refused = resolve_loaded(&server.state, &read_as_running, &Caller::default()).await;

    let err = refused.expect_err("the wake was declined");
    assert!(
        matches!(&err, ResolveError::ParkedMidRequest(m) if m == MESSAGE),
        "{err:?}"
    );
    assert_eq!(err.http_status().as_u16(), 503);
    assert_eq!(err.client_message(), MESSAGE);
    assert!(err.retry_after_secs().is_some());
    assert_eq!(status(&server, agent).await, "parked");
    server.cleanup().await;
}
