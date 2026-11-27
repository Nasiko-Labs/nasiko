use std::sync::Arc;

use async_trait::async_trait;
use nasiko_config::Config;
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, InstanceInfo,
    Result as RuntimeResult, RuntimeState, WorkspaceEntry, WorkspaceFile, WorkspaceRef,
};
use nasiko_server::state::AppState;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ─── Infra endpoints — overridable via env for CI ────────────────────────────

fn pg_admin_url() -> String {
    std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

fn redis_url() -> String {
    std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into())
}

fn s3_endpoint() -> String {
    std::env::var("TEST_S3_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".into())
}

/// Rate limits are counted in Redis, which every test server shares, so without this a test
/// would start on whatever budget the tests before it spent (30 logins a minute, run-wide).
async fn clear_rate_limits(redis: &nasiko_redis::SharedRedis) {
    let mut connection = redis
        .connection()
        .await
        .expect("connect to redis — is it running? (set TEST_REDIS_URL to override)");
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("nasiko:rl:*")
        .query_async(&mut connection)
        .await
        .expect("list rate-limit keys");
    if !keys.is_empty() {
        let () = redis::cmd("DEL")
            .arg(keys)
            .query_async(&mut connection)
            .await
            .expect("clear rate-limit keys");
    }
}

// ─── FakeRuntime ─────────────────────────────────────────────────────────────

/// No-op ContainerRuntime for integration tests.
///
/// Tests exercise HTTP routes, ACL, and DB state — not actual container
/// deployment. Using FakeRuntime eliminates the Docker daemon dependency from
/// CI and removes ~300ms of Docker ping overhead per test.
///
/// Tracks deployed containers in memory (`deploy`/`destroy` update it, `list`
/// reads it) so tests can exercise `list`'s per-caller filtering without a
/// real runtime.
#[derive(Default)]
pub struct FakeRuntime {
    containers: std::sync::Mutex<std::collections::HashMap<ContainerId, DeploymentStatus>>,
    /// Instances returned by `list_instances` — set via [`set_instances`] so
    /// hours-meter tests can script exactly what the reconciler observes.
    instances: std::sync::Mutex<Vec<InstanceInfo>>,
    /// When set, `deploy` fails instead of succeeding — lets a test exercise
    /// a genuine (post-build) deploy failure without a real runtime.
    fail_deploy: std::sync::atomic::AtomicBool,
    /// The `(code, message)` every `deploy` is refused with, as a layer that declines to run
    /// workloads answers: not a failure of the deploy, so callers keep what they had.
    decline_deploy: std::sync::Mutex<Option<(String, String)>>,
    /// When set, `build` fails instead of succeeding - a failed image build, which must leave
    /// the running agent untouched.
    fail_build: std::sync::atomic::AtomicBool,
    /// Every spec `deploy` accepted, in order - lets a test assert on the env a
    /// deploy path actually handed the runtime.
    deployed: std::sync::Mutex<Vec<DeploymentSpec>>,
    /// When set, `status` reports a container that was never deployed (or was
    /// destroyed) as not found, like a real runtime - what startup reconcile
    /// keys on. Off by default: most tests expect everything to look running.
    status_follows_deploys: std::sync::atomic::AtomicBool,
    /// Stands in for agents' persistent `/workspace`, keyed by
    /// `{subpath}/{rel_path}` so a test can prove the server scoped a read to
    /// the right agent rather than just echoing back what it asked for.
    workspace: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    /// What `supports_autoscaling` answers; off by default, like a runtime that
    /// cannot autoscale.
    autoscaling: std::sync::atomic::AtomicBool,
    /// Every `apply_replica_bounds` call, in order: (container, min, max, target).
    applied_bounds: std::sync::Mutex<Vec<AppliedBounds>>,
    /// Every `deploy` call's replica bounds, in order: (container, min, max).
    deployed_bounds: std::sync::Mutex<Vec<(ContainerId, u32, u32)>>,
    /// When set, `build` waits for it to be notified before returning.
    build_gate: std::sync::Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// How many `build` calls have started.
    builds_started: std::sync::atomic::AtomicUsize,
    /// What `restarts_in_place` answers; off by default, like a runtime whose restart does not
    /// apply a changed env.
    restarts_in_place: std::sync::atomic::AtomicBool,
    /// When set, `status` reports this state instead of `Running`.
    reported_state: std::sync::Mutex<Option<RuntimeState>>,
    /// Every `restart` call, in order.
    restarted: std::sync::Mutex<Vec<ContainerId>>,
    /// Every `refresh_secrets` call, in order: the container and the env it was given.
    refreshed: std::sync::Mutex<Vec<(ContainerId, std::collections::HashMap<String, String>)>>,
    /// When set, the next `destroy` waits this long first, once.
    destroy_delay: std::sync::Mutex<Option<std::time::Duration>>,
}

/// One `apply_replica_bounds` call: (container, min, max, target).
pub type AppliedBounds = (ContainerId, u32, u32, Option<u32>);

impl FakeRuntime {
    /// Replace the instance set the next `list_instances` call reports.
    #[allow(dead_code)]
    pub fn set_instances(&self, instances: Vec<InstanceInfo>) {
        *self.instances.lock().unwrap() = instances;
    }

    /// Put a file in an agent's fake persistent storage.
    #[allow(dead_code)]
    pub fn put_workspace_file(&self, workspace: &WorkspaceRef, rel_path: &str, body: &[u8]) {
        // Key on the *scoped* subpath so a test can seed files under a specific
        // user's `u/<token>` subtree, mirroring the real runtimes.
        self.workspace
            .lock()
            .unwrap()
            .insert(format!("{}/{rel_path}", workspace.subpath()), body.to_vec());
    }

    /// Makes the next `destroy` wait `delay` first, once, so a test can act while it is in flight.
    #[allow(dead_code)]
    pub fn delay_next_destroy(&self, delay: std::time::Duration) {
        *self.destroy_delay.lock().unwrap() = Some(delay);
    }

    /// The specs deployed so far, oldest first.
    #[allow(dead_code)]
    pub fn deployed_specs(&self) -> Vec<DeploymentSpec> {
        self.deployed.lock().unwrap().clone()
    }

    /// The most recent spec deployed for `container_id`, if any.
    #[allow(dead_code)]
    pub fn last_spec_for(&self, container_id: &ContainerId) -> Option<DeploymentSpec> {
        self.deployed
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|spec| &spec.container_id == container_id)
            .cloned()
    }

    /// Make `status` answer from what was actually deployed.
    #[allow(dead_code)]
    pub fn set_status_follows_deploys(&self, follow: bool) {
        self.status_follows_deploys
            .store(follow, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make the next (and all subsequent) `deploy` calls fail instead of
    /// succeeding.
    #[allow(dead_code)]
    pub fn set_fail_deploy(&self, fail: bool) {
        self.fail_deploy
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make every `deploy` answer `RuntimeError::Declined` with `code` and `message` until
    /// cleared with `None`.
    #[allow(dead_code)]
    pub fn set_decline_deploy(&self, declined: Option<(&str, &str)>) {
        *self.decline_deploy.lock().unwrap() =
            declined.map(|(code, message)| (code.to_owned(), message.to_owned()));
    }

    /// Make `build` calls fail instead of succeeding.
    #[allow(dead_code)]
    pub fn set_fail_build(&self, fail: bool) {
        self.fail_build
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make `supports_autoscaling` answer `supported`.
    #[allow(dead_code)]
    pub fn set_supports_autoscaling(&self, supported: bool) {
        self.autoscaling
            .store(supported, std::sync::atomic::Ordering::SeqCst);
    }

    /// The `apply_replica_bounds` calls made so far.
    #[allow(dead_code)]
    pub fn applied_bounds(&self) -> Vec<AppliedBounds> {
        self.applied_bounds.lock().unwrap().clone()
    }

    /// Make `restarts_in_place` answer `in_place`.
    #[allow(dead_code)]
    pub fn set_restarts_in_place(&self, in_place: bool) {
        self.restarts_in_place
            .store(in_place, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make `status` report `state` (the agent is stopped, crashed, ...) instead of `Running`.
    #[allow(dead_code)]
    pub fn set_reported_state(&self, state: RuntimeState) {
        *self.reported_state.lock().unwrap() = Some(state);
    }

    /// The containers `restart` was called on so far.
    #[allow(dead_code)]
    pub fn restarted(&self) -> Vec<ContainerId> {
        self.restarted.lock().unwrap().clone()
    }

    /// The env most recently given to `refresh_secrets` for `container_id`, if any.
    #[allow(dead_code)]
    pub fn last_refreshed_env(
        &self,
        container_id: &ContainerId,
    ) -> Option<std::collections::HashMap<String, String>> {
        self.refreshed
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(id, _)| id == container_id)
            .map(|(_, env)| env.clone())
    }

    /// The (container, min, max) of every `deploy` so far.
    #[allow(dead_code)]
    pub fn deployed_bounds(&self) -> Vec<(ContainerId, u32, u32)> {
        self.deployed_bounds.lock().unwrap().clone()
    }

    /// Makes every later `build` wait until the returned gate is notified
    /// (`notify_one` per build), so a test can act between a job's start and its deploy.
    #[allow(dead_code)]
    pub fn hold_builds(&self) -> Arc<tokio::sync::Notify> {
        let gate = Arc::new(tokio::sync::Notify::new());
        *self.build_gate.lock().unwrap() = Some(gate.clone());
        gate
    }

    /// How many `build` calls have started, held or not.
    #[allow(dead_code)]
    pub fn builds_started(&self) -> usize {
        self.builds_started
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn fake_status(container_id: &ContainerId) -> DeploymentStatus {
    DeploymentStatus {
        container_id: container_id.clone(),
        state: RuntimeState::Running,
        replicas_live: 1,
        endpoint: Some("http://localhost:8000".into()),
        message: None,
        restart_count: 0,
    }
}

#[async_trait]
impl ContainerRuntime for FakeRuntime {
    async fn deploy(&self, spec: &DeploymentSpec) -> RuntimeResult<DeploymentStatus> {
        if self.fail_deploy.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(nasiko_runtime::RuntimeError::BackendUnreachable(
                "fake deploy failure (scripted by test)".into(),
            ));
        }
        if let Some((code, message)) = self.decline_deploy.lock().unwrap().clone() {
            return Err(nasiko_runtime::RuntimeError::Declined { code, message });
        }
        let status = fake_status(&spec.container_id);
        self.deployed.lock().unwrap().push(spec.clone());
        self.deployed_bounds.lock().unwrap().push((
            spec.container_id.clone(),
            spec.min_replicas,
            spec.max_replicas,
        ));
        self.containers
            .lock()
            .unwrap()
            .insert(spec.container_id.clone(), status.clone());
        Ok(status)
    }

    async fn destroy(&self, container_id: &ContainerId) -> RuntimeResult<()> {
        let delay = self.destroy_delay.lock().unwrap().take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.containers.lock().unwrap().remove(container_id);
        Ok(())
    }

    async fn scale(&self, _container_id: &ContainerId, _replicas: u32) -> RuntimeResult<()> {
        Ok(())
    }

    async fn restart(&self, container_id: &ContainerId) -> RuntimeResult<()> {
        self.restarted.lock().unwrap().push(container_id.clone());
        Ok(())
    }

    async fn status(&self, container_id: &ContainerId) -> RuntimeResult<DeploymentStatus> {
        let follows = self
            .status_follows_deploys
            .load(std::sync::atomic::Ordering::SeqCst);
        if follows && !self.containers.lock().unwrap().contains_key(container_id) {
            return Err(nasiko_runtime::RuntimeError::ContainerNotFound(
                container_id.clone(),
            ));
        }
        let mut status = fake_status(container_id);
        if let Some(state) = *self.reported_state.lock().unwrap() {
            status.state = state;
        }
        Ok(status)
    }

    async fn list(&self) -> RuntimeResult<Vec<DeploymentStatus>> {
        Ok(self.containers.lock().unwrap().values().cloned().collect())
    }

    async fn endpoint(&self, container_id: &ContainerId) -> RuntimeResult<String> {
        // Mirror a real runtime: a container that was never deployed can't
        // resolve. The agent proxy prefers this live lookup and only falls
        // back to the stored `agents.url` when it errors, so tests that seed
        // `agents.url` directly (no runtime deploy) rely on this failing.
        if self.containers.lock().unwrap().contains_key(container_id) {
            Ok("http://localhost:8000".into())
        } else {
            Err(nasiko_runtime::RuntimeError::ContainerNotFound(
                container_id.clone(),
            ))
        }
    }

    async fn logs(&self, _container_id: &ContainerId, _tail: u32) -> RuntimeResult<Vec<String>> {
        Ok(vec![])
    }

    async fn build(
        &self,
        _tar_context: &[u8],
        image_tag: &str,
    ) -> RuntimeResult<nasiko_runtime::BuildReport> {
        self.builds_started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = self.build_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        if self.fail_build.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(nasiko_runtime::RuntimeError::BackendUnreachable(
                "fake build failure (scripted by test)".into(),
            ));
        }
        Ok(nasiko_runtime::BuildReport::image_only(image_tag))
    }

    async fn supports_autoscaling(&self) -> bool {
        self.autoscaling.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn restarts_in_place(&self) -> bool {
        self.restarts_in_place
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn refresh_secrets(
        &self,
        container_id: &ContainerId,
        _name: &str,
        env_vars: std::collections::HashMap<String, String>,
    ) -> RuntimeResult<()> {
        self.refreshed
            .lock()
            .unwrap()
            .push((container_id.clone(), env_vars));
        Ok(())
    }

    async fn apply_replica_bounds(
        &self,
        container_id: &ContainerId,
        min: u32,
        max: u32,
        target: Option<u32>,
    ) -> RuntimeResult<()> {
        self.applied_bounds
            .lock()
            .unwrap()
            .push((container_id.clone(), min, max, target));
        Ok(())
    }

    // Explicit impl (not the trait default) so tests fully control what the
    // hours-meter reconciler observes.
    async fn list_instances(&self) -> RuntimeResult<Vec<InstanceInfo>> {
        Ok(self.instances.lock().unwrap().clone())
    }

    // Explicit impls (not the trait defaults, which are "empty" and
    // "unsupported") so a test can prove the server reached the runtime and
    // scoped the read to one agent's subdirectory.
    async fn list_workspace(&self, w: &WorkspaceRef) -> RuntimeResult<Vec<WorkspaceEntry>> {
        let prefix = format!("{}/", w.subpath());
        Ok(self
            .workspace
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(key, body)| {
                Some(WorkspaceEntry {
                    path: key.strip_prefix(&prefix)?.to_owned(),
                    size: body.len() as u64,
                    mtime: 0,
                })
            })
            .collect())
    }

    async fn read_workspace_file(
        &self,
        w: &WorkspaceRef,
        rel_path: &str,
    ) -> RuntimeResult<WorkspaceFile> {
        let key = format!("{}/{rel_path}", w.subpath());
        let body = self
            .workspace
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                nasiko_runtime::RuntimeError::Internal(format!("no such file: {rel_path}"))
            })?;
        Ok(WorkspaceFile {
            size: body.len() as u64,
            stream: Box::pin(futures::stream::once(async move {
                Ok(bytes::Bytes::from(body))
            })),
        })
    }
}

// ─── RowWake ─────────────────────────────────────────────────────────────────

/// Stands in for the platform's wake over [`FakeRuntime`], which models no scaling: a parked
/// agent given an address with [`RowWake::serve_at`] goes back to `running` at that address (the
/// fake runtime has no container for it, so the resolver falls back to the stored `url`), and is
/// recorded as woken. A parked agent with no address is refused, as a wake that could not start
/// it would be. Any other agent goes through untouched.
#[allow(dead_code)]
pub struct RowWake {
    db: PgPool,
    addresses: std::sync::Mutex<std::collections::HashMap<Uuid, String>>,
    woken: std::sync::Mutex<Vec<Uuid>>,
}

#[allow(dead_code)]
impl RowWake {
    /// A wake installed on `server`'s state, for every path that forwards.
    pub fn install(server: &TestServer) -> Arc<Self> {
        let wake = Arc::new(Self {
            db: server.db.clone(),
            addresses: Default::default(),
            woken: Default::default(),
        });
        server.state.agent_wake.install(wake.clone());
        wake
    }

    /// Where `agent` serves once woken.
    pub fn serve_at(&self, agent: Uuid, address: &str) {
        self.addresses
            .lock()
            .unwrap()
            .insert(agent, address.to_owned());
    }

    /// How many times `agent` was woken.
    pub fn woken_count(&self, agent: Uuid) -> usize {
        self.woken
            .lock()
            .unwrap()
            .iter()
            .filter(|id| **id == agent)
            .count()
    }
}

#[async_trait]
impl nasiko_server::agent_wake::AgentWake for RowWake {
    async fn before_forward(&self, agent_id: Uuid, _flow_id: Option<&str>) -> Result<(), String> {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| e.to_string())?;
        if status.as_deref() != Some("parked") {
            return Ok(());
        }
        let address = self.addresses.lock().unwrap().get(&agent_id).cloned();
        let Some(address) = address else {
            return Err("the agent could not be started, try again shortly".to_owned());
        };
        sqlx::query(
            "UPDATE agents SET status = 'running', url = $2, last_request_at = now() \
             WHERE id = $1 AND status = 'parked'",
        )
        .bind(agent_id)
        .bind(address)
        .execute(&self.db)
        .await
        .map_err(|e| e.to_string())?;
        self.woken.lock().unwrap().push(agent_id);
        Ok(())
    }
}

// ─── TestServer ──────────────────────────────────────────────────────────────

/// A running test server bound to a random port, backed by an isolated DB.
/// Call `cleanup().await` at the end of each test to drop the test database.
#[allow(dead_code)]
pub struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    /// Direct pool access for tests that need to seed or verify DB state.
    #[allow(dead_code)]
    pub db: PgPool,
    /// The fake runtime backing the server — lets tests script instance
    /// observations (`set_instances`) and drive the hours meter directly.
    #[allow(dead_code)]
    pub runtime: Arc<FakeRuntime>,
    /// The fake sandbox provider backing harness sessions.
    #[allow(dead_code)]
    pub sandbox: Arc<nasiko_runtime::sandbox_fake::FakeSandboxProvider>,
    /// Clone of the running server's `AppState::flow_events` — lets a test
    /// subscribe to a flow's `FlowEventBus` channel directly and assert on
    /// what actually got published (or didn't), rather than only on the HTTP
    /// response.
    #[allow(dead_code)]
    pub flow_events: nasiko_flow::FlowEventBus,
    /// Clone of the running server's `AppState::genai_metrics`: lets a test read the
    /// in-process per-agent in-flight count while a request is held open.
    #[allow(dead_code)]
    pub genai_metrics: nasiko_server::telemetry::genai::GenAiMetrics,
    /// Clone of the running server's `AppState`, for tests that call a
    /// background routine (seed, reconcile) directly instead of over HTTP.
    #[allow(dead_code)]
    pub state: AppState,
    db_name: String,
    admin_pool: PgPool,
}

/// Single-connection pool options used throughout tests.
///
/// max_connections(1) keeps shared memory usage minimal — Postgres allocates
/// DSM segments per connection for parallel workers, and a per-test admin pool
/// of default size (10) quickly exhausts the system's shared-memory budget
/// when many test DBs are created in sequence.
fn minimal_pool_opts() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .idle_timeout(std::time::Duration::from_secs(1))
        .acquire_timeout(std::time::Duration::from_secs(30))
}

impl TestServer {
    #[allow(dead_code)]
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// Same as [`start`](Self::start), but lets the caller override fields on
    /// the generated `Config` before the server boots — e.g. setting
    /// `build_push_token` for tests exercising the build-service OCI auth
    /// path, which `test_config()` otherwise always leaves empty.
    #[allow(dead_code)]
    pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        let fake_runtime = Arc::new(FakeRuntime::default());
        Self::start_with_runtime_and_handle(
            configure,
            fake_runtime.clone(),
            fake_runtime,
            None,
            None,
            None,
            None,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn start_with_storage(oci_storage: Arc<dyn nasiko_runtime::BlobStore>) -> Self {
        let fake_runtime = Arc::new(FakeRuntime::default());
        Self::start_with_runtime_and_handle(
            |_| {},
            fake_runtime.clone(),
            fake_runtime,
            Some(oci_storage),
            None,
            None,
            None,
        )
        .await
    }

    /// Same as [`start`](Self::start), with `spend_gate` on the app's `AppState`.
    #[allow(dead_code)]
    pub async fn start_with_spend_gate(
        spend_gate: Arc<dyn nasiko_server::spend_gate::SpendGate>,
    ) -> Self {
        let fake_runtime = Arc::new(FakeRuntime::default());
        Self::start_with_runtime_and_handle(
            |_| {},
            fake_runtime.clone(),
            fake_runtime,
            None,
            None,
            Some(spend_gate),
            None,
        )
        .await
    }

    /// Same as [`start`](Self::start), with `agent_wake` installed on the app's `AppState`.
    #[allow(dead_code)]
    pub async fn start_with_agent_wake(
        agent_wake: Arc<dyn nasiko_server::agent_wake::AgentWake>,
    ) -> Self {
        let fake_runtime = Arc::new(FakeRuntime::default());
        Self::start_with_runtime_and_handle(
            |_| {},
            fake_runtime.clone(),
            fake_runtime,
            None,
            Some(agent_wake),
            None,
            None,
        )
        .await
    }

    /// Same as [`start`](Self::start), with `entitlements` installed on the
    /// server's MCP state - how a test plays an edition that restricts users'
    /// tools (the OSS default is unrestricted).
    #[allow(dead_code)]
    pub async fn start_with_entitlements(
        entitlements: Arc<dyn nasiko_mcp_gateway::ToolEntitlements>,
    ) -> Self {
        let fake_runtime = Arc::new(FakeRuntime::default());
        Self::start_with_runtime_and_handle(
            |_| {},
            fake_runtime.clone(),
            fake_runtime,
            None,
            None,
            None,
            Some(entitlements),
        )
        .await
    }

    /// Same as [`start_with`](Self::start_with), but lets the caller supply a
    /// real `ContainerRuntime` (e.g. a real `DockerRuntime`) instead of
    /// `FakeRuntime` — needed by tests that must observe a real container's
    /// lifecycle through the actual HTTP route (not by calling orchestration
    /// functions directly), such as confirming `DELETE
    /// /api/mcp/connectors/{id}` really destroys an uploaded connector's
    /// container. `TestServer.runtime` is a separate, unrelated `FakeRuntime`
    /// in this case — fine, since callers of this constructor never use it.
    #[allow(dead_code)]
    pub async fn start_with_runtime(
        configure: impl FnOnce(&mut Config),
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
        Self::start_with_runtime_and_handle(
            configure,
            runtime,
            Arc::new(FakeRuntime::default()),
            None,
            None,
            None,
            None,
        )
        .await
    }

    /// Like [`start_with_runtime`](Self::start_with_runtime), but binds `0.0.0.0`
    /// and publishes `LLM_GATEWAY_BASE_URL=http://host.docker.internal:<port>`
    /// into the environment **before** the app reads its config at boot — so a
    /// deployed agent *container* can route its model calls back to this server.
    /// A container cannot reach the host's loopback, which is why the default
    /// `127.0.0.1` bind of the other constructors is unusable here.
    ///
    /// Mutates process env, so callers must be `#[serial]`. Used by the
    /// coding-agent live E2E; `TestServer.runtime` is an unrelated `FakeRuntime`.
    #[allow(dead_code)]
    pub async fn start_reachable_with_runtime(
        configure: impl FnOnce(&mut Config),
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Self {
        let pg_admin = pg_admin_url();
        let db_name = format!("nasiko_test_{}", Uuid::new_v4().simple());
        let admin = minimal_pool_opts()
            .connect(&pg_admin)
            .await
            .expect("connect to postgres — is the DB available? (set TEST_PG_URL to override)");
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create test database");
        let db_url = {
            let base = pg_admin.rsplitn(2, '/').last().unwrap_or(&pg_admin);
            format!("{base}/{db_name}")
        };
        let db = minimal_pool_opts()
            .connect(&db_url)
            .await
            .expect("connect to test db");
        AppState::run_migrations(&db).await;

        // Bind first so the port is known, then publish it as the gateway base URL
        // the app's router config reads at boot (and the coding-agent per-turn
        // injector reads per request).
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // SAFETY: callers of this constructor are #[serial].
        unsafe {
            std::env::set_var(
                "LLM_GATEWAY_BASE_URL",
                format!("http://host.docker.internal:{port}"),
            );
        }

        let s3_ep = s3_endpoint();
        let mut config = test_config(db_url, redis_url(), s3_ep.clone());
        configure(&mut config);

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let auth: Arc<dyn nasiko_auth::AuthService> =
            Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));
        let oci_storage: Arc<dyn nasiko_runtime::BlobStore> = Arc::new(
            nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await,
        );
        let sandbox = Arc::new(nasiko_runtime::sandbox_fake::FakeSandboxProvider::default());
        let agent_scaling = Arc::new(nasiko_server::agent_scaling::SingleReplica::new(
            runtime.clone(),
            db.clone(),
        ));
        let state = AppState::from_config_with_sandbox(
            config,
            auth,
            runtime,
            oci_storage,
            db.clone(),
            agent_scaling,
            sandbox.clone(),
        )
        .await;
        clear_rate_limits(&state.redis).await;
        let flow_events = state.flow_events.clone();
        let genai_metrics = state.genai_metrics.clone();
        let handle_state = state.clone();
        let app = nasiko_server::build_app(state, fallback);

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        TestServer {
            // The test's own client runs on the host, so loopback works for it.
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            db: db.clone(),
            runtime: Arc::new(FakeRuntime::default()),
            sandbox,
            flow_events,
            genai_metrics,
            state: handle_state,
            db_name,
            admin_pool: admin,
        }
    }

    /// Shared implementation: `runtime` backs `AppState` (may be fake or
    /// real); `fake_handle` is stored on `TestServer` so hours-meter tests can
    /// script instance observations (`set_instances`) — the two must be the
    /// *same* `Arc` when the caller wants that capability (only [`start_with`]
    /// guarantees this).
    async fn start_with_runtime_and_handle(
        configure: impl FnOnce(&mut Config),
        runtime: Arc<dyn ContainerRuntime>,
        fake_handle: Arc<FakeRuntime>,
        oci_storage: Option<Arc<dyn nasiko_runtime::BlobStore>>,
        agent_wake: Option<Arc<dyn nasiko_server::agent_wake::AgentWake>>,
        spend_gate: Option<Arc<dyn nasiko_server::spend_gate::SpendGate>>,
        entitlements: Option<Arc<dyn nasiko_mcp_gateway::ToolEntitlements>>,
    ) -> Self {
        let pg_admin = pg_admin_url();
        let db_name = format!("nasiko_test_{}", Uuid::new_v4().simple());

        let admin = minimal_pool_opts()
            .connect(&pg_admin)
            .await
            .expect("connect to postgres — is the DB available? (set TEST_PG_URL to override)");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create test database");

        // Build db_url from pg_admin_url by replacing the database name.
        let db_url = {
            let base = pg_admin.rsplitn(2, '/').last().unwrap_or(&pg_admin);
            format!("{base}/{db_name}")
        };

        let db = minimal_pool_opts()
            .connect(&db_url)
            .await
            .expect("connect to test db");

        // The fresh per-test DB starts empty; apply migrations so auth
        // (auth_tokens revocation lookup), routing, and agent tables exist.
        // Without this every request fails closed at `require_auth` with 401
        // (or, for routes that don't hit auth first, a generic query error).
        AppState::run_migrations(&db).await;

        let s3_ep = s3_endpoint();
        let mut config = test_config(db_url, redis_url(), s3_ep.clone());
        configure(&mut config);

        let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
        let auth: Arc<dyn nasiko_auth::AuthService> =
            Arc::new(nasiko_auth::AuthServiceImpl::new(db.clone(), jwt_secret));

        // Integration tests run against the S3-compatible store `just infra`
        // brings up, which is also the only backend this edition ships.
        let oci_storage: Arc<dyn nasiko_runtime::BlobStore> = match oci_storage {
            Some(storage) => storage,
            None => Arc::new(
                nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await,
            ),
        };
        let sandbox = Arc::new(nasiko_runtime::sandbox_fake::FakeSandboxProvider::default());
        let agent_scaling = Arc::new(nasiko_server::agent_scaling::SingleReplica::new(
            runtime.clone(),
            db.clone(),
        ));
        let mut state = AppState::from_config_with_sandbox(
            config,
            auth,
            runtime,
            oci_storage,
            db.clone(),
            agent_scaling,
            sandbox.clone(),
        )
        .await;
        clear_rate_limits(&state.redis).await;
        if let Some(spend_gate) = spend_gate {
            state.spend_gate = spend_gate;
        }
        // The platform's default wake starts a parked row's own workload through the runtime, and
        // `FakeRuntime` models no scaling, so it would wait out its budget for an address a seeded
        // parked row never gets. Tests that exercise a wake install one (`RowWake`, or the default
        // over a runtime that models scaling, as `tests/parked_agent_wake.rs` does).
        state.agent_wake.install(
            agent_wake.unwrap_or_else(|| Arc::new(nasiko_server::agent_wake::NoAgentWake)),
        );
        if let Some(entitlements) = entitlements {
            state.mcp.entitlements = entitlements;
        }
        let flow_events = state.flow_events.clone();
        let genai_metrics = state.genai_metrics.clone();
        let handle_state = state.clone();

        let app = nasiko_server::build_app(state, fallback);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            // As the server binary does, so per-address limits see the peer.
            let service = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
            axum::serve(listener, service).await.unwrap();
        });

        TestServer {
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            db: db.clone(),
            runtime: fake_handle,
            sandbox,
            flow_events,
            genai_metrics,
            state: handle_state,
            db_name,
            admin_pool: admin,
        }
    }

    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        // Terminate connections to the test DB before dropping it.
        sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&self.admin_pool)
        .await
        .ok();

        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&self.admin_pool)
            .await
            .ok();

        // Close both pools so Postgres can release their DSM segments immediately.
        self.db.close().await;
        self.admin_pool.close().await;
    }

    #[allow(dead_code)]
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

fn test_config(db_url: String, redis_url: String, s3_endpoint: String) -> Config {
    // SAFETY: tests run serially via #[serial], so no concurrent env mutation.
    unsafe {
        std::env::set_var("JWT_SECRET", "test-secret-for-nasiko-tests");
        std::env::set_var("S3_ENDPOINT", &s3_endpoint);
        std::env::set_var("S3_ACCESS_KEY", "nasiko");
        std::env::set_var("S3_SECRET_KEY", "nasiko123");
        std::env::set_var("S3_REGION", "us-east-1");
        // 32 bytes of 0x41 ('A'), base64-encoded — required by SecretsCrypto::load_master_key()
        std::env::set_var(
            "SECRETS_ENCRYPTION_KEY",
            "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=",
        );
    }

    Config {
        bind: "127.0.0.1:0".into(),
        shutdown_drain_delay_secs: 0,
        shutdown_grace_secs: 0,
        domain: None,
        delegation_audience: "nasiko-cp-test".to_string(),
        delegation_enabled: true,
        database_url: db_url,
        redis_url,
        agent_runtime: "local".into(),
        k8s_namespace: "nasiko-test".into(),
        kubeconfig: None,
        storage_provider: "s3".into(),
        s3_endpoint,
        s3_bucket: "nasiko-test".into(),
        s3_access_key: "nasiko".into(),
        s3_secret_key: "nasiko123".into(),
        s3_region: "us-east-1".into(),
        assessment_intake_url: None,
        secrets_encryption_key: "12345678901234567890123456789012".into(),
        oci_storage_bucket: "nasiko-test-artifacts".into(),
        agent_image_registry: String::new(),
        agent_registry_username: None,
        agent_registry_password: None,
        agent_registry_host: None,
        seed_image_platform: "linux/amd64".into(),
        agent_image_registry_placeholder: None,
        agent_image_registry_aliases: Vec::new(),
        build_push_token: String::new(),
        agent_jwt_secret: String::new(),
        agent_jwt_algorithm: "HS256".into(),
        injection_proxy_token: String::new(),
        seed_agents: None,
        seed_agent_settings: Default::default(),
        seed_workflows: String::new(),
        tiering_enabled: false,
        tiering_interval_secs: 86400,
        cold_path: String::new(),
        agent_platform_key_fallback: true,
        openai_api_key: None,
        openai_base_url: None,
        openai_model: "gpt-4o".into(),
        decomposer_api_url: None,
        decomposer_api_key: None,
        router_model: "gpt-4o".into(),
        capability_generator_model: "gpt-4o".into(),
        mcp_description_model: "gpt-4o".into(),
        a2a_discovery_url: None,
        otel_endpoint: None,
        otel_protocol: "grpc".into(),
        otel_headers: None,
        otel_service_name: "nasiko-test".into(),
        otel_sample_ratio: "0.0".into(),
        otel_collector_endpoint: "http://localhost:4318".into(),
        otel_capture_content: false,
        coding_agent_otlp_endpoint: None,
        // Fixture turns are dated 2023, far outside Tempo's window; a test that
        // exercises the age cutoff sets this back to the production default.
        coding_agent_ingest: nasiko_config::CodingAgentIngestConfig {
            tempo_max_age_secs: u64::MAX / 4,
            ..Default::default()
        },
        tempo_url: "http://localhost:3200".into(),
        loki_url: "http://localhost:3100".into(),
        observability_enabled: false,
        tenant_id: None,
        // Off in tests: both loops hit the network on their first tick.
        model_catalog_sync_enabled: false,
        model_pricing_sync_enabled: false,
        flow_max_depth: 5,
        flow_max_fan_out: 20,
        flow_max_tokens: 100_000,
        flow_timeout_secs: 120,
        hitl_request_ttl_days: 7,
        hitl_resume_poll_interval_secs: 5,
        hitl_resume_recovery_interval_secs: 10 * 60,
        hitl_resume_lease_minutes: 2,
        hitl_resume_max_attempts: 3,
        hitl_resume_retry_delay_secs: 2,
        coding_agent_idle_ttl_secs: 0,
        coding_agent_turn_lease_secs: 60,
        coding_agent_lease_heartbeat_secs: 20,
        sandbox_provider: nasiko_config::SandboxProviderSetting::Docker,
        harness_sessions_enabled: true,
        build_sandbox_smoke: nasiko_config::BuildSandboxSmoke::Off,
        session_idle_grace_secs: 120,
        session_cpu_quiescent_millicores: 20,
        session_reaper_interval_secs: 30,
        agent_open_task_max_age_secs: 3600,
        agent_park_idle_secs: 3600,
        harness_image_prefix: "nasiko/harness-".to_string(),
        hitl_lost_task_retries: 3,
        agent_state_ttl_days: 7,
        agent_state_max_body_bytes: 1_048_576,
        agent_state_rate_limit_per_minute: 600,
        agent_peer_call_rate_limit_per_minute: 300,
        agent_state_list_bytes_budget: 8 * 1024 * 1024,
        github_client_id: None,
        github_client_secret: None,
        router_shortlist_threshold: 15,
        router_shortlist_size: 10,
        embedding_model: "text-embedding-3-small".into(),
        agent_call_timeout_secs: 600,
        agent_startup_budget_secs: 120,
        pacms_history_pool_size: 150,
        savings_factor_refresh_secs: 86_400,
        savings_factor_min_samples: 1_600,
        savings_factor_window_days: 30,
        minimal_code_holdout_pct: 0, // no flow withheld, so the ladder is deterministic under test
        react_compress_enabled: false,
        react_compress_min_bytes: 2048,
        history_compress_enabled: false,
        history_compress_min_bytes: 2048,
        pacms_budget_low: 500,
        pacms_budget_medium: 1000,
        pacms_budget_high: 5000,
        pacms_history_mandatory_recent: 3,
        context_k_low: 1,
        context_k_medium: 5,
        context_k_high: 20,
        github_callback_url: None,
        github_central_callback_url: None,
        docker_agent_network: None,
        oci_registry_host: None,
        container_hours_poll_secs: 0, // disabled so the background loop never races tests driving reconcile_once directly
        trace_usage_sync_secs: 0,
        trace_usage_overlap_secs: 180,
        trace_usage_max_window_secs: 600,
        trace_usage_batch_size: 8,
        git_clone_allowed_hosts: vec![
            "github.com".to_owned(),
            "gitlab.com".to_owned(),
            "bitbucket.org".to_owned(),
        ],
        registry_import_allowed_hosts: vec![],
        cors_allowed_origins: vec![],
        // Tests register agents at loopback mock servers. The SSRF guard's own
        // test (`agent_url_ssrf`) turns this off.
        agent_allow_private_urls: true,
        agent_current_time: true,
        oidc_issuer_url: None,
        oidc_client_id: None,
        oidc_client_secret: None,
        oidc_redirect_uri: None,
        oidc_allowed_redirect_origins: vec![],
        oidc_scopes: "".to_string(),
        oidc_provider_label: "".to_string(),
        oidc_central_callback_url: None,
        admin_username: "admin".into(),
        admin_password: "test-admin-password".into(),
        // Overridable so tests can point the Composio ToolProvider at a mockito
        // stub (there is no other seam to inject a fake provider — McpState builds
        // it straight from these two Config fields). Unset in the vast majority of
        // tests, which must keep seeing "Composio not configured" (COMPOSIO_API_KEY
        // unset in production === `composio_api_key: None`).
        composio_api_key: std::env::var("TEST_COMPOSIO_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        composio_base_url: std::env::var("TEST_COMPOSIO_BASE_URL")
            .unwrap_or_else(|_| "https://backend.composio.dev".into()),
        composio_webhook_secret: std::env::var("COMPOSIO_WEBHOOK_SECRET")
            .ok()
            .filter(|s| !s.is_empty()),
        // Overridable so the MCP OAuth callback round-trip test can exercise the
        // full `exchange_code` path (which needs `oauth_redirect_uri()` to be
        // `Some`). `None` by default, matching every other test's assumption
        // that the gateway's public URL is unconfigured.
        mcp_gateway_public_url: std::env::var("TEST_MCP_GATEWAY_PUBLIC_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        // None here too: falls back to mcp_gateway_public_url above, which the
        // oauth callback test already sets to an HTTPS test domain — no
        // separate override needed for that test to keep passing.
        mcp_oauth_redirect_base_url: None,
        composio_callback_base_url: None,
        mcp_session_ttl_seconds: 300,
        mcp_perm_cache_ttl_seconds: 30,
        mcp_manifest_ttl_seconds: 300,
        mcp_upload_max_bytes: 50 * 1024 * 1024,
        mcp_gateway_max_body_bytes: 8 * 1024 * 1024,
        mcp_upload_default_port: 8080,
        mcp_servers_network: "nasiko-mcp-servers-net".to_string(),
        mcp_upload_max_replicas: 1,
        agent_max_replicas: 1,
        // Serial builds in tests: several suites seed build_jobs rows and assert
        // on them, and a concurrent worker would make those assertions racy.
        database_max_connections: 10,
        dashboard_max_concurrent: 10,
        build_concurrency: 1,
        build_push_rate_limit_per_minute: 600,
        // Serial MAF runs in tests, for the same reason as build_concurrency above.
        maf_worker_concurrency: 1,
        maf_heartbeat_secs: 20,
        agent_default_memory: "512Mi".to_string(),
        agent_memory_volume: "nasiko-agent-memory".to_string(),
        agent_memory_init_image: "alpine:3.21".to_string(),
        mcp_toolcount_ttl_seconds: 3600,
        seed_toolkits: vec![],
        seed_connectors: String::new(),
        mcp_tool_search_mode: "".to_string(),
        mcp_tool_search_tool_limit: 0,
        app_base_url: "".to_string(),
        // Single-tenant test config — multi-tenant admission + CP-cookie paths off.
        multi_tenant_mode: false,
        allow_personal_emails: false,
        nasiko_bff_url: None,
        mcp_tool_search_meta_limit: 0,
        mcp_gateway_instructions: String::new(),
        // This literal is built directly, not through `Config::from_env` — so
        // it bypasses that function's JWT_SECRET-derived fallback (which
        // domain-separates with an `mcp-identity::` prefix; see
        // `Config::from_env`'s own derivation in `nasiko-config`). Set directly to
        // `TEST_JWT_SECRET`'s own value, unprefixed, so tests deriving the
        // signing key for `identity::SignedIdentity::verify` have a fixed,
        // known value to use (see `mcp_system_connector.rs`).
        mcp_identity_signing_key: TEST_JWT_SECRET.to_string(),
    }
}

async fn fallback() -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_FOUND
}

// ─── JWT test helpers ─────────────────────────────────────────────────────────

/// JWT secret used in all OSS integration tests — must match test_config().
pub const TEST_JWT_SECRET: &str = "test-secret-for-nasiko-tests";

/// Sign a short-lived JWT for a test identity using the known test secret.
#[allow(dead_code)]
pub fn sign_token(user_id: &str, username: &str, is_superuser: bool, _role: &str) -> String {
    // _role is accepted for call-site compatibility but no longer part of Identity —
    // role is an internal EE detail resolved from the DB, never carried in the token.
    let identity = nasiko_auth::Identity {
        user_id: user_id.to_owned(),
        username: username.to_owned(),
        is_superuser,
    };
    nasiko_auth::jwt::encode_jwt(TEST_JWT_SECRET, 3600, &identity).expect("test JWT signing failed")
}

/// Sign a short-lived agent-typed JWT (as minted by `issue_agent_token`) —
/// `encode_agent_jwt` stamps `token_type = "agent"`, which `decode_jwt`/
/// `decode_jwt_with_jti` reject outright (AUTH-3), so this must never be
/// usable to authenticate as a user via `require_auth`.
#[allow(dead_code)]
pub fn sign_agent_token(agent_id: &str) -> String {
    let identity = nasiko_auth::Identity {
        user_id: agent_id.to_owned(),
        username: format!("agent:{agent_id}"),
        is_superuser: false,
    };
    nasiko_auth::jwt::encode_agent_jwt(TEST_JWT_SECRET, 3600, &identity)
        .expect("test JWT signing failed")
}

/// Attach a superuser (admin role) JWT to a request builder.
#[allow(dead_code)]
pub fn as_superuser(
    rb: reqwest::RequestBuilder,
    user_id: &str,
    username: &str,
) -> reqwest::RequestBuilder {
    rb.bearer_auth(sign_token(user_id, username, true, "admin"))
}

/// Attach a member JWT to a request builder.
#[allow(dead_code)]
pub fn as_member(
    rb: reqwest::RequestBuilder,
    user_id: &str,
    username: &str,
) -> reqwest::RequestBuilder {
    rb.bearer_auth(sign_token(user_id, username, false, "member"))
}

// ─── MCP gateway auth test helpers (docs/MCP_GATEWAY_AGENT_AUTH.md) ──────────

/// Mint the agent's MCP gateway credential — the plaintext that deploy-time
/// wiring injects into the container env as `MCP_GATEWAY_TOKEN`.
#[allow(dead_code)]
pub async fn mint_gateway_token(db: &sqlx::PgPool, agent_id: uuid::Uuid) -> String {
    nasiko_mcp_gateway::agent_tokens::mint(db, agent_id)
        .await
        .expect("mint gateway token")
}

/// Open a live flow for `(user, agent)` exactly the way a dispatch path does —
/// `flows` row + `flow_participants` record — and return `(flow_id,
/// traceparent)`, where the traceparent is what the platform forwards to the
/// agent (its trace id IS the flow id).
#[allow(dead_code)]
pub async fn open_flow(
    db: &sqlx::PgPool,
    user_id: uuid::Uuid,
    agent_id: uuid::Uuid,
) -> (String, String) {
    let flow_id = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_id, status, metadata, session_id)
         VALUES ($1, $2, $3, 'running', '{}'::jsonb, $1)",
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(agent_id)
    .execute(db)
    .await
    .expect("insert flows row");
    sqlx::query("INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)")
        .bind(&flow_id)
        .bind(agent_id)
        .execute(db)
        .await
        .expect("insert flow participant");
    let traceparent = format!("00-{flow_id}-00f067aa0ba902b7-01");
    (flow_id, traceparent)
}

/// Attach HTTP Basic auth — the credential type the OCI registry's pull-only
/// path (`nasiko_oci::pull_credentials`) accepts, distinct from the bearer-JWT
/// paths above.
#[allow(dead_code)]
pub fn as_pull_credential(
    rb: reqwest::RequestBuilder,
    username: &str,
    password: &str,
) -> reqwest::RequestBuilder {
    rb.basic_auth(username, Some(password))
}

/// Build an in-memory zip archive from `(path, bytes)` pairs.
#[allow(dead_code)]
pub fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }
    cursor.into_inner()
}
