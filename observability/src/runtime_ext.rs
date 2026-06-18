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
            agent_id: spec.container_id.to_string(),
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
}