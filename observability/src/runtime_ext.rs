//! Deploy-time instrumentation decorators.
//!
//! [`InstrumentedRuntime`] wraps a [`ContainerRuntime`] and patches every [`DeploymentSpec`];
//! [`InstrumentedSandbox`] wraps a [`SandboxProvider`] and patches every [`SandboxSpec`] at
//! `acquire`. Both run the same [`InstrumentationInjector`]s, so a deployed agent, a woken
//! framework agent and a harness session export telemetry the same way.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nasiko_runtime::{
    BuildReport, ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, InstanceInfo,
    Result, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider, SandboxProviderKind,
    SandboxSpec, SandboxState, SandboxStats, SandboxTransport, SessionAttach, StdioHandle,
    WorkspaceEntry, WorkspaceFile, WorkspaceRef,
};

use crate::injector::{AgentContext, InstrumentationInjector, OtelInjector};

/// Wraps any [`ContainerRuntime`] and injects OTEL environment variables into
/// every [`DeploymentSpec`] before forwarding to the inner runtime.
///
/// This is a zero-overhead decorator: all methods except `deploy` are pure
/// pass-throughs.
///
/// # Example
///
/// ```rust,ignore
/// let runtime = InstrumentedRuntime::new(
///     DockerRuntime::new(config),
///     OtelInjector,
///     "http://otel-collector.nasiko-infra:4317".into(),
///     "grpc".into(),
///     false,
///     None,
/// );
/// ```
pub struct InstrumentedRuntime<R, I> {
    inner: R,
    injector: I,
    otel_collector_endpoint: String,
    otel_protocol: String,
    capture_content: bool,
    tenant_id: Option<String>,
}

impl<R: ContainerRuntime, I: InstrumentationInjector> InstrumentedRuntime<R, I> {
    pub fn new(
        inner: R,
        injector: I,
        otel_collector_endpoint: String,
        otel_protocol: String,
        capture_content: bool,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            inner,
            injector,
            otel_collector_endpoint,
            otel_protocol,
            capture_content,
            tenant_id,
        }
    }
}

#[async_trait]
impl<R: ContainerRuntime, I: InstrumentationInjector> ContainerRuntime
    for InstrumentedRuntime<R, I>
{
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<DeploymentStatus> {
        let mut patched = spec.clone();
        let ctx = AgentContext {
            agent_id: spec.name.clone(),
            agent_uuid: workload_uuid(&spec.container_id),
            tenant_id: self.tenant_id.clone(),
            version: None,
            capture_content: self.capture_content,
            otel_collector_endpoint: self.otel_collector_endpoint.clone(),
            otel_protocol: self.otel_protocol.clone(),
        };
        self.injector.inject(&mut patched.env_vars, &ctx);
        self.inner.deploy(&patched).await
    }

    async fn destroy(&self, container_id: &ContainerId) -> Result<()> {
        self.inner.destroy(container_id).await
    }

    async fn scale(&self, container_id: &ContainerId, replicas: u32) -> Result<()> {
        self.inner.scale(container_id, replicas).await
    }

    async fn restart(&self, container_id: &ContainerId) -> Result<()> {
        self.inner.restart(container_id).await
    }

    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus> {
        self.inner.status(container_id).await
    }

    async fn list(&self) -> Result<Vec<DeploymentStatus>> {
        self.inner.list().await
    }

    async fn endpoint(&self, container_id: &ContainerId) -> Result<String> {
        self.inner.endpoint(container_id).await
    }

    /// Forward the stdio attach to the inner runtime. MUST be overridden here —
    /// the trait's default is `Unsupported`, so without this a coding-agent
    /// attach would be rejected at the decorator and never reach the backend
    /// that supports it (RUN-1). Injection does not apply: the stream carries
    /// JSON-RPC, not a `DeploymentSpec`.
    async fn attach_stdio(
        &self,
        container_id: &ContainerId,
        attach: SessionAttach,
    ) -> Result<StdioHandle> {
        self.inner.attach_stdio(container_id, attach).await
    }

    async fn logs(&self, container_id: &ContainerId, tail: u32) -> Result<Vec<String>> {
        self.inner.logs(container_id, tail).await
    }

    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<BuildReport> {
        self.inner.build(tar_context, image_tag).await
    }

    /// Forward to the inner runtime. MUST be overridden here - the trait's default
    /// is `false`, so without this an autoscaling backend would look unable to
    /// scale and every agent would be pinned to one replica (RUN-1).
    async fn supports_autoscaling(&self) -> bool {
        self.inner.supports_autoscaling().await
    }

    /// Forward to the inner runtime. MUST be overridden here - the trait's default
    /// is "unsupported", so without this no bounds change would ever reach the
    /// backend (RUN-1).
    async fn apply_replica_bounds(
        &self,
        id: &ContainerId,
        min: u32,
        max: u32,
        target: Option<u32>,
    ) -> Result<()> {
        self.inner.apply_replica_bounds(id, min, max, target).await
    }

    /// Forward to the inner runtime. MUST be overridden here - the trait's default is
    /// `false`, so without this a backend that rolls restarts in place would be
    /// redeployed (destroyed and recreated) on every restart (RUN-1).
    async fn restarts_in_place(&self) -> bool {
        self.inner.restarts_in_place().await
    }

    /// Inject, then forward. MUST be overridden here — the trait's default is a
    /// no-op, so without this a K8s Secret rotation would be silently dropped by
    /// the decorator (RUN-1) and never reach KubeRuntime.
    ///
    /// The injection is not optional: `refresh_secrets` carries the agent's whole
    /// desired environment and the Kubernetes backend applies it as such, so
    /// forwarding the caller's raw map would *delete* every variable this
    /// decorator stack adds at `deploy()` — `OTEL_*` from `OtelInjector`,
    /// `MCP_GATEWAY_URL`/`MCP_GATEWAY_CONNECT_URL` from `McpInjector`. A restarted
    /// agent would come back with no telemetry and no gateway endpoint. Running
    /// the same injector here keeps refresh and deploy producing identical
    /// environments.
    async fn refresh_secrets(
        &self,
        id: &ContainerId,
        name: &str,
        env_vars: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut patched = env_vars;
        let ctx = AgentContext {
            agent_id: name.to_string(),
            // The same value `deploy` injected, or a refresh would change the env and drop it.
            agent_uuid: workload_uuid(id),
            tenant_id: self.tenant_id.clone(),
            version: None,
            capture_content: self.capture_content,
            otel_collector_endpoint: self.otel_collector_endpoint.clone(),
            otel_protocol: self.otel_protocol.clone(),
        };
        self.injector.inject(&mut patched, &ctx);
        self.inner.refresh_secrets(id, name, patched).await
    }

    /// Forward instance listing to the inner runtime. MUST be overridden here — the
    /// trait's default synthesizes entries from `list()` against the decorator, so
    /// without this the inner backend's per-instance identity (container IDs, pod
    /// UIDs, true start times) would be silently bypassed (RUN-1).
    async fn list_instances(&self) -> Result<Vec<InstanceInfo>> {
        self.inner.list_instances().await
    }

    async fn troubled(&self) -> Result<Option<Vec<ContainerId>>> {
        self.inner.troubled().await
    }

    async fn is_serving(&self, container_id: &ContainerId) -> Result<bool> {
        self.inner.is_serving(container_id).await
    }

    async fn wait_routable(
        &self,
        container_id: &ContainerId,
        timeout: std::time::Duration,
    ) -> Result<String> {
        self.inner.wait_routable(container_id, timeout).await
    }

    /// Forward workspace listing to the inner runtime. MUST be overridden here —
    /// the trait's default is an empty list, so without this every agent's
    /// files would look absent no matter what the backend can see (RUN-1).
    async fn list_workspace(&self, workspace: &WorkspaceRef) -> Result<Vec<WorkspaceEntry>> {
        self.inner.list_workspace(workspace).await
    }

    /// Forward workspace file reads to the inner runtime. MUST be overridden
    /// here — the trait's default is "unsupported", so without this every
    /// download would fail even on a backend that supports it (RUN-1).
    async fn read_workspace_file(
        &self,
        workspace: &WorkspaceRef,
        rel_path: &str,
    ) -> Result<WorkspaceFile> {
        self.inner.read_workspace_file(workspace, rel_path).await
    }
}

/// Wraps any [`SandboxProvider`] and injects instrumentation env into every [`SandboxSpec`]
/// at `acquire`, like [`InstrumentedRuntime`] does for deployments. Every other method
/// delegates unchanged.
///
/// [`OtelInjector`] runs for every [`SandboxKey`]. Injectors added with
/// [`Self::with_agent_injector`] (the MCP injector) run **only** for `Agent` and
/// `AgentContext` keys: a harness session already carries its own `MCP_GATEWAY_*` from its
/// session env, and the per-agent injector would overwrite it. Like the runtime decorator,
/// injectors overwrite any value already in the spec.
pub struct InstrumentedSandbox<P: ?Sized> {
    inner: Arc<P>,
    agent_injectors: Vec<Box<dyn InstrumentationInjector>>,
    otel_collector_endpoint: String,
    otel_protocol: String,
    capture_content: bool,
    tenant_id: Option<String>,
}

impl<P: SandboxProvider + ?Sized> InstrumentedSandbox<P> {
    pub fn new(
        inner: Arc<P>,
        otel_collector_endpoint: String,
        otel_protocol: String,
        capture_content: bool,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            inner,
            agent_injectors: Vec::new(),
            otel_collector_endpoint,
            otel_protocol,
            capture_content,
            tenant_id,
        }
    }

    /// Add an injector that runs for agent-keyed sandboxes only.
    pub fn with_agent_injector(mut self, injector: impl InstrumentationInjector + 'static) -> Self {
        self.agent_injectors.push(Box::new(injector));
        self
    }

    fn patched(&self, spec: &SandboxSpec) -> SandboxSpec {
        let mut patched = spec.clone();
        patched.env.extend(sandbox_env_additions(
            &spec.key,
            &self.otel_collector_endpoint,
            &self.otel_protocol,
            self.capture_content,
            self.tenant_id.as_deref(),
        ));
        if !matches!(spec.key, SandboxKey::Session { .. }) {
            let ctx = agent_context(
                &spec.key,
                &self.otel_collector_endpoint,
                &self.otel_protocol,
                self.capture_content,
                self.tenant_id.as_deref(),
            );
            for injector in &self.agent_injectors {
                injector.inject(&mut patched.env, &ctx);
            }
        }
        patched
    }
}

/// A workload's container id as the UUID it was made from, or `None` for one that is not one.
fn workload_uuid(id: &ContainerId) -> Option<String> {
    uuid::Uuid::parse_str(id.as_str())
        .ok()
        .map(|u| u.to_string())
}

fn agent_context(
    key: &SandboxKey,
    otel_collector_endpoint: &str,
    otel_protocol: &str,
    capture_content: bool,
    tenant_id: Option<&str>,
) -> AgentContext {
    AgentContext {
        agent_id: service_name(key),
        // A sandbox's service name already is its UUID (or `harness-<session>`).
        agent_uuid: None,
        tenant_id: tenant_id.map(str::to_owned),
        version: None,
        capture_content,
        otel_collector_endpoint: otel_collector_endpoint.to_owned(),
        otel_protocol: otel_protocol.to_owned(),
    }
}

/// The env [`InstrumentedSandbox`] adds to every sandbox of `key` at `acquire`. A caller that
/// hands the sandbox its settings some other way than the spec's env (a shim configured over
/// HTTP) applies the same additions with this function, so the two cannot drift.
pub fn sandbox_env_additions(
    key: &SandboxKey,
    otel_collector_endpoint: &str,
    otel_protocol: &str,
    capture_content: bool,
    tenant_id: Option<&str>,
) -> HashMap<String, String> {
    let ctx = agent_context(
        key,
        otel_collector_endpoint,
        otel_protocol,
        capture_content,
        tenant_id,
    );
    let mut env = HashMap::new();
    OtelInjector.inject(&mut env, &ctx);
    env
}

/// The OTEL service name: the agent id for agent sandboxes (as for deployed agents),
/// `harness-<session id>` for a session.
fn service_name(key: &SandboxKey) -> String {
    match key {
        SandboxKey::Session { session_id } => format!("harness-{session_id}"),
        SandboxKey::Agent { agent_id } | SandboxKey::AgentContext { agent_id, .. } => {
            agent_id.to_string()
        }
    }
}

#[async_trait]
impl<P: SandboxProvider + ?Sized> SandboxProvider for InstrumentedSandbox<P> {
    fn kind(&self) -> SandboxProviderKind {
        self.inner.kind()
    }

    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn is_available_for(&self, key: &SandboxKey) -> bool {
        self.inner.is_available_for(key)
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        self.inner.acquire(&self.patched(spec)).await
    }

    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport> {
        self.inner.attach(handle).await
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        self.inner.status(handle).await
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        self.inner.suspend(handle).await
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        self.inner.release(handle).await
    }

    async fn resume_takes_new_env(&self, handle: &SandboxHandle) -> Result<bool> {
        self.inner.resume_takes_new_env(handle).await
    }

    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats> {
        self.inner.stats(handle).await
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        self.inner.list().await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn env_additions_are_what_the_decorator_injects_into_a_session_sandbox() {
        let key = SandboxKey::Session {
            session_id: uuid::Uuid::nil(),
        };
        let env = sandbox_env_additions(&key, "http://collector:4317", "grpc", true, Some("t1"));
        assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://collector:4317");
        assert_eq!(
            env["OTEL_SERVICE_NAME"],
            format!("harness-{}", uuid::Uuid::nil())
        );
        assert!(env["OTEL_RESOURCE_ATTRIBUTES"].contains("tenant.id=t1"));
    }

    use super::*;
    use crate::injector::{AgentContext, OtelInjector};
    use nasiko_runtime::RuntimeState;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Inner runtime that records whether `refresh_secrets` reached it, and with
    /// what environment.
    struct RecordingRuntime {
        refreshed: Arc<AtomicBool>,
        refresh_env: Arc<Mutex<HashMap<String, String>>>,
    }

    #[async_trait]
    impl ContainerRuntime for RecordingRuntime {
        async fn deploy(&self, s: &DeploymentSpec) -> Result<DeploymentStatus> {
            Ok(DeploymentStatus {
                container_id: s.container_id.clone(),
                state: RuntimeState::Running,
                replicas_live: s.min_replicas,
                endpoint: None,
                message: None,
                restart_count: 0,
            })
        }
        async fn destroy(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }
        async fn scale(&self, _id: &ContainerId, _r: u32) -> Result<()> {
            Ok(())
        }
        async fn restart(&self, _id: &ContainerId) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _id: &ContainerId) -> Result<DeploymentStatus> {
            unimplemented!()
        }
        async fn list(&self) -> Result<Vec<DeploymentStatus>> {
            Ok(vec![])
        }
        async fn endpoint(&self, _id: &ContainerId) -> Result<String> {
            Ok(String::new())
        }
        async fn logs(&self, _id: &ContainerId, _t: u32) -> Result<Vec<String>> {
            Ok(vec![])
        }
        async fn build(&self, _c: &[u8], t: &str) -> Result<BuildReport> {
            Ok(BuildReport::image_only(t))
        }
        async fn refresh_secrets(
            &self,
            _id: &ContainerId,
            _name: &str,
            env: HashMap<String, String>,
        ) -> Result<()> {
            self.refreshed.store(true, Ordering::SeqCst);
            *self.refresh_env.lock().unwrap() = env;
            Ok(())
        }
        async fn supports_autoscaling(&self) -> bool {
            true
        }
        async fn restarts_in_place(&self) -> bool {
            true
        }
        async fn apply_replica_bounds(
            &self,
            _id: &ContainerId,
            _min: u32,
            _max: u32,
            _target: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }
        async fn list_instances(&self) -> Result<Vec<InstanceInfo>> {
            Ok(vec![InstanceInfo {
                container_id: ContainerId::new("agent"),
                instance_key: "marker-instance".to_string(),
                started_at: None,
                ready: true,
            }])
        }
        async fn troubled(&self) -> Result<Option<Vec<ContainerId>>> {
            Ok(Some(vec![ContainerId::new("crashing-agent")]))
        }
        /// Answers without `status()`, which this double does not implement: the decorator
        /// reaching the trait's default instead would panic.
        async fn is_serving(&self, _id: &ContainerId) -> Result<bool> {
            Ok(true)
        }
        async fn list_workspace(&self, _w: &WorkspaceRef) -> Result<Vec<WorkspaceEntry>> {
            Ok(vec![WorkspaceEntry {
                path: "marker-file".to_string(),
                size: 3,
                mtime: 0,
            }])
        }
        async fn read_workspace_file(
            &self,
            _w: &WorkspaceRef,
            _rel: &str,
        ) -> Result<WorkspaceFile> {
            Ok(WorkspaceFile {
                size: 3,
                stream: Box::pin(futures_util::stream::once(async {
                    Ok(bytes::Bytes::from_static(b"abc"))
                })),
            })
        }
    }

    struct NoopInjector;
    impl InstrumentationInjector for NoopInjector {
        fn inject(&self, _env: &mut HashMap<String, String>, _ctx: &AgentContext) {}
    }

    /// Records the `tenant_id` it was given, so a test can assert on it
    /// without depending on `OtelInjector`'s specific env-var encoding.
    struct CapturingInjector {
        seen_tenant_id: Arc<Mutex<Option<String>>>,
    }
    impl InstrumentationInjector for CapturingInjector {
        fn inject(&self, _env: &mut HashMap<String, String>, ctx: &AgentContext) {
            *self.seen_tenant_id.lock().unwrap() = ctx.tenant_id.clone();
        }
    }

    fn minimal_spec() -> DeploymentSpec {
        DeploymentSpec {
            container_id: ContainerId::new("agent"),
            name: "agent".to_string(),
            image: "example/agent:latest".to_string(),
            min_replicas: 1,
            max_replicas: 1,
            replica_target: None,
            env_vars: HashMap::new(),
            ports: vec![8080],
            resources: None,
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
            harden: false,
            network_override: None,
            workload_kind: Default::default(),
            writable: false,
            writable_path: None,
            owner_id: uuid::Uuid::nil(),
            force_pull: false,
            parkable: false,
            overrides: Default::default(),
        }
    }

    /// Regression guard: `InstrumentedRuntime::deploy` must pass its own
    /// configured `tenant_id` through to `AgentContext`, not the hardcoded
    /// `None` this decorator used before tenant_id was wired up.
    #[tokio::test]
    async fn deploy_passes_configured_tenant_id_to_agent_context() {
        let seen = Arc::new(Mutex::new(None));
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            CapturingInjector {
                seen_tenant_id: seen.clone(),
            },
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            Some("tenant-abc".to_string()),
        );
        rt.deploy(&minimal_spec())
            .await
            .expect("deploy should succeed");
        assert_eq!(seen.lock().unwrap().as_deref(), Some("tenant-abc"));
    }

    /// A runtime with no configured tenant_id must still deploy with `None`
    /// (a standalone, non-multi-tenant deployment) — not silently invent one.
    #[tokio::test]
    async fn deploy_passes_none_tenant_id_when_unconfigured() {
        let seen = Arc::new(Mutex::new(Some("should-be-overwritten".to_string())));
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            CapturingInjector {
                seen_tenant_id: seen.clone(),
            },
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        rt.deploy(&minimal_spec())
            .await
            .expect("deploy should succeed");
        assert_eq!(*seen.lock().unwrap(), None);
    }

    /// RUN-1 regression guard: the decorator must forward `refresh_secrets` to the
    /// inner runtime, not fall through to the trait's no-op default (which would
    /// silently drop a K8s secret rotation).
    #[tokio::test]
    async fn refresh_secrets_forwards_to_inner() {
        let flag = Arc::new(AtomicBool::new(false));
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: flag.clone(),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        rt.refresh_secrets(&ContainerId::new("agent"), "agent", HashMap::new())
            .await
            .expect("refresh_secrets should succeed");
        assert!(
            flag.load(Ordering::SeqCst),
            "InstrumentedRuntime must forward refresh_secrets to the inner runtime (RUN-1)"
        );
    }

    /// `refresh_secrets` carries the agent's *whole* desired environment and the
    /// K8s backend server-side-applies it, so anything the injector adds at
    /// `deploy()` must be re-added here. Without this, a K8s restart silently
    /// stripped `OTEL_*` and the MCP gateway endpoint from the live Secret and the
    /// agent came back blind and gateway-less.
    #[tokio::test]
    async fn refresh_secrets_reinjects_instrumentation() {
        let captured = Arc::new(Mutex::new(HashMap::new()));
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: captured.clone(),
            },
            OtelInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            Some("tenant-a".to_string()),
        );

        let mut caller_env = HashMap::new();
        caller_env.insert("AGENT_SECRET".to_string(), "kept".to_string());
        rt.refresh_secrets(&ContainerId::new("agent-uuid"), "weather", caller_env)
            .await
            .expect("refresh_secrets should succeed");

        let env = captured.lock().unwrap();
        assert_eq!(
            env.get("OTEL_EXPORTER_OTLP_ENDPOINT").map(String::as_str),
            Some("http://collector:4318"),
            "injected OTEL config must survive a secret refresh"
        );
        assert_eq!(
            env.get("AGENT_SECRET").map(String::as_str),
            Some("kept"),
            "the caller's own secrets must still be forwarded"
        );
        // The decorator only has a ContainerId, which is the agent UUID; the
        // service name must come from the name the caller threads through, or
        // every restarted agent renames itself to a UUID in Tempo.
        assert_eq!(
            env.get("OTEL_SERVICE_NAME").map(String::as_str),
            Some("weather"),
            "refresh must use the agent name, not the container id, as service name"
        );
    }

    /// A decorator over `RecordingRuntime` with the injector that changes
    /// nothing — the shape every forwarding guard below needs.
    fn instrumented() -> InstrumentedRuntime<RecordingRuntime, NoopInjector> {
        InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        )
    }

    /// RUN-1 regression guard: the trait's default is `false`, so only forwarding
    /// reports the inner runtime's `true`.
    #[tokio::test]
    async fn supports_autoscaling_forwards_to_inner() {
        assert!(
            instrumented().supports_autoscaling().await,
            "InstrumentedRuntime must forward supports_autoscaling (RUN-1)"
        );
    }

    /// RUN-1 regression guard: the trait's default is `false`, so only forwarding
    /// reports the inner runtime's `true`.
    #[tokio::test]
    async fn restarts_in_place_forwards_to_inner() {
        assert!(
            instrumented().restarts_in_place().await,
            "InstrumentedRuntime must forward restarts_in_place (RUN-1)"
        );
    }

    /// RUN-1 regression guard: the trait's default is an error, so merely
    /// succeeding proves forwarding.
    #[tokio::test]
    async fn apply_replica_bounds_forwards_to_inner() {
        instrumented()
            .apply_replica_bounds(&ContainerId::new("agent"), 1, 3, Some(2))
            .await
            .expect("InstrumentedRuntime must forward apply_replica_bounds (RUN-1)");
    }

    /// RUN-1 regression guard: the decorator must forward `list_workspace`. The
    /// trait's default returns `[]`, so the marker entry is what distinguishes
    /// "forwarded" from "silently fell through to the default".
    #[tokio::test]
    async fn list_workspace_forwards_to_inner() {
        let rt = instrumented();
        let files = rt
            .list_workspace(&WorkspaceRef {
                owner_id: uuid::Uuid::nil(),
                container_id: ContainerId::new("agent"),
            })
            .await
            .expect("list_workspace should succeed");
        assert_eq!(
            files.first().map(|f| f.path.as_str()),
            Some("marker-file"),
            "InstrumentedRuntime must forward list_workspace to the inner runtime (RUN-1)"
        );
    }

    /// RUN-1 regression guard: the decorator must forward `read_workspace_file`.
    /// The trait's default is an error, so merely succeeding proves forwarding.
    #[tokio::test]
    async fn read_workspace_file_forwards_to_inner() {
        let rt = instrumented();
        let file = rt
            .read_workspace_file(
                &WorkspaceRef {
                    owner_id: uuid::Uuid::nil(),
                    container_id: ContainerId::new("agent"),
                },
                "marker-file",
            )
            .await
            .expect("InstrumentedRuntime must forward read_workspace_file (RUN-1)");
        assert_eq!(file.size, 3);
    }

    /// The decorator must forward `troubled`: the trait default answers `None`, which
    /// would silently send a periodic sweep back to checking every agent.
    #[tokio::test]
    async fn troubled_forwards_to_inner() {
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        assert_eq!(
            rt.troubled().await.expect("troubled should succeed"),
            Some(vec![ContainerId::new("crashing-agent")])
        );
    }

    #[tokio::test]
    async fn is_serving_forwards_to_inner() {
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        assert!(
            rt.is_serving(&ContainerId::new("agent"))
                .await
                .expect("is_serving should succeed")
        );
    }

    /// RUN-1 regression guard: the decorator must forward `list_instances` to the
    /// inner runtime. The trait's default synthesizes entries from `list()` — which
    /// returns `[]` for `RecordingRuntime` — so an empty result here would mean
    /// default-impl fallthrough, while the marker instance proves forwarding.
    #[tokio::test]
    async fn list_instances_forwards_to_inner() {
        let rt = InstrumentedRuntime::new(
            RecordingRuntime {
                refreshed: Arc::new(AtomicBool::new(false)),
                refresh_env: Arc::new(Mutex::new(HashMap::new())),
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        let instances = rt
            .list_instances()
            .await
            .expect("list_instances should succeed");
        assert_eq!(
            instances,
            vec![InstanceInfo {
                container_id: ContainerId::new("agent"),
                instance_key: "marker-instance".to_string(),
                started_at: None,
                ready: true,
            }],
            "InstrumentedRuntime must forward list_instances to the inner runtime (RUN-1)"
        );
    }

    mod sandbox {
        use super::super::*;
        use nasiko_runtime::TransportKind;
        use nasiko_runtime::sandbox_fake::FakeSandboxProvider;
        use std::collections::HashMap;
        use uuid::Uuid;

        struct MarkerInjector;
        impl InstrumentationInjector for MarkerInjector {
            fn inject(&self, env: &mut HashMap<String, String>, _ctx: &AgentContext) {
                env.insert("MCP_GATEWAY_URL".into(), "http://gw".into());
                env.insert("MCP_GATEWAY_TOKEN".into(), "agent-token".into());
            }
        }

        fn spec(key: SandboxKey, env: &[(&str, &str)]) -> SandboxSpec {
            SandboxSpec {
                key,
                image: "img:1".into(),
                command: vec![],
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                resources: None,
                workspace: None,
                runtime_class: None,
                ports: vec![8080],
                transport: TransportKind::Http,
                image_pull_secret: None,
            }
        }

        fn wrapped() -> (
            Arc<FakeSandboxProvider>,
            InstrumentedSandbox<FakeSandboxProvider>,
        ) {
            let fake = Arc::new(FakeSandboxProvider::default());
            let sandbox = InstrumentedSandbox::new(
                fake.clone(),
                "http://collector:4317".into(),
                "grpc".into(),
                false,
                None,
            )
            .with_agent_injector(MarkerInjector);
            (fake, sandbox)
        }

        #[tokio::test]
        async fn session_gets_otel_but_not_agent_injector_env() {
            let (fake, sandbox) = wrapped();
            let id = Uuid::new_v4();
            let key = SandboxKey::Session { session_id: id };
            sandbox
                .acquire(&spec(
                    key.clone(),
                    &[("MCP_GATEWAY_TOKEN", "session-token")],
                ))
                .await
                .unwrap();
            let env = fake.last_acquire_spec(&key).unwrap().env;
            assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://collector:4317");
            assert_eq!(env["OTEL_SERVICE_NAME"], format!("harness-{id}"));
            assert_eq!(env["MCP_GATEWAY_TOKEN"], "session-token");
            assert!(!env.contains_key("MCP_GATEWAY_URL"));
        }

        #[tokio::test]
        async fn agent_keys_get_otel_and_agent_injector_env() {
            let (fake, sandbox) = wrapped();
            let id = Uuid::new_v4();
            for key in [
                SandboxKey::Agent { agent_id: id },
                SandboxKey::AgentContext {
                    agent_id: id,
                    context_id: "c1".into(),
                },
            ] {
                sandbox.acquire(&spec(key.clone(), &[])).await.unwrap();
                let env = fake.last_acquire_spec(&key).unwrap().env;
                assert_eq!(env["OTEL_SERVICE_NAME"], id.to_string());
                assert_eq!(env["MCP_GATEWAY_URL"], "http://gw");
                assert_eq!(env["MCP_GATEWAY_TOKEN"], "agent-token");
            }
        }

        #[tokio::test]
        async fn injectors_overwrite_existing_env_like_the_runtime_decorator() {
            let (fake, sandbox) = wrapped();
            let key = SandboxKey::Agent {
                agent_id: Uuid::new_v4(),
            };
            sandbox
                .acquire(&spec(
                    key.clone(),
                    &[("OTEL_SERVICE_NAME", "mine"), ("KEEP", "1")],
                ))
                .await
                .unwrap();
            let env = fake.last_acquire_spec(&key).unwrap().env;
            assert_ne!(env["OTEL_SERVICE_NAME"], "mine");
            assert_eq!(env["KEEP"], "1");
        }

        #[tokio::test]
        async fn other_methods_delegate() {
            let (fake, sandbox) = wrapped();
            assert_eq!(sandbox.kind(), SandboxProviderKind::Fake);
            assert!(sandbox.is_available());
            let key = SandboxKey::Agent {
                agent_id: Uuid::new_v4(),
            };
            assert!(sandbox.is_available_for(&key));
            fake.set_unavailable(true);
            assert!(!sandbox.is_available_for(&key));
            fake.set_unavailable(false);
            let handle = sandbox.acquire(&spec(key, &[])).await.unwrap();
            assert!(!sandbox.resume_takes_new_env(&handle).await.unwrap());
            fake.set_resume_takes_new_env(true);
            assert!(sandbox.resume_takes_new_env(&handle).await.unwrap());
            assert_eq!(sandbox.list().await.unwrap().len(), 1);
            sandbox.release(&handle).await.unwrap();
        }
    }
}
