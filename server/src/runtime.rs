use nasiko_config::Config;
use nasiko_mcp_gateway::McpInjector;
use nasiko_observability::{InstrumentedRuntime, OtelInjector};
use nasiko_runtime::{DockerRuntime, DockerRuntimeConfig, Result};

/// Build a `DockerRuntime` wrapped with two nested `InstrumentedRuntime` layers
/// so every `deploy()` call automatically injects both the 7 `OTEL_*`
/// environment variables AND `MCP_GATEWAY_URL` (when configured) —
/// `InstrumentedRuntime<R, I>` composes over one injector at a time, so a
/// second env var set means a second wrapping layer, not a second field.
///
/// The returned runtime implements `ContainerRuntime` transparently — callers
/// never import bollard, observability, or mcp-gateway directly.
pub async fn build_docker_runtime(
    config: &Config,
) -> Result<InstrumentedRuntime<InstrumentedRuntime<DockerRuntime, OtelInjector>, McpInjector>> {
    let docker = DockerRuntime::new(DockerRuntimeConfig {
        network: config.docker_agent_network.clone(),
        registry_host: config.oci_registry_host.clone(),
        ..DockerRuntimeConfig::default()
    }).await?;
    let otel_instrumented = InstrumentedRuntime::new(
        docker,
        OtelInjector,
        config.otel_collector_endpoint.clone(),
        config.otel_capture_content,
    );
    Ok(InstrumentedRuntime::new(
        otel_instrumented,
        McpInjector { gateway_public_url: config.mcp_gateway_public_url.clone() },
        config.otel_collector_endpoint.clone(),
        config.otel_capture_content,
    ))
}