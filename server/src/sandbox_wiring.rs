//! Selecting the [`SandboxProvider`] a binary runs harness sessions on.
//!
//! The provider is an edition decision, received by `AppState` like the container runtime.
//! This module is the OSS edition's choice: Docker when it can be had, otherwise a provider
//! that refuses every call and names what was asked for, so a misconfiguration fails at the
//! first session rather than at boot or compile time.

use std::sync::Arc;

use nasiko_config::{Config, SandboxProviderSetting};
use nasiko_mcp_gateway::McpInjector;
use nasiko_observability::InstrumentedSandbox;
use nasiko_runtime::{
    DockerSandboxProvider, ImageSource, SandboxProvider, SandboxProviderKind,
    UnavailableSandboxProvider, connect_docker_sandbox_provider,
};

/// The provider kind an operator setting names.
pub fn kind_of(setting: SandboxProviderSetting) -> SandboxProviderKind {
    match setting {
        SandboxProviderSetting::Docker => SandboxProviderKind::Docker,
        SandboxProviderSetting::Pod => SandboxProviderKind::Pod,
        SandboxProviderSetting::SharedPod => SandboxProviderKind::SharedPod,
        SandboxProviderSetting::HarnessAgent => SandboxProviderKind::HarnessAgent,
    }
}

/// Connect the Docker sandbox provider when `SANDBOX_PROVIDER=docker`, sharing the Docker
/// settings and image source of the deployment runtime. `None` when another provider is
/// configured or the daemon cannot be reached (logged).
pub async fn connect_docker_sandbox(
    config: &Config,
    image_source: Option<Arc<dyn ImageSource>>,
) -> Option<DockerSandboxProvider> {
    if config.sandbox_provider != SandboxProviderSetting::Docker {
        return None;
    }
    match connect_docker_sandbox_provider(
        crate::runtime::docker_runtime_config(config),
        image_source,
    )
    .await
    {
        Ok(provider) => Some(provider),
        Err(e) => {
            tracing::warn!(error = %e, "docker sandbox provider unavailable");
            None
        }
    }
}

/// The provider for `config`: the connected Docker one when configured and supplied, else
/// [`UnavailableSandboxProvider`] for the configured kind (`pod`, `shared-pod` and
/// `harness-agent` are linked by the enterprise binary, not this edition).
pub fn build_sandbox_provider(
    config: &Config,
    docker: Option<DockerSandboxProvider>,
) -> Arc<dyn SandboxProvider> {
    let base: Arc<dyn SandboxProvider> = match (config.sandbox_provider, docker) {
        (SandboxProviderSetting::Docker, Some(docker)) => Arc::new(docker),
        (setting, _) => Arc::new(UnavailableSandboxProvider::new(kind_of(setting))),
    };
    let provider = instrument_sandbox(base, config);
    tracing::info!(provider = %provider.kind(), "sandbox provider: {}", provider.kind());
    provider
}

/// Wrap `provider` so every sandbox it acquires gets the platform's OTEL env (and, for agent
/// sandboxes, `MCP_GATEWAY_URL`) — the sandbox counterpart of [`crate::runtime::instrument`].
/// Shared by both editions so their provider stacks cannot drift.
pub fn instrument_sandbox(
    provider: Arc<dyn SandboxProvider>,
    config: &Config,
) -> Arc<dyn SandboxProvider> {
    Arc::new(
        InstrumentedSandbox::new(
            provider,
            config.otel_collector_endpoint.clone(),
            config.otel_protocol.clone(),
            config.otel_capture_content,
            config.tenant_id.clone(),
        )
        .with_agent_injector(McpInjector {
            gateway_public_url: config.mcp_gateway_public_url.clone(),
        }),
    )
}

/// The env [`instrument_sandbox`]'s decorator adds to a harness session's sandbox, for a caller
/// that delivers the session's settings other than through the spec (a handshake-mode shim).
pub fn session_env_additions(
    config: &Config,
    key: &nasiko_runtime::SandboxKey,
) -> std::collections::HashMap<String, String> {
    nasiko_observability::sandbox_env_additions(
        key,
        &config.otel_collector_endpoint,
        &config.otel_protocol,
        config.otel_capture_content,
        config.tenant_id.as_deref(),
    )
}
