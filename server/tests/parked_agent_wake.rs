//! The platform's default wake (`agent_wake::parked::ParkedAgentWake`) against a real database: a
//! parked agent's own workload is scaled back to one replica and waited for until it answers, once
//! however many callers arrive together, and the park is undone (status, address, deployment row);
//! a row parked without a workload gets one deployed from its catalog row, the spec a redeploy
//! gets; a wake dropped mid-way releases the agent's lock; and every request a workload serves
//! stamps the agent's idle clock, at most once a minute.
//!
//! Requires infra (Postgres, Redis, S3) like the rest of the suite:
//!   cargo test -p nasiko-server --test parked_agent_wake -- --test-threads=1

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, routing::post};
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, Result as RuntimeResult,
    RuntimeError, RuntimeState,
};
use nasiko_server::agent_resolver::{Caller, ResolveError, resolve_target};
use nasiko_server::agent_wake::parked::{ParkedAgentWake, WakeDeps, WorkloadDeployer};
use nasiko_server::agent_wake::{AGENT_LOCK_KEY_SQL, AgentWake, WakeRefusal};
use nasiko_server::agents::catalog_deploy;
use serde_json::json;
use serial_test::serial;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Notify;
use uuid::Uuid;

/// A runtime whose agents' workloads scale to and from zero, as Docker and Kubernetes do: an agent
/// with no entry has no workload at all.
#[derive(Default)]
struct WakeableRuntime {
    /// The address a running workload answers at.
    address: String,
    replicas: Mutex<HashMap<ContainerId, u32>>,
    /// A started workload that never reaches Running.
    never_runs: AtomicBool,
    /// How long `scale` takes, so concurrent wakes overlap.
    scale_delay: Duration,
    /// While set, every `scale` waits on it until `release_scales`.
    scale_hold: Mutex<Option<Arc<Notify>>>,
    /// Scales that reached the hold.
    scales_held: AtomicUsize,
    scale_ups: AtomicUsize,
    /// Scales to zero: what a failed wake's undo does to the workload it started.
    scale_downs: AtomicUsize,
    /// Every `scale`, `status` and `endpoint` call.
    runtime_calls: AtomicUsize,
    /// Every spec `deploy` was given.
    deployed: Mutex<Vec<DeploymentSpec>>,
    /// The `(code, message)` every start is refused with, as a layer that declines to run
    /// workloads answers: a scale-up of a workload that exists, and a deploy.
    declined: Mutex<Option<(String, String)>>,
}

impl WakeableRuntime {
    fn answering_at(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            ..Self::default()
        }
    }

    /// The agent's workload exists, scaled to zero.
    fn with_parked_workload(self, agent_id: Uuid) -> Self {
        self.replicas
            .lock()
            .unwrap()
            .insert(ContainerId::from_uuid(agent_id), 0);
        self
    }

    fn declining_starts(self, code: &str, message: &str) -> Self {
        *self.declined.lock().unwrap() = Some((code.to_owned(), message.to_owned()));
        self
    }

    /// The layer that declined stops declining.
    fn allow_starts(&self) {
        *self.declined.lock().unwrap() = None;
    }

    fn decline(&self) -> RuntimeResult<()> {
        match self.declined.lock().unwrap().clone() {
            Some((code, message)) => Err(RuntimeError::Declined { code, message }),
            None => Ok(()),
        }
    }

    fn scaling_slowly(mut self) -> Self {
        self.scale_delay = Duration::from_millis(300);
        self
    }

    fn never_running(self) -> Self {
        self.never_runs.store(true, Ordering::SeqCst);
        self
    }

    fn hold_scales(&self) {
        *self.scale_hold.lock().unwrap() = Some(Arc::new(Notify::new()));
    }

    fn release_scales(&self) {
        if let Some(hold) = self.scale_hold.lock().unwrap().take() {
            hold.notify_waiters();
        }
    }

    fn scales_held(&self) -> usize {
        self.scales_held.load(Ordering::SeqCst)
    }

    fn replicas_of(&self, agent_id: Uuid) -> Option<u32> {
        self.replicas
            .lock()
            .unwrap()
            .get(&ContainerId::from_uuid(agent_id))
            .copied()
    }

    fn scale_ups(&self) -> usize {
        self.scale_ups.load(Ordering::SeqCst)
    }

    fn scale_downs(&self) -> usize {
        self.scale_downs.load(Ordering::SeqCst)
    }

    fn runtime_calls(&self) -> usize {
        self.runtime_calls.load(Ordering::SeqCst)
    }

    fn deployed(&self) -> Vec<DeploymentSpec> {
        self.deployed.lock().unwrap().clone()
    }

    fn serving(&self, id: &ContainerId) -> bool {
        let replicas = self.replicas.lock().unwrap().get(id).copied();
        replicas.is_some_and(|n| n > 0) && !self.never_runs.load(Ordering::SeqCst)
    }

    fn status_of(&self, id: &ContainerId) -> DeploymentStatus {
        let replicas = self.replicas.lock().unwrap().get(id).copied();
        let state = match replicas {
            None => RuntimeState::Unknown,
            Some(0) => RuntimeState::Stopped,
            Some(_) if self.never_runs.load(Ordering::SeqCst) => RuntimeState::Pending,
            Some(_) => RuntimeState::Running,
        };
        DeploymentStatus {
            container_id: id.clone(),
            state,
            replicas_live: u32::from(self.serving(id)),
            endpoint: None,
            message: None,
            restart_count: 0,
        }
    }
}

#[async_trait]
impl ContainerRuntime for WakeableRuntime {
    async fn deploy(&self, spec: &DeploymentSpec) -> RuntimeResult<DeploymentStatus> {
        self.decline()?;
        self.deployed.lock().unwrap().push(spec.clone());
        self.replicas
            .lock()
            .unwrap()
            .insert(spec.container_id.clone(), 1);
        Ok(self.status_of(&spec.container_id))
    }

    async fn destroy(&self, id: &ContainerId) -> RuntimeResult<()> {
        self.replicas.lock().unwrap().remove(id);
        Ok(())
    }

    async fn scale(&self, id: &ContainerId, replicas: u32) -> RuntimeResult<()> {
        self.runtime_calls.fetch_add(1, Ordering::SeqCst);
        if replicas > 0 {
            self.scale_ups.fetch_add(1, Ordering::SeqCst);
        } else {
            self.scale_downs.fetch_add(1, Ordering::SeqCst);
        }
        tokio::time::sleep(self.scale_delay).await;
        let hold = self.scale_hold.lock().unwrap().clone();
        if let Some(hold) = hold {
            self.scales_held.fetch_add(1, Ordering::SeqCst);
            hold.notified().await;
        }
        match self.replicas.lock().unwrap().get_mut(id) {
            Some(current) => {
                if replicas > 0 {
                    self.decline()?;
                }
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
        self.runtime_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.status_of(id))
    }

    async fn list(&self) -> RuntimeResult<Vec<DeploymentStatus>> {
        Ok(Vec::new())
    }

    async fn endpoint(&self, id: &ContainerId) -> RuntimeResult<String> {
        self.runtime_calls.fetch_add(1, Ordering::SeqCst);
        if self.serving(id) {
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

const BUDGET: Duration = Duration::from_secs(5);

/// What a layer that refuses to start workloads says: its own code, and words fit for the caller.
const DECLINED_CODE: &str = "plan_refused";
const DECLINED_MESSAGE: &str =
    "This workspace cannot start more agents. Ask an admin to change the plan.";

/// A deployer that cannot deploy: the tests that use it have a workload to scale, or show what a
/// failed deploy leaves.
struct NoDeploy;

#[async_trait]
impl WorkloadDeployer for NoDeploy {
    async fn deploy(&self, _agent_id: Uuid) -> Result<(), WakeRefusal> {
        Err("the agent's workload could not be deployed"
            .to_owned()
            .into())
    }
}

/// A wake over `runtime`, on a pool of its own wide enough for a wake to hold its transaction
/// while the test reads the database.
async fn wake_over(server: &common::TestServer, runtime: Arc<WakeableRuntime>) -> ParkedAgentWake {
    wake_with(server, runtime, Arc::new(NoDeploy)).await
}

async fn wake_with(
    server: &common::TestServer,
    runtime: Arc<WakeableRuntime>,
    deployer: Arc<dyn WorkloadDeployer>,
) -> ParkedAgentWake {
    let db = PgPoolOptions::new()
        .max_connections(4)
        .connect_with((*server.db.connect_options()).clone())
        .await
        .expect("connect a wider pool to the test database");
    let deps = WakeDeps {
        db,
        runtime,
        http: reqwest::Client::new(),
        flow_events: server.flow_events.clone(),
        deployer,
    };
    ParkedAgentWake::new(deps, BUDGET)
}

/// The platform's deployer over `server`'s state, as the composition root wires it.
fn catalog_deployer(server: &common::TestServer) -> Arc<dyn WorkloadDeployer> {
    let (deployer, requests) = catalog_deploy::deployer();
    tokio::spawn(catalog_deploy::run(server.state.clone(), requests));
    Arc::new(deployer)
}

async fn seed_owner(server: &common::TestServer) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("owner_{}", Uuid::new_v4().simple()))
        .bind(format!("{}@test.example", Uuid::new_v4().simple()))
        .fetch_one(&server.db)
        .await
        .expect("seed owner")
}

async fn seed_agent(server: &common::TestServer, status: &str, url: Option<&str>) -> Uuid {
    let owner = seed_owner(server).await;
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url) \
         VALUES ($1, $2, 'x:1.0.0', $3, $4) RETURNING id",
    )
    .bind(format!("parked-wake-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(status)
    .bind(url)
    .fetch_one(&server.db)
    .await
    .expect("seed agent")
}

/// The row's status, and whether its last request was stamped within the last few seconds.
async fn status_and_fresh_stamp(server: &common::TestServer, agent: Uuid) -> (String, bool) {
    sqlx::query_as(
        "SELECT status, COALESCE(last_request_at > now() - interval '5 seconds', false) \
         FROM agents WHERE id = $1",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .expect("read the agent row")
}

async fn stored_url(server: &common::TestServer, agent: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT url FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .expect("read the agent's url")
}

/// Two deployment rows, an old one and the newest, both stopped as a park leaves them.
async fn seed_parked_deployments(server: &common::TestServer, agent: Uuid) {
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
         ($1, $2, 'stopped', now() - interval '1 hour'), ($1, $2, 'stopped', now())",
    )
    .bind(agent)
    .bind(build)
    .execute(&server.db)
    .await
    .expect("seed deployments");
}

/// The agent's deployment rows' statuses, newest first.
async fn deployment_statuses(server: &common::TestServer, agent: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT status::text FROM agent_deployments WHERE agent_id = $1 ORDER BY created_at DESC",
    )
    .bind(agent)
    .fetch_all(&server.db)
    .await
    .expect("read deployments")
}

/// How many sessions hold `agent`'s lock: a wake's lock must not outlive its transaction.
///
/// That one key only (a single-key lock's 64 bits are `classid` high and `objid` low): background
/// work in the test server (the reaper, erasure retries, `nasiko_leader`'s leaders) takes advisory
/// locks of its own at any moment, and counting every lock made these assertions flaky.
async fn agent_locks_held(db: &PgPool, agent: Uuid) -> i64 {
    sqlx::query_scalar(&format!(
        "WITH k AS (SELECT {AGENT_LOCK_KEY_SQL} AS key) \
         SELECT count(*) FROM pg_locks, k \
         WHERE locktype = 'advisory' AND granted AND objsubid = 1 \
           AND classid = ((k.key >> 32) & 4294967295)::oid \
           AND objid = (k.key & 4294967295)::oid \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database())"
    ))
    .bind(agent.to_string())
    .fetch_one(db)
    .await
    .expect("read pg_locks")
}

async fn eventually<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn set_last_request_ago(server: &common::TestServer, agent: Uuid, secs: i64) {
    sqlx::query(
        "UPDATE agents SET last_request_at = now() - ($2::bigint * interval '1 second') \
         WHERE id = $1",
    )
    .bind(agent)
    .bind(secs)
    .execute(&server.db)
    .await
    .expect("set last_request_at");
}

#[tokio::test]
#[serial]
async fn a_call_to_an_agent_that_is_not_parked_touches_no_runtime() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "running", Some("http://127.0.0.1:1")).await;
    let runtime = Arc::new(WakeableRuntime::default().with_parked_workload(agent));
    let wake = wake_over(&server, runtime.clone()).await;

    assert_eq!(wake.before_forward(agent, None).await, Ok(()));
    assert_eq!(
        wake.before_forward(Uuid::new_v4(), None).await,
        Ok(()),
        "an unknown agent is the resolver's to refuse"
    );

    assert_eq!(runtime.runtime_calls(), 0);
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_parked_agent_is_scaled_up_waited_for_and_its_park_undone() {
    let (address, _hits) = start_counting_agent().await;
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    seed_parked_deployments(&server, agent).await;
    let runtime =
        Arc::new(WakeableRuntime::answering_at(address.clone()).with_parked_workload(agent));
    let wake = wake_over(&server, runtime.clone()).await;
    let mut events = server.flow_events.subscribe("parked-wake-flow").await;

    let woke = wake.before_forward(agent, Some("parked-wake-flow")).await;

    assert_eq!(woke, Ok(()));
    assert_eq!(runtime.replicas_of(agent), Some(1));
    assert_eq!(runtime.scale_ups(), 1);
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true)
    );
    assert_eq!(
        stored_url(&server, agent).await,
        Some(address),
        "the park cleared it"
    );
    assert_eq!(
        deployment_statuses(&server, agent).await,
        vec!["running".to_owned(), "stopped".to_owned()],
        "the newest deployment is live again; history stays as it was"
    );
    let event = events
        .try_recv()
        .expect("the wake was announced on the flow");
    assert!(
        matches!(&event, nasiko_flow::FlowEvent::AgentWaking { agent_id, wake_ms: None } if *agent_id == agent.to_string()),
        "{event:?}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn concurrent_calls_to_a_parked_agent_wake_it_once() {
    let (address, _hits) = start_counting_agent().await;
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(
        WakeableRuntime::answering_at(address)
            .with_parked_workload(agent)
            .scaling_slowly(),
    );
    let wake = Arc::new(wake_over(&server, runtime.clone()).await);

    let calls = (0..3).map(|_| {
        let wake = wake.clone();
        tokio::spawn(async move { wake.before_forward(agent, None).await })
    });
    let outcomes = futures::future::join_all(calls).await;

    for outcome in outcomes {
        assert_eq!(outcome.expect("the call task"), Ok(()));
    }
    assert_eq!(runtime.scale_ups(), 1, "the later callers found it woken");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
}

/// A row parked before parking scaled workloads to zero has no workload (that parking destroyed
/// it): the wake deploys one from the agent's catalog row, with the port its last deployment
/// recorded and as a parkable workload, waits for it as for any other, and undoes the park.
#[tokio::test]
#[serial]
async fn an_agent_parked_without_a_workload_gets_one_deployed_and_is_woken() {
    let (address, _hits) = start_counting_agent().await;
    let runtime = Arc::new(WakeableRuntime::answering_at(address.clone()));
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let agent = seed_agent(&server, "parked", None).await;
    seed_parked_deployments(&server, agent).await;
    sqlx::query("UPDATE agent_deployments SET spec_ports = '{9100}' WHERE agent_id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    let wake = wake_with(&server, runtime.clone(), catalog_deployer(&server)).await;

    assert_eq!(wake.before_forward(agent, None).await, Ok(()));

    let deployed = runtime.deployed();
    assert_eq!(deployed.len(), 1, "one deploy of the missing workload");
    let spec = &deployed[0];
    assert_eq!(spec.container_id, ContainerId::from_uuid(agent));
    assert_eq!(
        spec.ports,
        vec![9100],
        "the port its last deployment recorded"
    );
    assert_eq!(spec.env_vars.get("PORT").map(String::as_str), Some("9100"));
    assert!(
        spec.parkable,
        "an `auto` agent deploys a workload that can be parked"
    );
    assert_eq!(runtime.replicas_of(agent), Some(1));
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true)
    );
    assert_eq!(stored_url(&server, agent).await, Some(address));
    assert_eq!(
        deployment_statuses(&server, agent).await,
        vec![
            "running".to_owned(),
            "stopped".to_owned(),
            "stopped".to_owned()
        ],
        "the new workload is recorded for the crash guardian; history stays as it was"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_parked_agent_whose_missing_workload_cannot_be_deployed_is_refused_and_stays_parked() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(WakeableRuntime::default());
    let wake = wake_over(&server, runtime.clone()).await;

    let refused = wake.before_forward(agent, None).await;

    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false)
    );
    // The failed wake's undo re-parks in the background under the agent's lock
    // (`ParkAgainUnlessWoken`), so the lock is released shortly after the refusal, not at it.
    eventually("the failed wake's lock to be released", || async {
        agent_locks_held(&server.db, agent).await == 0
    })
    .await;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_agent_that_does_not_come_up_in_time_is_refused_and_stays_parked() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(
        WakeableRuntime::default()
            .with_parked_workload(agent)
            .never_running(),
    );
    let wake = wake_over(&server, runtime.clone()).await;
    let started = std::time::Instant::now();

    let refused = wake.before_forward(agent, None).await;

    assert_eq!(
        refused,
        Err("the agent did not start in time, try again shortly".to_owned())
    );
    assert!(
        started.elapsed() < BUDGET + Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false),
        "the next call tries again"
    );
    server.cleanup().await;
}

/// A layer that refuses to start the workload says why: the caller is told those words, not the
/// wake's own "try again", and the agent stays parked for the next call.
#[tokio::test]
#[serial]
async fn a_wake_the_runtime_declines_is_refused_in_the_declines_own_words_and_stays_parked() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(
        WakeableRuntime::default()
            .with_parked_workload(agent)
            .declining_starts(DECLINED_CODE, DECLINED_MESSAGE),
    );
    let wake = wake_over(&server, runtime.clone()).await;

    let refused = wake.before_forward(agent, None).await;

    assert_eq!(refused, Err(DECLINED_MESSAGE.to_owned()));
    assert_eq!(runtime.replicas_of(agent), Some(0), "nothing was started");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false)
    );
    server.cleanup().await;
}

/// The same refusal when the wake has no workload to scale and deploys one from the catalog: the
/// deploy's refusal is relayed through the deploy worker, not replaced by its generic message.
#[tokio::test]
#[serial]
async fn a_missing_workloads_deploy_the_runtime_declines_is_refused_in_the_declines_own_words() {
    let runtime =
        Arc::new(WakeableRuntime::default().declining_starts(DECLINED_CODE, DECLINED_MESSAGE));
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let agent = seed_agent(&server, "parked", None).await;
    seed_parked_deployments(&server, agent).await;
    let wake = wake_with(&server, runtime.clone(), catalog_deployer(&server)).await;

    let refused = wake.before_forward(agent, None).await;

    assert_eq!(refused, Err(DECLINED_MESSAGE.to_owned()));
    assert!(runtime.deployed().is_empty(), "nothing was deployed");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false)
    );
    server.cleanup().await;
}

/// A caller that retries (the HITL resume) must tell a decline from a failure, on both ways a wake
/// can start an agent: scaling the workload it has, and deploying the one it lacks.
#[tokio::test]
#[serial]
async fn a_wake_refusal_says_the_runtime_declined_with_a_workload_and_without_one() {
    let runtime =
        Arc::new(WakeableRuntime::default().declining_starts(DECLINED_CODE, DECLINED_MESSAGE));
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let with_workload = seed_agent(&server, "parked", None).await;
    runtime
        .replicas
        .lock()
        .unwrap()
        .insert(ContainerId::from_uuid(with_workload), 0);
    let without_workload = seed_agent(&server, "parked", None).await;
    seed_parked_deployments(&server, without_workload).await;
    let wake = wake_with(&server, runtime.clone(), catalog_deployer(&server)).await;
    let declined = Err(WakeRefusal {
        message: DECLINED_MESSAGE.to_owned(),
        declined: true,
    });

    assert_eq!(
        wake.before_forward_or_decline(with_workload, None).await,
        declined
    );
    assert_eq!(
        wake.before_forward_or_decline(without_workload, None).await,
        declined
    );
    server.cleanup().await;
}

/// The contrast: a deploy that fails for any other reason is a failure, not a decline.
#[tokio::test]
#[serial]
async fn a_wake_refusal_that_is_not_a_decline_says_so() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let wake = wake_over(&server, Arc::new(WakeableRuntime::default())).await;

    let refused = wake.before_forward_or_decline(agent, None).await;

    assert_eq!(
        refused,
        Err(WakeRefusal {
            message: "the agent's workload could not be deployed".to_owned(),
            declined: false,
        })
    );
    server.cleanup().await;
}

/// A decline comes before anything started, so there is nothing to scale back down: the failed
/// wake's undo is disarmed, and no scale to zero is scheduled behind the refusal.
#[tokio::test]
#[serial]
async fn a_wake_the_runtime_declines_schedules_no_scale_to_zero() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(
        WakeableRuntime::default()
            .with_parked_workload(agent)
            .declining_starts(DECLINED_CODE, DECLINED_MESSAGE),
    );
    let wake = wake_over(&server, runtime.clone()).await;

    let refused = wake.before_forward(agent, None).await;

    assert_eq!(refused, Err(DECLINED_MESSAGE.to_owned()));
    // An armed undo runs in the background as soon as the wake returns, so give it time to show.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(runtime.scale_ups(), 1, "the one attempt to start it");
    assert_eq!(runtime.scale_downs(), 0, "nothing was started to stop");
    assert_eq!(agent_locks_held(&server.db, agent).await, 0);
    server.cleanup().await;
}

/// The contrast that shows the test above can see an undo: a wake that fails after the workload
/// started scales it back to zero.
#[tokio::test]
#[serial]
async fn a_wake_that_fails_after_the_workload_started_scales_it_back_to_zero() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(
        WakeableRuntime::default()
            .with_parked_workload(agent)
            .never_running(),
    );
    let wake = wake_over(&server, runtime.clone()).await;

    let refused = wake.before_forward(agent, None).await;

    assert!(refused.is_err(), "{refused:?}");
    eventually(
        "the failed wake's workload to be scaled to zero",
        || async { runtime.scale_downs() == 1 },
    )
    .await;
    assert_eq!(runtime.replicas_of(agent), Some(0));
    server.cleanup().await;
}

/// A caller that goes away mid-wake (a client disconnect drops the handler's future) must not
/// leave the agent's lock held or its row changed: the next call wakes it.
#[tokio::test]
#[serial]
async fn a_wake_dropped_mid_flight_releases_the_lock_and_keeps_the_row() {
    let (address, _hits) = start_counting_agent().await;
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let runtime = Arc::new(WakeableRuntime::answering_at(address).with_parked_workload(agent));
    let wake = Arc::new(wake_over(&server, runtime.clone()).await);
    runtime.hold_scales();
    let waking = tokio::spawn({
        let wake = wake.clone();
        async move { wake.before_forward(agent, None).await }
    });
    eventually("the wake to be inside scale(1)", || async {
        runtime.scales_held() == 1
    })
    .await;
    assert_eq!(
        agent_locks_held(&server.db, agent).await,
        1,
        "precondition: the wake holds the agent's lock"
    );

    waking.abort();
    assert!(waking.await.unwrap_err().is_cancelled());

    eventually("the dropped wake to release the agent's lock", || async {
        agent_locks_held(&server.db, agent).await == 0
    })
    .await;
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false),
        "the row stays for the next call"
    );
    runtime.release_scales();
    assert_eq!(wake.before_forward(agent, None).await, Ok(()));
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
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

/// A parked agent of `server`, its workload scaled to zero in `runtime`, called through the
/// proxy with the platform's wake installed over `runtime`.
async fn call_a_parked_agent_through_the_proxy(
    server: &common::TestServer,
    runtime: &Arc<WakeableRuntime>,
) -> (Uuid, reqwest::Response) {
    let agent = seed_agent(server, "parked", None).await;
    runtime
        .replicas
        .lock()
        .unwrap()
        .insert(ContainerId::from_uuid(agent), 0);
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let wake = ParkedAgentWake::new(
        WakeDeps {
            db: server.db.clone(),
            runtime: runtime.clone(),
            http: server.state.http_client.clone(),
            flow_events: server.flow_events.clone(),
            deployer: Arc::new(NoDeploy),
        },
        BUDGET,
    );
    server.state.agent_wake.install(Arc::new(wake));
    let body = json!({
        "jsonrpc": "2.0", "id": "1", "method": "message/send",
        "params": {"message": {
            "messageId": Uuid::new_v4().to_string(),
            "role": "ROLE_USER",
            "parts": [{"text": "hello"}]
        }}
    });

    let res = common::as_superuser(
        server
            .client
            .post(server.url(&format!("/api/agents/{agent}")))
            .json(&body),
        &owner.to_string(),
        "parked-wake",
    )
    .send()
    .await
    .expect("request");
    (agent, res)
}

/// The whole path: the proxy wakes the parked agent before it resolves, re-reads the row the wake
/// changed, and forwards to the agent's own workload.
#[tokio::test]
#[serial]
async fn the_proxy_serves_a_woken_agent_from_its_own_workload() {
    let (address, hits) = start_counting_agent().await;
    let runtime = Arc::new(WakeableRuntime::answering_at(address));
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;

    let (agent, res) = call_a_parked_agent_through_the_proxy(&server, &runtime).await;

    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scale_ups(), 1);
    assert!(
        runtime.deployed().is_empty(),
        "its own workload, not a new one"
    );
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true)
    );
    server.cleanup().await;
}

/// The refusal reaches the proxy's client: a 503 whose body is the declining layer's words, with
/// nothing sent to the agent.
#[tokio::test]
#[serial]
async fn the_proxy_answers_503_with_the_declines_words_when_waking_a_parked_agent_is_declined() {
    let (address, hits) = start_counting_agent().await;
    let runtime = Arc::new(
        WakeableRuntime::answering_at(address).declining_starts(DECLINED_CODE, DECLINED_MESSAGE),
    );
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;

    let (agent, res) = call_a_parked_agent_through_the_proxy(&server, &runtime).await;

    assert_eq!(res.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.text().await.expect("body"), DECLINED_MESSAGE);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing reaches the agent");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("parked".to_owned(), false)
    );
    server.cleanup().await;
}

/// The reaper measures a running agent's idleness from `last_request_at`, so the resident path
/// stamps it too, at most once a minute, and never revives a row whose status moved on.
#[tokio::test]
#[serial]
async fn serving_from_a_workload_stamps_the_idle_clock_at_most_once_a_minute() {
    let (address, _hits) = start_counting_agent().await;
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "running", Some(&address)).await;
    let caller = Caller::default();
    let resolve = || resolve_target(&server.state, agent, &caller);

    resolve().await.expect("resolved");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true),
        "a first request stamps the clock"
    );

    set_last_request_ago(&server, agent, 30).await;
    resolve().await.expect("resolved");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), false),
        "a stamp under a minute old is kept"
    );

    set_last_request_ago(&server, agent, 120).await;
    resolve().await.expect("resolved");
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("running".to_owned(), true),
        "a stamp over a minute old is renewed"
    );

    sqlx::query("UPDATE agents SET status = 'stopped', last_request_at = NULL WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    assert!(matches!(resolve().await, Err(ResolveError::NotRunning(_))));
    assert_eq!(
        status_and_fresh_stamp(&server, agent).await,
        ("stopped".to_owned(), false)
    );
    server.cleanup().await;
}

// ─── The HITL resume wakes the agent it resumes ─────────────────────────────

/// A `resolved` pause on `agent` that its owner answered `answered_secs_ago` seconds ago: a row
/// the HITL dispatcher claims and delivers on its next tick.
async fn seed_answered_pause(
    server: &common::TestServer,
    agent: Uuid,
    answered_secs_ago: i64,
) -> Uuid {
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .expect("read the agent's owner");
    sqlx::query_scalar(
        "INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, expires_at,
             status, human_response, resolved_by, resolved_at)
         VALUES ('input_required', 'direct_chat', $1, $2, $3, $4, $5, now() + interval '3 days',
                 'resolved', $6, $2, now() - ($7::bigint * interval '1 second'))
         RETURNING id",
    )
    .bind(agent)
    .bind(owner)
    .bind(format!("task-{}", Uuid::new_v4()))
    .bind(format!("ctx-{}", Uuid::new_v4()))
    .bind(json!({"message": "what should I do?"}))
    .bind(json!({"answer": "go ahead"}))
    .bind(answered_secs_ago)
    .fetch_one(&server.db)
    .await
    .expect("seed an answered pause")
}

/// What the dispatcher left on a pause once its first delivery try was recorded: the resume
/// status, the attempts it used, the message of the failure, and whether the claim is still held.
async fn resume_after_first_try(
    server: &common::TestServer,
    pause: Uuid,
) -> (String, i32, Option<String>, bool) {
    for _ in 0..200 {
        let row: (String, i32, Option<String>, bool) = sqlx::query_as(
            "SELECT resume_status, resume_dispatch_attempts, resume_last_error, \
                    resume_claimed_at IS NOT NULL \
             FROM hitl_requests WHERE id = $1",
        )
        .bind(pause)
        .fetch_one(&server.db)
        .await
        .expect("read the resume row");
        if row.2.is_some() {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the dispatcher never recorded a try for pause {pause}");
}

/// Makes a held claim stale, as waiting out its lease would.
async fn lapse_the_resume_lease(server: &common::TestServer, pause: Uuid) {
    sqlx::query(
        "UPDATE hitl_requests SET resume_claimed_at = now() - interval '1 hour' WHERE id = $1",
    )
    .bind(pause)
    .execute(&server.db)
    .await
    .expect("lapse the lease");
}

async fn hits_reach(hits: &AtomicUsize, wanted: usize) {
    for _ in 0..200 {
        if hits.load(Ordering::SeqCst) == wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "{} requests reached the agent, not {wanted}",
        hits.load(Ordering::SeqCst)
    );
}

/// A parked agent whose start the runtime declines, with the platform's wake over the runtime
/// installed on the server (which has none by default), and the agent's address answering.
/// `with_workload` says whether the agent has a workload to scale or one to deploy.
async fn parked_agent_whose_start_is_declined(
    with_workload: bool,
) -> (
    common::TestServer,
    Arc<WakeableRuntime>,
    Uuid,
    Arc<AtomicUsize>,
) {
    let (address, hits) = start_counting_agent().await;
    let runtime = Arc::new(
        WakeableRuntime::answering_at(address).declining_starts(DECLINED_CODE, DECLINED_MESSAGE),
    );
    let server = common::TestServer::start_with_runtime(|_| {}, runtime.clone()).await;
    let agent = seed_agent(&server, "parked", None).await;
    seed_parked_deployments(&server, agent).await;
    if with_workload {
        runtime
            .replicas
            .lock()
            .unwrap()
            .insert(ContainerId::from_uuid(agent), 0);
    }
    let wake = wake_with(&server, runtime.clone(), catalog_deployer(&server)).await;
    server.state.agent_wake.install(Arc::new(wake));
    (server, runtime, agent, hits)
}

/// A declined wake is no failed delivery: the answer waits for the start to be allowed instead of
/// running down the five attempts, with the refusal's words kept for whoever looks, and it is
/// delivered once the start is allowed and the claim's lease lapses.
async fn a_declined_resume_waits_and_is_delivered_once_the_start_is_allowed(with_workload: bool) {
    let (server, runtime, agent, hits) = parked_agent_whose_start_is_declined(with_workload).await;
    let pause = seed_answered_pause(&server, agent, 0).await;

    let (status, attempts, last_error, claimed) = resume_after_first_try(&server, pause).await;

    assert_eq!(status, "not_started", "the answer is not lost");
    assert_eq!(attempts, 0, "the declined try is given back");
    assert!(claimed, "the lease holds the next try back until it lapses");
    let last_error = last_error.expect("recorded");
    assert!(last_error.contains(DECLINED_MESSAGE), "{last_error}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    runtime.allow_starts();
    lapse_the_resume_lease(&server, pause).await;
    hits_reach(&hits, 1).await;
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_hitl_resume_whose_wake_is_declined_keeps_its_attempt_and_is_delivered_once_allowed() {
    a_declined_resume_waits_and_is_delivered_once_the_start_is_allowed(true).await;
}

#[tokio::test]
#[serial]
async fn a_hitl_resume_whose_missing_workload_deploy_is_declined_keeps_its_attempt_and_is_delivered_once_allowed()
 {
    a_declined_resume_waits_and_is_delivered_once_the_start_is_allowed(false).await;
}

/// The contrast: a wake that fails for any other reason uses its attempt and releases the claim.
#[tokio::test]
#[serial]
async fn a_hitl_resume_whose_wake_fails_for_another_reason_uses_its_attempt() {
    let server = common::TestServer::start().await;
    let agent = seed_agent(&server, "parked", None).await;
    let wake = wake_over(&server, Arc::new(WakeableRuntime::default())).await;
    server.state.agent_wake.install(Arc::new(wake));
    let pause = seed_answered_pause(&server, agent, 0).await;

    let (status, attempts, last_error, claimed) = resume_after_first_try(&server, pause).await;

    assert_eq!(status, "not_started", "retryable until the cap");
    assert_eq!(attempts, 1);
    assert!(!claimed);
    let last_error = last_error.expect("recorded");
    assert!(
        last_error.contains("the agent's workload could not be deployed"),
        "{last_error}"
    );
    server.cleanup().await;
}

/// The wait for a declined start is bounded: an approval is never delivered days after it was
/// given, so a resume still declined a day after the human answered is terminal with the words.
#[tokio::test]
#[serial]
async fn a_declined_hitl_resume_answered_over_a_day_ago_fails_with_the_message() {
    let (server, _runtime, agent, hits) = parked_agent_whose_start_is_declined(true).await;
    let a_day_and_an_hour = 25 * 60 * 60;
    let pause = seed_answered_pause(&server, agent, a_day_and_an_hour).await;

    let (status, _attempts, last_error, _claimed) = resume_after_first_try(&server, pause).await;

    assert_eq!(status, "failed");
    let last_error = last_error.expect("recorded");
    assert!(last_error.contains(DECLINED_MESSAGE), "{last_error}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    server.cleanup().await;
}
