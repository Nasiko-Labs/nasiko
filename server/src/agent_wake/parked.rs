//! The platform's wake: a parked agent's own workload, scaled back to one replica.
//!
//! An idle parkable agent is parked by scaling its workload to zero and marking its row `parked`.
//! The next call to it asks [`ParkedAgentWake`] first, which scales the workload back to one
//! replica and waits until the agent answers, so the call is served by the agent's own workload
//! (its env, ports, mounts and every platform injection), not by a stand-in.
//!
//! Every forward asks, so the common case is one indexed read: anything but a parked agent goes
//! straight through, with no lock and no runtime call. A parked one is woken once per process:
//! concurrent callers of the same agent wait behind the first, holding no wake slot and no pooled
//! connection, and find the row `running` when it is done. The wake itself runs in one transaction
//! holding the agent's advisory lock ([`lock_agent`]), so wakes on other replicas wait for it too,
//! and the lock is released at commit or rollback, including when a caller drops the future. A
//! wake holds a pooled connection while it waits, so only a few run at once (`wake_slots`).
//!
//! A row parked before parking scaled workloads to zero has no workload at all (that parking
//! destroyed it): `scale` answers `ContainerNotFound`, and the wake deploys the workload from the
//! agent's catalog row ([`WorkloadDeployer`]), the spec a redeploy gets, then waits for it as for
//! any other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nasiko_flow::{FlowEvent, FlowEventBus};
use nasiko_runtime::{ContainerId, ContainerRuntime, RuntimeError};
use sqlx::{PgConnection, PgPool};
use tokio::sync::{Semaphore, SemaphorePermit};
use tracing::{info, warn};
use uuid::Uuid;

use super::{AgentWake, lock_agent};
use crate::agents::deployments::record_newest_deployment_status;
use crate::agents::utils::answers_http;

/// How often a wake asks a woken agent whether it answers yet.
const ANSWER_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The most wakes that run at once in one process, each holding one pooled connection for up to
/// the wake budget. Fewer on a small pool: see [`wake_slots`].
const MAX_CONCURRENT_WAKES: usize = 8;

/// What a client is told when a wake fails; the cause stays in the logs.
const COULD_NOT_START: &str = "the agent could not be started, try again shortly";
const NOT_STARTED_IN_TIME: &str = "the agent did not start in time, try again shortly";

/// Deploys a parked agent's workload when it has none, from its catalog row.
#[async_trait]
pub trait WorkloadDeployer: Send + Sync {
    /// Returns once the runtime has accepted the deploy, not once the agent answers. An `Err`
    /// carries the message the caller is refused with; the cause is logged where it happened.
    async fn deploy(&self, agent_id: Uuid) -> Result<(), String>;
}

/// What a wake works with, all received from the composition root.
pub struct WakeDeps {
    pub db: PgPool,
    pub runtime: Arc<dyn ContainerRuntime>,
    /// For asking a woken agent whether it answers; each probe sets its own short timeout.
    pub http: reqwest::Client,
    pub flow_events: FlowEventBus,
    /// For a parked agent that has no workload to scale up.
    pub deployer: Arc<dyn WorkloadDeployer>,
}

pub struct ParkedAgentWake {
    deps: WakeDeps,
    /// How long a call waits for a parked agent to answer, every queue and lock included.
    budget: Duration,
    wake_slots: Semaphore,
    /// The in-flight wake of each agent in this process. Weak, so an entry lives only as long as
    /// a caller holds its gate.
    gates: Mutex<HashMap<Uuid, Weak<tokio::sync::Mutex<()>>>>,
}

/// What scaling a parked agent's workload up found.
#[derive(Debug, PartialEq, Eq)]
enum ScaleUp {
    /// The runtime is bringing the workload back.
    Started,
    /// There is no workload to scale: the row was parked before parking scaled workloads.
    NoWorkload,
}

impl ParkedAgentWake {
    pub fn new(deps: WakeDeps, budget: Duration) -> Self {
        let slots = wake_slots(deps.db.options().get_max_connections());
        Self {
            deps,
            budget,
            wake_slots: Semaphore::new(slots),
            gates: Mutex::default(),
        }
    }

    /// The one read every forward pays. An unknown or deleted agent is not parked: the resolver
    /// answers for it.
    async fn is_parked(&self, agent_id: Uuid) -> Result<bool, String> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT status, workload_kind FROM agents WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_optional(&self.deps.db)
        .await
        .map_err(|e| unavailable(agent_id, "status read", e))?;
        Ok(row.is_some_and(|(status, kind)| is_wakeable(&status, &kind)))
    }

    /// The agent's gate: the same one for every caller while any of them holds it.
    fn gate_of(&self, agent_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().expect("wake gates lock poisoned");
        if let Some(gate) = gates.get(&agent_id).and_then(Weak::upgrade) {
            return gate;
        }
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(agent_id, Arc::downgrade(&gate));
        gate
    }

    /// Wakes the agent, or waits for the wake another caller in this process already started and
    /// then sees what it left.
    async fn wake_once(&self, agent_id: Uuid, flow_id: Option<&str>) -> Result<(), String> {
        let deadline = Instant::now() + self.budget;
        let gate = self.gate_of(agent_id);
        let _turn = match gate.try_lock() {
            Ok(turn) => turn,
            Err(_) => {
                let turn = tokio::time::timeout_at(deadline.into(), gate.lock())
                    .await
                    .map_err(|_| not_in_time(agent_id))?;
                if !self.is_parked(agent_id).await? {
                    return Ok(());
                }
                turn
            }
        };
        self.wake(agent_id, flow_id, deadline).await
    }

    /// Wakes the agent under its lock. On any failure the transaction rolls back, the row stays
    /// `parked`, the workload is scaled back to zero ([`ParkAgainUnlessWoken`]) so nothing runs
    /// under a parked row, and the next call tries again.
    async fn wake(
        &self,
        agent_id: Uuid,
        flow_id: Option<&str>,
        deadline: Instant,
    ) -> Result<(), String> {
        let started = Instant::now();
        let _slot = self.wake_slot(agent_id, deadline).await?;
        let mut tx = self
            .deps
            .db
            .begin()
            .await
            .map_err(|e| unavailable(agent_id, "begin", e))?;
        lock_agent(&mut tx, agent_id, deadline)
            .await
            .map_err(|e| unavailable(agent_id, "lock", e))?;
        if !still_parked(&mut tx, agent_id)
            .await
            .map_err(|e| unavailable(agent_id, "status re-read", e))?
        {
            // Woken by a caller on another replica that held the lock first.
            return Ok(());
        }
        let container_id = ContainerId::from_uuid(agent_id);
        // Armed before scaling: a scale that times out may still have reached the runtime.
        let undo = ParkAgainUnlessWoken {
            db: self.deps.db.clone(),
            runtime: self.deps.runtime.clone(),
            agent_id,
            armed: true,
        };
        match self.scale_up(&container_id, deadline).await {
            Ok(ScaleUp::Started) => {
                info!(%agent_id, "agent wake: scaled a parked agent up; waiting until it answers");
            }
            Ok(ScaleUp::NoWorkload) => self.deploy_workload(agent_id, deadline).await?,
            Err(e) => return Err(unavailable(agent_id, "scale", e)),
        }
        announce_waking(&self.deps.flow_events, agent_id, flow_id).await;
        let Some(address) = self.wait_answering(&container_id, deadline).await else {
            return Err(not_in_time(agent_id));
        };
        mark_running(&mut tx, agent_id, &address)
            .await
            .map_err(|e| unavailable(agent_id, "mark running", e))?;
        tx.commit()
            .await
            .map_err(|e| unavailable(agent_id, "commit", e))?;
        undo.disarm();
        let wake_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        info!(%agent_id, wake_ms, "agent wake: agent answers");
        Ok(())
    }

    async fn wake_slot(
        &self,
        agent_id: Uuid,
        deadline: Instant,
    ) -> Result<SemaphorePermit<'_>, String> {
        match tokio::time::timeout_at(deadline.into(), self.wake_slots.acquire()).await {
            Ok(Ok(slot)) => Ok(slot),
            Ok(Err(e)) => Err(unavailable(agent_id, "queue", e)),
            Err(_) => Err(unavailable(agent_id, "queue", "no wake slot in time")),
        }
    }

    /// Deploys the workload a parked agent does not have, within the wake's budget. A deploy that
    /// outlasts the budget still finishes, and the next call scales up what it left.
    async fn deploy_workload(&self, agent_id: Uuid, deadline: Instant) -> Result<(), String> {
        info!(%agent_id, "agent wake: a parked agent has no workload; deploying it");
        let deployed = self.deps.deployer.deploy(agent_id);
        match tokio::time::timeout_at(deadline.into(), deployed).await {
            Ok(outcome) => outcome,
            Err(_) => Err(not_in_time(agent_id)),
        }
    }

    async fn scale_up(
        &self,
        container_id: &ContainerId,
        deadline: Instant,
    ) -> Result<ScaleUp, RuntimeError> {
        let scaled = self.deps.runtime.scale(container_id, 1);
        let scaled = tokio::time::timeout_at(deadline.into(), scaled)
            .await
            .map_err(|_| RuntimeError::Timeout("scale to one replica".to_owned()))?;
        match scaled {
            Ok(()) => Ok(ScaleUp::Started),
            Err(RuntimeError::ContainerNotFound(_)) => Ok(ScaleUp::NoWorkload),
            Err(e) => Err(e),
        }
    }

    /// Waits until the agent answers at its own address and returns that address; `None` when
    /// the deadline passes first. The address is the runtime's `endpoint()`, never the forwarding
    /// address, which can be a proxy in front of the agent that answers whether or not the agent
    /// is up. `wait_routable` is event-driven where the runtime can be (on Kubernetes, a Ready
    /// pod), and the HTTP probe covers a runtime that has an address before the agent listens
    /// (Docker maps the port as soon as the container starts).
    async fn wait_answering(
        &self,
        container_id: &ContainerId,
        deadline: Instant,
    ) -> Option<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let routable = self.deps.runtime.wait_routable(container_id, remaining);
        let address = match tokio::time::timeout_at(deadline.into(), routable).await {
            Ok(Ok(address)) => address,
            Ok(Err(e)) => {
                warn!(%container_id, %e, "agent wake: the woken agent has no address");
                return None;
            }
            Err(_) => return None,
        };
        loop {
            let answered = answers_http(&self.deps.http, &address);
            if matches!(
                tokio::time::timeout_at(deadline.into(), answered).await,
                Ok(true)
            ) {
                return Some(address);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            tokio::time::sleep(remaining.min(ANSWER_POLL_INTERVAL)).await;
        }
    }
}

/// How long undoing a failed wake waits for the agent's lock.
const PARK_AGAIN_LOCK_WAIT: Duration = Duration::from_secs(60);

/// Scales a wake's workload back to zero unless the wake committed. A failed wake rolls its row
/// back to `parked`, and the idle reaper parks only `running` rows, so a workload it left up
/// would keep running, and billing, until the next call. Runs on drop, so a caller that went
/// away mid-wake is covered too.
struct ParkAgainUnlessWoken {
    db: PgPool,
    runtime: Arc<dyn ContainerRuntime>,
    agent_id: Uuid,
    armed: bool,
}

impl ParkAgainUnlessWoken {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ParkAgainUnlessWoken {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!(agent_id = %self.agent_id, "agent wake: no runtime to scale a failed wake back down on");
            return;
        };
        let (db, runtime, agent_id) = (self.db.clone(), self.runtime.clone(), self.agent_id);
        handle.spawn(async move {
            if let Err(e) = park_again(&db, runtime.as_ref(), agent_id).await {
                warn!(%agent_id, %e, "agent wake: could not scale a failed wake back down; the next call retries the wake");
            }
        });
    }
}

/// Scales a parked agent's workload to zero, under the agent's lock so it cannot undo a wake
/// that a later caller has since completed: once the lock is held, a row that is no longer
/// `parked` belongs to that wake (or to the owner) and is left alone.
async fn park_again(
    db: &PgPool,
    runtime: &dyn ContainerRuntime,
    agent_id: Uuid,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    lock_agent(&mut tx, agent_id, Instant::now() + PARK_AGAIN_LOCK_WAIT).await?;
    if !still_parked(&mut tx, agent_id).await? {
        return Ok(());
    }
    match runtime.scale(&ContainerId::from_uuid(agent_id), 0).await {
        Ok(()) | Err(RuntimeError::ContainerNotFound(_)) => {}
        Err(e) => return Err(e.into()),
    }
    info!(%agent_id, "agent wake: scaled a failed wake's workload back to zero");
    tx.commit().await?;
    Ok(())
}

#[async_trait]
impl AgentWake for ParkedAgentWake {
    async fn before_forward(&self, agent_id: Uuid, flow_id: Option<&str>) -> Result<(), String> {
        if self.is_parked(agent_id).await? {
            return self.wake_once(agent_id, flow_id).await;
        }
        Ok(())
    }
}

/// Only a framework agent at rest is woken here. A harness agent's sessions have sandboxes of
/// their own, and every other status is the resolver's to answer.
fn is_wakeable(status: &str, workload_kind: &str) -> bool {
    status == "parked" && workload_kind != "harness"
}

/// A quarter of the pool, so a burst of wakes cannot starve the requests that wake nothing, and
/// at most [`MAX_CONCURRENT_WAKES`]; never none. Callers beyond it queue holding no connection.
fn wake_slots(pool_max_connections: u32) -> usize {
    usize::try_from(pool_max_connections / 4)
        .unwrap_or(usize::MAX)
        .clamp(1, MAX_CONCURRENT_WAKES)
}

async fn still_parked(tx: &mut PgConnection, agent_id: Uuid) -> sqlx::Result<bool> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(&mut *tx)
            .await?;
    Ok(status.as_deref() == Some("parked"))
}

/// Undoes the park: the agent serves again at `address` (the park cleared it, and a row with an
/// address is one a workload serves), its idle clock starts from this call, and its newest
/// deployment is live again (the park stopped it). Guarded so a status that moved on during the
/// wake (the owner stopped the agent) is not overwritten.
async fn mark_running(tx: &mut PgConnection, agent_id: Uuid, address: &str) -> sqlx::Result<()> {
    let marked = sqlx::query(
        "UPDATE agents SET status = 'running', url = $2, last_request_at = now(), \
                updated_at = now() \
         WHERE id = $1 AND status = 'parked'",
    )
    .bind(agent_id)
    .bind(address)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if marked == 0 {
        warn!(%agent_id, "agent wake: the agent's status changed during the wake; left as it is");
        return Ok(());
    }
    record_newest_deployment_status(&mut *tx, agent_id, "running").await
}

/// Tells the flow's event stream that its call waits for a wake, so a client can show it.
async fn announce_waking(bus: &FlowEventBus, agent_id: Uuid, flow_id: Option<&str>) {
    let Some(flow_id) = flow_id else {
        return;
    };
    let event = FlowEvent::AgentWaking {
        agent_id: agent_id.to_string(),
        wake_ms: None,
    };
    bus.publish(flow_id, event).await;
}

/// Logs the cause and returns the message the caller sees, which carries no internal detail.
fn unavailable(agent_id: Uuid, step: &str, cause: impl std::fmt::Display) -> String {
    warn!(%agent_id, step, %cause, "agent wake failed");
    COULD_NOT_START.to_owned()
}

fn not_in_time(agent_id: Uuid) -> String {
    warn!(%agent_id, "agent wake: the agent did not answer within the wake budget");
    NOT_STARTED_IN_TIME.to_owned()
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use nasiko_runtime::{DeploymentSpec, DeploymentStatus, RuntimeState};

    use super::*;

    /// A runtime whose one workload scales as told and is routable at `address` (or never).
    #[derive(Default)]
    struct ScriptedRuntime {
        /// What `scale` answers with; `None` scales.
        scale_error: Mutex<Option<RuntimeError>>,
        scaled_to: Mutex<Vec<u32>>,
        /// What `wait_routable` answers once routable; `None` never becomes routable.
        address: Option<String>,
    }

    impl ScriptedRuntime {
        fn routable_at(address: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                address: Some(address.into()),
                ..Self::default()
            })
        }

        fn never_routable() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn failing_scale(error: RuntimeError) -> Arc<Self> {
            let runtime = Self::default();
            *runtime.scale_error.lock().unwrap() = Some(error);
            Arc::new(runtime)
        }

        fn scaled_to(&self) -> Vec<u32> {
            self.scaled_to.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ContainerRuntime for ScriptedRuntime {
        async fn deploy(&self, _spec: &DeploymentSpec) -> nasiko_runtime::Result<DeploymentStatus> {
            unimplemented!("a wake never deploys")
        }

        async fn destroy(&self, _id: &ContainerId) -> nasiko_runtime::Result<()> {
            unimplemented!("a wake never destroys")
        }

        async fn scale(&self, _id: &ContainerId, replicas: u32) -> nasiko_runtime::Result<()> {
            self.scaled_to.lock().unwrap().push(replicas);
            match self.scale_error.lock().unwrap().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        async fn restart(&self, _id: &ContainerId) -> nasiko_runtime::Result<()> {
            unimplemented!("a wake never restarts")
        }

        async fn status(&self, id: &ContainerId) -> nasiko_runtime::Result<DeploymentStatus> {
            Ok(DeploymentStatus {
                container_id: id.clone(),
                state: RuntimeState::Running,
                replicas_live: 1,
                endpoint: self.address.clone(),
                message: None,
                restart_count: 0,
            })
        }

        async fn list(&self) -> nasiko_runtime::Result<Vec<DeploymentStatus>> {
            Ok(Vec::new())
        }

        async fn endpoint(&self, id: &ContainerId) -> nasiko_runtime::Result<String> {
            self.address
                .clone()
                .ok_or_else(|| RuntimeError::ContainerNotFound(id.clone()))
        }

        async fn wait_routable(
            &self,
            id: &ContainerId,
            timeout: Duration,
        ) -> nasiko_runtime::Result<String> {
            if let Some(address) = &self.address {
                return Ok(address.clone());
            }
            tokio::time::sleep(timeout).await;
            Err(RuntimeError::Timeout(format!("{id} to become routable")))
        }

        async fn logs(&self, _id: &ContainerId, _tail: u32) -> nasiko_runtime::Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn build(
            &self,
            _tar: &[u8],
            tag: &str,
        ) -> nasiko_runtime::Result<nasiko_runtime::BuildReport> {
            Ok(nasiko_runtime::BuildReport::image_only(tag))
        }
    }

    /// A deployer that records which agents it was asked to deploy, taking `delay` to answer.
    #[derive(Default)]
    struct RecordingDeployer {
        deployed: Mutex<Vec<Uuid>>,
        delay: Duration,
    }

    #[async_trait]
    impl WorkloadDeployer for RecordingDeployer {
        async fn deploy(&self, agent_id: Uuid) -> Result<(), String> {
            tokio::time::sleep(self.delay).await;
            self.deployed.lock().unwrap().push(agent_id);
            Ok(())
        }
    }

    /// A wake over `runtime`; its pool is never connected, so these tests stay off the database.
    fn wake_over(runtime: Arc<ScriptedRuntime>) -> ParkedAgentWake {
        wake_with(runtime, Arc::new(RecordingDeployer::default()))
    }

    fn wake_with(
        runtime: Arc<ScriptedRuntime>,
        deployer: Arc<RecordingDeployer>,
    ) -> ParkedAgentWake {
        let db = PgPool::connect_lazy("postgres://hermetic.invalid/none").expect("lazy pool");
        let deps = WakeDeps {
            db,
            runtime,
            http: reqwest::Client::new(),
            flow_events: FlowEventBus::new(),
            deployer,
        };
        ParkedAgentWake::new(deps, Duration::from_secs(5))
    }

    /// A server on loopback that answers every request with a 404: up, if not serving a card.
    async fn start_answering_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, Router::new()).await.unwrap();
        });
        address
    }

    fn container() -> ContainerId {
        ContainerId::from_uuid(Uuid::new_v4())
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    #[test]
    fn only_a_parked_framework_agent_is_wakeable() {
        assert!(is_wakeable("parked", "agent"));
        assert!(!is_wakeable("parked", "harness"));
        for status in [
            "running",
            "stopped",
            "failed",
            "deploying",
            "registered",
            "crashed",
        ] {
            assert!(!is_wakeable(status, "agent"), "{status}");
        }
    }

    #[test]
    fn wakes_take_a_quarter_of_the_pool_at_most_eight_and_never_none() {
        assert_eq!(wake_slots(50), 8);
        assert_eq!(wake_slots(500), 8);
        assert_eq!(wake_slots(16), 4);
        assert_eq!(wake_slots(5), 1);
        assert_eq!(wake_slots(1), 1);
    }

    #[tokio::test]
    async fn concurrent_callers_of_one_agent_share_one_gate() {
        let wake = wake_over(ScriptedRuntime::never_routable());
        let agent = Uuid::new_v4();

        let first = wake.gate_of(agent);
        let second = wake.gate_of(agent);
        let other_agent = wake.gate_of(Uuid::new_v4());

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &other_agent));
    }

    #[tokio::test]
    async fn a_gate_nobody_holds_is_not_kept() {
        let wake = wake_over(ScriptedRuntime::never_routable());
        let agent = Uuid::new_v4();
        drop(wake.gate_of(agent));

        let _next = wake.gate_of(Uuid::new_v4());

        let gates = wake.gates.lock().unwrap();
        assert!(!gates.contains_key(&agent), "pruned once released");
        assert_eq!(gates.len(), 1);
    }

    #[tokio::test]
    async fn scaling_up_asks_the_runtime_for_one_replica() {
        let runtime = ScriptedRuntime::routable_at("http://127.0.0.1:1");
        let wake = wake_over(runtime.clone());

        let scaled = wake.scale_up(&container(), soon()).await.unwrap();

        assert_eq!(scaled, ScaleUp::Started);
        assert_eq!(runtime.scaled_to(), vec![1]);
    }

    /// A row parked before parking scaled workloads has none; the wake deploys it.
    #[tokio::test]
    async fn a_missing_workload_is_no_workload_not_a_failure() {
        let id = container();
        let runtime = ScriptedRuntime::failing_scale(RuntimeError::ContainerNotFound(id.clone()));
        let wake = wake_over(runtime);

        let scaled = wake.scale_up(&id, soon()).await.unwrap();

        assert_eq!(scaled, ScaleUp::NoWorkload);
    }

    #[tokio::test]
    async fn a_missing_workload_is_deployed_through_the_deployer() {
        let deployer = Arc::new(RecordingDeployer::default());
        let wake = wake_with(ScriptedRuntime::never_routable(), deployer.clone());
        let agent_id = Uuid::new_v4();

        wake.deploy_workload(agent_id, soon()).await.unwrap();

        assert_eq!(*deployer.deployed.lock().unwrap(), vec![agent_id]);
    }

    #[tokio::test]
    async fn a_deploy_that_outlasts_the_budget_ends_the_wait_at_the_deadline() {
        let deployer = Arc::new(RecordingDeployer {
            delay: Duration::from_secs(5),
            ..RecordingDeployer::default()
        });
        let wake = wake_with(ScriptedRuntime::never_routable(), deployer);
        let started = Instant::now();

        let outcome = wake
            .deploy_workload(Uuid::new_v4(), started + Duration::from_millis(50))
            .await;

        assert_eq!(outcome, Err(NOT_STARTED_IN_TIME.to_owned()));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn any_other_scale_failure_is_an_error() {
        let runtime =
            ScriptedRuntime::failing_scale(RuntimeError::BackendUnreachable("api down".into()));
        let wake = wake_over(runtime);

        let scaled = wake.scale_up(&container(), soon()).await;

        assert!(matches!(scaled, Err(RuntimeError::BackendUnreachable(_))));
    }

    /// Any HTTP answer, even an error status, means the agent's server is listening.
    #[tokio::test]
    async fn an_agent_that_answers_http_is_reached_at_its_own_address() {
        let address = start_answering_server().await;
        let wake = wake_over(ScriptedRuntime::routable_at(address.clone()));

        let answered = wake.wait_answering(&container(), soon()).await;

        assert_eq!(answered, Some(address));
    }

    /// Docker's mapped port exists before the agent listens: an address alone is not enough.
    #[tokio::test]
    async fn an_address_nothing_answers_on_ends_the_wait_at_the_deadline() {
        let wake = wake_over(ScriptedRuntime::routable_at("http://127.0.0.1:1"));
        let started = Instant::now();

        let answered = wake
            .wait_answering(&container(), started + Duration::from_millis(300))
            .await;

        assert_eq!(answered, None);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_agent_that_never_gets_an_address_ends_the_wait_at_the_deadline() {
        let wake = wake_over(ScriptedRuntime::never_routable());
        let started = Instant::now();

        let answered = wake
            .wait_answering(&container(), started + Duration::from_millis(50))
            .await;

        assert_eq!(answered, None);
        assert!(
            started.elapsed() < ANSWER_POLL_INTERVAL,
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_wake_is_announced_on_the_callers_flow_as_it_begins() {
        let bus = FlowEventBus::new();
        let mut events = bus.subscribe("flow-1").await;
        let agent_id = Uuid::new_v4();

        announce_waking(&bus, agent_id, Some("flow-1")).await;

        match events.try_recv().expect("an event") {
            FlowEvent::AgentWaking {
                agent_id: announced,
                wake_ms,
            } => {
                assert_eq!(announced, agent_id.to_string());
                assert_eq!(wake_ms, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_wake_outside_a_flow_is_not_announced() {
        let bus = FlowEventBus::new();
        let mut events = bus.subscribe("flow-1").await;

        announce_waking(&bus, Uuid::new_v4(), None).await;

        assert!(events.try_recv().is_err());
    }
}
