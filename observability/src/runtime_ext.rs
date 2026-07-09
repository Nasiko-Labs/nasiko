use async_trait::async_trait;
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, Result};

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
///     "http://otel-collector.nasiko-infra:4318".into(),
///     false,
/// );
/// ```
pub struct InstrumentedRuntime<R, I> {
    inner: R,
    injector: I,
    otel_collector_endpoint: String,
    capture_content: bool,
}

impl<R: ContainerRuntime, I: InstrumentationInjector> InstrumentedRuntime<R, I> {
    pub fn new(
        inner: R,
        injector: I,
        otel_collector_endpoint: String,
        capture_content: bool,
    ) -> Self {
        Self {
            inner,
            injector,
            otel_collector_endpoint,
            capture_content,
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
            tenant_id: None,
            version: None,
            capture_content: self.capture_content,
            otel_collector_endpoint: self.otel_collector_endpoint.clone(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injector::AgentContext;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Inner runtime that records whether `refresh_secrets` reached it.
    struct RecordingRuntime {
        refreshed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ContainerRuntime for RecordingRuntime {
        async fn deploy(&self, _s: &DeploymentSpec) -> Result<DeploymentStatus> { unimplemented!() }
        async fn destroy(&self, _id: &ContainerId) -> Result<()> { Ok(()) }
        async fn scale(&self, _id: &ContainerId, _r: u32) -> Result<()> { Ok(()) }
        async fn restart(&self, _id: &ContainerId) -> Result<()> { Ok(()) }
        async fn status(&self, _id: &ContainerId) -> Result<DeploymentStatus> { unimplemented!() }
        async fn list(&self) -> Result<Vec<DeploymentStatus>> { Ok(vec![]) }
        async fn endpoint(&self, _id: &ContainerId) -> Result<String> { Ok(String::new()) }
        async fn logs(&self, _id: &ContainerId, _t: u32) -> Result<Vec<String>> { Ok(vec![]) }
        async fn build(&self, _c: &[u8], _t: &str) -> Result<String> { Ok(String::new()) }
        async fn refresh_secrets(
            &self,
            _id: &ContainerId,
            _env: HashMap<String, String>,
        ) -> Result<()> {
            self.refreshed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct NoopInjector;
    impl InstrumentationInjector for NoopInjector {
        fn inject(&self, _env: &mut HashMap<String, String>, _ctx: &AgentContext) {}
    }

    /// RUN-1 regression guard: the decorator must forward `refresh_secrets` to the
    /// inner runtime, not fall through to the trait's no-op default (which would
    /// silently drop a K8s secret rotation).
    #[tokio::test]
    async fn refresh_secrets_forwards_to_inner() {
        let flag = Arc::new(AtomicBool::new(false));
        let rt = InstrumentedRuntime::new(
            RecordingRuntime { refreshed: flag.clone() },
            NoopInjector,
            "http://collector:4318".to_string(),
            false,
        );
        rt.refresh_secrets(&ContainerId::new("agent"), HashMap::new())
            .await
            .expect("refresh_secrets should succeed");
        assert!(
            flag.load(Ordering::SeqCst),
            "InstrumentedRuntime must forward refresh_secrets to the inner runtime (RUN-1)"
        );
    }
}