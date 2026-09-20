use async_trait::async_trait;
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, InstanceInfo, Result,
};

use crate::injector::{AgentContext, InstrumentationInjector};

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

    async fn logs(&self, container_id: &ContainerId, tail: u32) -> Result<Vec<String>> {
        self.inner.logs(container_id, tail).await
    }

    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<String> {
        self.inner.build(tar_context, image_tag).await
    }

    async fn try_delete_autoscaler(&self, id: &ContainerId) -> Result<()> {
        self.inner.try_delete_autoscaler(id).await
    }

    /// Forward secret refresh to the inner runtime. MUST be overridden here — the
    /// trait's default is a no-op, so without this a K8s Secret rotation would be
    /// silently dropped by the decorator (RUN-1) and never reach KubeRuntime.
    async fn refresh_secrets(
        &self,
        id: &ContainerId,
        env_vars: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.inner.refresh_secrets(id, env_vars).await
    }

    /// Forward instance listing to the inner runtime. MUST be overridden here — the
    /// trait's default synthesizes entries from `list()` against the decorator, so
    /// without this the inner backend's per-instance identity (container IDs, pod
    /// UIDs, true start times) would be silently bypassed (RUN-1).
    async fn list_instances(&self) -> Result<Vec<InstanceInfo>> {
        self.inner.list_instances().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::AgentContext;
    use nasiko_runtime::{RuntimeError, RuntimeState};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// What the mock's `status()` should return.
    ///
    /// `status()` is the one `ContainerRuntime` method whose *failure* path matters to
    /// callers — a missing container and a crashed one are different conditions, and
    /// code that branches on them needs both reachable from a test. Modelling the
    /// outcome as data (rather than a fixed `Ok`) keeps those paths deterministic
    /// without a mocking framework.
    #[derive(Clone)]
    enum StatusBehavior {
        /// Succeed, reporting this lifecycle state for the queried container.
        State(RuntimeState),
        /// Fail as if no container exists for the queried id.
        NotFound,
        /// Fail with a backend-internal error carrying this message.
        Internal(String),
    }

    impl Default for StatusBehavior {
        /// A healthy, running agent — the common case, so tests that do not care
        /// about `status()` need not mention it.
        fn default() -> Self {
            Self::State(RuntimeState::Running)
        }
    }

    /// Inner runtime that records whether `refresh_secrets` reached it.
    #[derive(Default)]
    struct RecordingRuntime {
        refreshed: Arc<AtomicBool>,
        status: StatusBehavior,
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
        async fn status(&self, id: &ContainerId) -> Result<DeploymentStatus> {
            match &self.status {
                StatusBehavior::State(state) => Ok(DeploymentStatus {
                    container_id: id.clone(),
                    state: *state,
                    // Only a running agent has a live replica; every other state
                    // reports none, matching what a real backend surfaces.
                    replicas_live: u32::from(*state == RuntimeState::Running),
                    endpoint: None,
                    message: None,
                    restart_count: 0,
                }),
                StatusBehavior::NotFound => Err(RuntimeError::ContainerNotFound(id.clone())),
                StatusBehavior::Internal(msg) => Err(RuntimeError::Internal(msg.clone())),
            }
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
        async fn build(&self, _c: &[u8], _t: &str) -> Result<String> {
            Ok(String::new())
        }
        async fn refresh_secrets(
            &self,
            _id: &ContainerId,
            _env: HashMap<String, String>,
        ) -> Result<()> {
            self.refreshed.store(true, Ordering::SeqCst);
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        );
        rt.refresh_secrets(&ContainerId::new("agent"), HashMap::new())
            .await
            .expect("refresh_secrets should succeed");
        assert!(
            flag.load(Ordering::SeqCst),
            "InstrumentedRuntime must forward refresh_secrets to the inner runtime (RUN-1)"
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
                ..Default::default()
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

    /// Build an `InstrumentedRuntime` wrapping a mock with the given status behavior.
    fn rt_with_status(
        status: StatusBehavior,
    ) -> InstrumentedRuntime<RecordingRuntime, NoopInjector> {
        InstrumentedRuntime::new(
            RecordingRuntime {
                status,
                ..Default::default()
            },
            NoopInjector,
            "http://collector:4318".to_string(),
            "http/protobuf".to_string(),
            false,
            None,
        )
    }

    /// Success path: `status()` reaches the inner runtime and its result is returned
    /// unchanged. The decorator instruments `deploy()` only, so it must not reshape
    /// status data on the way back out.
    #[tokio::test]
    async fn status_forwards_inner_success_unchanged() {
        let rt = rt_with_status(StatusBehavior::State(RuntimeState::Running));
        let got = rt
            .status(&ContainerId::new("agent"))
            .await
            .expect("status should succeed");

        assert_eq!(got.container_id, ContainerId::new("agent"));
        assert_eq!(got.state, RuntimeState::Running);
        assert_eq!(got.replicas_live, 1);
        assert_eq!(got.restart_count, 0);
    }

    /// A non-running state is reported as-is, with no live replicas. Guards against a
    /// mock that hard-codes `Running` and so cannot exercise unhealthy-agent branches.
    #[tokio::test]
    async fn status_reports_non_running_states_with_no_live_replicas() {
        for state in [
            RuntimeState::Pending,
            RuntimeState::Crashed,
            RuntimeState::Failed,
            RuntimeState::Stopped,
        ] {
            let got = rt_with_status(StatusBehavior::State(state))
                .status(&ContainerId::new("agent"))
                .await
                .expect("status should succeed");
            assert_eq!(got.state, state, "state must round-trip unchanged");
            assert_eq!(got.replicas_live, 0, "{state} must report no live replicas");
        }
    }

    /// Failure path: a missing container surfaces as `ContainerNotFound`, not a panic.
    /// This is the case the previous `unimplemented!()` made untestable.
    #[tokio::test]
    async fn status_propagates_container_not_found() {
        let err = rt_with_status(StatusBehavior::NotFound)
            .status(&ContainerId::new("ghost"))
            .await
            .expect_err("missing container must be an error");

        match err {
            RuntimeError::ContainerNotFound(id) => assert_eq!(id, ContainerId::new("ghost")),
            other => panic!("expected ContainerNotFound, got {other:?}"),
        }
    }

    /// Failure path: a backend fault propagates with its message intact, so callers
    /// can log or surface the underlying reason.
    #[tokio::test]
    async fn status_propagates_backend_error_message() {
        let err = rt_with_status(StatusBehavior::Internal("daemon unreachable".to_string()))
            .status(&ContainerId::new("agent"))
            .await
            .expect_err("backend fault must be an error");

        match err {
            RuntimeError::Internal(msg) => assert_eq!(msg, "daemon unreachable"),
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
