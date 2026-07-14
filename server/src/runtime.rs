use nasiko_config::Config;
use nasiko_mcp_gateway::McpInjector;
use nasiko_observability::{InstrumentedRuntime, OtelInjector};
use nasiko_runtime::{ContainerRuntime, DockerRuntime, DockerRuntimeConfig, Result};

/// Wrap any base `ContainerRuntime` in the platform's standard deploy-time
/// instrumentation stack: two nested `InstrumentedRuntime` layers so every
/// `deploy()` injects both the 7 `OTEL_*` env vars AND `MCP_GATEWAY_URL` (when
/// configured). `InstrumentedRuntime<R, I>` composes over one injector at a
/// time, so a second env var set means a second wrapping layer, not a field.
///
/// Generic over the base runtime so OSS (`DockerRuntime`) and EE (`KubeRuntime`)
/// share ONE definition of the injector stack — add/change an injector here and
/// both editions pick it up, instead of the two paths drifting.
pub fn instrument<R: ContainerRuntime>(
    base: R,
    config: &Config,
) -> InstrumentedRuntime<InstrumentedRuntime<R, OtelInjector>, McpInjector> {
    let otel_instrumented = InstrumentedRuntime::new(
        base,
        OtelInjector,
        config.otel_collector_endpoint.clone(),
        config.otel_protocol.clone(),
        config.otel_capture_content,
    );
    InstrumentedRuntime::new(
        otel_instrumented,
        McpInjector { gateway_public_url: config.mcp_gateway_public_url.clone() },
        config.otel_collector_endpoint.clone(),
        config.otel_protocol.clone(),
        config.otel_capture_content,
    )
}

/// Build a `DockerRuntime` wrapped with the standard instrumentation stack (see
/// [`instrument`]). The returned runtime implements `ContainerRuntime`
/// transparently — callers never import bollard, observability, or mcp-gateway.
pub async fn build_docker_runtime(
    config: &Config,
) -> Result<InstrumentedRuntime<InstrumentedRuntime<DockerRuntime, OtelInjector>, McpInjector>> {
    let docker = DockerRuntime::new(DockerRuntimeConfig {
        network: config.docker_agent_network.clone(),
        registry_host: config.oci_registry_host.clone(),
        ..DockerRuntimeConfig::default()
    }).await?;
    Ok(instrument(docker, config))
}