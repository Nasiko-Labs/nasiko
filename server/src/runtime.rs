use nasiko_config::Config;
use nasiko_observability::{InstrumentedRuntime, OtelInjector};
use nasiko_runtime::{DockerRuntime, DockerRuntimeConfig, Result};

/// Build a `DockerRuntime` wrapped with `InstrumentedRuntime` so that every
/// `deploy()` call automatically injects the 7 `OTEL_*` environment variables
/// defined in `InstrumentationInjector`.
///
/// The returned runtime implements `ContainerRuntime` transparently — callers
/// never import bollard or observability directly.
pub async fn build_docker_runtime(
    config: &Config,
) -> Result<InstrumentedRuntime<DockerRuntime, OtelInjector>> {
    let docker = DockerRuntime::new(DockerRuntimeConfig::default()).await?;
    Ok(InstrumentedRuntime::new(
        docker,
        OtelInjector,
        config.otel_collector_endpoint.clone(),
        config.otel_capture_content,
    ))
}