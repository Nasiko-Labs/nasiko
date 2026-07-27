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
    // Loud warning for a silent footgun: with no public gateway URL configured,
    // McpInjector is a no-op, so every deployed agent silently gets zero MCP
    // tools. Better to surface it once at startup than debug empty tool lists.
    if config.mcp_gateway_public_url.is_none() {
        tracing::warn!(
            "MCP_GATEWAY_PUBLIC_URL is unset — deployed agents will NOT receive MCP_GATEWAY_URL \
             and cannot reach the MCP gateway (tools will be silently unavailable). Set it to the \
             server's externally-reachable /api/mcp URL to enable agent tool access."
        );
    }

    let otel_instrumented = InstrumentedRuntime::new(
        base,
        OtelInjector,
        config.otel_collector_endpoint.clone(),
        config.otel_protocol.clone(),
        config.otel_capture_content,
    );
    InstrumentedRuntime::new(
        otel_instrumented,
        McpInjector {
            gateway_public_url: config.mcp_gateway_public_url.clone(),
        },
        config.otel_collector_endpoint.clone(),
        config.otel_protocol.clone(),
        config.otel_capture_content,
    )
}

/// Build a `DockerRuntime` wrapped with the standard instrumentation stack (see
/// [`instrument`]). The returned runtime implements `ContainerRuntime`
/// transparently — callers never import bollard, observability, or mcp-gateway.
///
/// The embedded OCI registry is wired in as the runtime's `ImageSource`: an
/// image the daemon doesn't have locally is `docker load`ed straight from
/// registry storage, so `nasiko deploy` works against a single-node server
/// with no `OCI_REGISTRY_HOST` (which stays supported as the pull fallback).
pub async fn build_docker_runtime(
    config: &Config,
    db: sqlx::PgPool,
) -> Result<InstrumentedRuntime<InstrumentedRuntime<DockerRuntime, OtelInjector>, McpInjector>> {
    let storage = nasiko_oci::storage::S3Storage::from_env(config.oci_storage_bucket.clone()).await;
    let image_source = std::sync::Arc::new(nasiko_oci::OciState::new(db, storage));

    let docker = DockerRuntime::new(DockerRuntimeConfig {
        network: config.docker_agent_network.clone(),
        registry_host: config.oci_registry_host.clone(),
        ..DockerRuntimeConfig::default()
    })
    .await?
    .with_image_source(image_source);
    Ok(instrument(docker, config))
}
