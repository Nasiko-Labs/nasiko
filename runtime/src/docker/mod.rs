use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::BuildImageOptions;
use bollard::models::{ContainerStateStatusEnum, HostConfig, PortBinding};
use bollard::container::LogsOptions;
use bollard::Docker;
use futures_util::StreamExt;
use tracing::{instrument, warn};

use crate::{
    error::{Result, RuntimeError},
    types::{validate_build_inputs, ContainerId, DeploymentSpec, DeploymentStatus, RuntimeState},
    ContainerRuntime,
};

// ── Config ─────────────────────────────────────────────────────────────────────

/// Configuration for the Docker runtime backend.
#[derive(Debug, Clone)]
pub struct DockerRuntimeConfig {
    /// IP address to bind container ports to.
    /// Default: `"127.0.0.1"` (loopback only). Use `"0.0.0.0"` for external access.
    pub bind_host: String,
    /// Per-operation timeout for Docker API calls (create, start, stop, inspect, logs).
    /// Default: 30 seconds.
    pub operation_timeout: Duration,
    /// Timeout for the entire image build stream. Docker builds can take minutes.
    /// This is intentionally separate from `operation_timeout` — never set them to the same value.
    /// Default: 30 minutes.
    pub build_timeout: Duration,
}

impl Default for DockerRuntimeConfig {
    fn default() -> Self {
        DockerRuntimeConfig {
            bind_host: "127.0.0.1".to_owned(),
            operation_timeout: Duration::from_secs(30),
            build_timeout: Duration::from_secs(30 * 60),
        }
    }
}

// ── Public struct ──────────────────────────────────────────────────────────────

/// Docker-based agent runtime for local development.
///
/// Each agent maps to a single Docker container named `nasiko-agent-{container_id}`.
/// Names are deterministic — no fuzzy matching, no prefix heuristics.
///
/// # Limitations
/// - Replicas > 1 are clamped to 1 (Docker has no native multi-replica concept).
/// - Host ports are ephemeral: Docker assigns a free port at container start time.
///   Call [`endpoint`](ContainerRuntime::endpoint) after deploy to discover the bound port.
/// - Concurrent deploys of the same `container_id` may race; the caller is responsible
///   for serialising concurrent operations on the same agent.
pub struct DockerRuntime {
    client: Docker,
    config: DockerRuntimeConfig,
}

impl std::fmt::Debug for DockerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerRuntime").finish_non_exhaustive()
    }
}

// ── Constructor & helpers ──────────────────────────────────────────────────────

impl DockerRuntime {
    /// Connect to the local Docker daemon using the platform default
    /// (Unix socket on Linux/macOS, named pipe on Windows). Pings the daemon
    /// with up to 3 attempts (100ms / 500ms backoff) before returning.
    pub async fn new(config: DockerRuntimeConfig) -> Result<Self> {
        let client = Docker::connect_with_local_defaults()
            .map_err(|e| RuntimeError::BackendUnreachable(e.to_string()))?;

        let mut last_err = String::new();
        for attempt in 0u32..3 {
            match client.ping().await {
                Ok(_) => return Ok(DockerRuntime { client, config }),
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < 2 {
                        let delay = if attempt == 0 { 100 } else { 500 };
                        warn!(attempt, "Docker ping failed, retrying");
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        Err(RuntimeError::BackendUnreachable(last_err))
    }

    /// Deterministic container name for an agent: `nasiko-agent-{container_id}`.
    fn container_name(id: &ContainerId) -> String {
        format!("nasiko-agent-{}", id.as_str())
    }

    /// Extract the agent ID from a container name, stripping the leading `/` that
    /// Docker adds and the `nasiko-agent-` prefix.
    fn container_id_from_name(name: &str) -> Option<ContainerId> {
        // Docker names arrive as "/nasiko-agent-{id}" from the list API
        let stripped = name.strip_prefix('/').unwrap_or(name);
        stripped
            .strip_prefix("nasiko-agent-")
            .map(ContainerId::new)
    }
}

// ── Error mapping ──────────────────────────────────────────────────────────────

fn is_not_found(err: &bollard::errors::Error) -> bool {
    matches!(
        err,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

/// Returns true for HTTP 304 (Not Modified) — e.g., stop on an already-stopped
/// container or start on an already-running container.
fn is_not_modified(err: &bollard::errors::Error) -> bool {
    matches!(
        err,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 304,
            ..
        }
    )
}

fn map_bollard_err(err: bollard::errors::Error) -> RuntimeError {
    warn!(error = %err, "bollard API error");
    match &err {
        bollard::errors::Error::IOError { .. }
        | bollard::errors::Error::HyperResponseError { .. } => {
            RuntimeError::BackendUnreachable(err.to_string())
        }
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            message,
        } if message.contains("No such image") => RuntimeError::ImageNotFound(message.clone()),
        bollard::errors::Error::DockerResponseServerError {
            status_code: 409,
            message,
        } => RuntimeError::ResourceConflict(message.clone()),
        _ => RuntimeError::Internal(err.to_string()),
    }
}

// ── State mapping ──────────────────────────────────────────────────────────────

/// Map a Docker container state to [`RuntimeState`].
///
/// For EXITED containers, a zero exit code means intentionally stopped;
/// a non-zero exit code means the process crashed.
fn map_container_state(
    status: Option<ContainerStateStatusEnum>,
    exit_code: Option<i64>,
) -> RuntimeState {
    match status {
        Some(ContainerStateStatusEnum::RUNNING) => RuntimeState::Running,
        Some(ContainerStateStatusEnum::CREATED) | Some(ContainerStateStatusEnum::RESTARTING) => {
            RuntimeState::Pending
        }
        Some(ContainerStateStatusEnum::PAUSED) | Some(ContainerStateStatusEnum::REMOVING) => {
            RuntimeState::Stopped
        }
        Some(ContainerStateStatusEnum::EXITED) => match exit_code {
            Some(0) | None => RuntimeState::Stopped,
            _ => RuntimeState::Crashed,
        },
        Some(ContainerStateStatusEnum::DEAD) => RuntimeState::Crashed,
        _ => RuntimeState::Unknown,
    }
}

/// Map the plain `state` string from `ContainerSummary` (list API) to [`RuntimeState`].
/// No exit code is available in list responses — EXITED is mapped to Stopped conservatively.
/// Use [`status`](ContainerRuntime::status) for accurate state when exit code matters.
fn map_summary_state(state: Option<&str>) -> RuntimeState {
    match state {
        Some("running") => RuntimeState::Running,
        Some("created") | Some("restarting") => RuntimeState::Pending,
        Some("exited") | Some("paused") | Some("removing") => RuntimeState::Stopped,
        Some("dead") => RuntimeState::Crashed,
        _ => RuntimeState::Unknown,
    }
}

// ── Port helpers ───────────────────────────────────────────────────────────────

type PortBindingsMap = HashMap<String, Option<Vec<PortBinding>>>;
type ExposedPortsMap = HashMap<String, HashMap<(), ()>>;

/// Build port bindings and exposed-ports maps from a port list.
/// Host port is left empty so Docker assigns an ephemeral port.
/// `bind_host` controls which interface ports are bound to (default: `"127.0.0.1"`).
fn build_port_config(ports: &[u16], bind_host: &str) -> (PortBindingsMap, ExposedPortsMap) {
    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();

    for &port in ports {
        let key = format!("{port}/tcp");
        exposed_ports.insert(key.clone(), HashMap::new());
        port_bindings.insert(
            key,
            Some(vec![PortBinding {
                host_ip: Some(bind_host.to_owned()),
                // Empty string → Docker picks an available ephemeral port
                host_port: Some(String::new()),
            }]),
        );
    }

    (port_bindings, exposed_ports)
}

/// Extract the first bound host port from a `NetworkSettings.Ports` map.
/// Ports are sorted numerically so the lowest container port is preferred.
/// Returns `None` if no bindings are present.
fn extract_host_port(
    ports: &HashMap<String, Option<Vec<PortBinding>>>,
) -> Option<String> {
    let mut keys: Vec<&String> = ports.keys().collect();
    // Numeric sort: "10000/tcp" < "9000/tcp" lexicographically but 9000 < 10000 numerically
    keys.sort_by_key(|k| {
        k.split('/').next().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0)
    });

    for key in keys {
        if let Some(Some(bindings)) = ports.get(key)
            && let Some(binding) = bindings.first()
            && let Some(hp) = &binding.host_port
            && !hp.is_empty()
        {
            return Some(hp.clone());
        }
    }
    None
}

// ── Resource helpers ───────────────────────────────────────────────────────────

/// Parse a memory string (`"512Mi"`, `"1Gi"`, or bare bytes) into bytes.
fn parse_memory_bytes(s: &str) -> i64 {
    let msg = "parse_memory_bytes called with unvalidated input";
    let overflow = "memory value overflows i64 — validate() should have rejected this";
    if let Some(n) = s.strip_suffix("Gi") {
        n.parse::<i64>().expect(msg).checked_mul(1024 * 1024 * 1024).expect(overflow)
    } else if let Some(n) = s.strip_suffix("Mi") {
        n.parse::<i64>().expect(msg).checked_mul(1024 * 1024).expect(overflow)
    } else if let Some(n) = s.strip_suffix("G") {
        n.parse::<i64>().expect(msg).checked_mul(1_000_000_000).expect(overflow)
    } else if let Some(n) = s.strip_suffix("M") {
        n.parse::<i64>().expect(msg).checked_mul(1_000_000).expect(overflow)
    } else {
        s.parse::<i64>().expect(msg)
    }
}

// ── Deploy helpers ─────────────────────────────────────────────────────────────

/// Create and start a container from a `DeploymentSpec`.
async fn create_and_start(
    client: &Docker,
    spec: &DeploymentSpec,
    bind_host: &str,
    timeout: Duration,
) -> Result<()> {
    let name = DockerRuntime::container_name(&spec.container_id);

    let env_vec: Vec<String> = spec
        .env_vars
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let (port_bindings, exposed_ports) = build_port_config(&spec.ports, bind_host);

    let lim = spec.resources.as_ref().cloned().unwrap_or_default();
    let host_config = HostConfig {
        port_bindings: Some(port_bindings),
        memory: Some(parse_memory_bytes(&lim.memory)),
        nano_cpus: Some(lim.cpu_milli as i64 * 1_000_000),
        ..Default::default()
    };

    let config = Config {
        image: Some(spec.image.clone()),
        env: Some(env_vec),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        ..Default::default()
    };

    let options = CreateContainerOptions {
        name: name.as_str(),
        platform: None,
    };

    tokio::time::timeout(timeout, client.create_container(Some(options), config))
        .await
        .map_err(|_| RuntimeError::Timeout("create_container".to_owned()))?
        .map_err(map_bollard_err)?;

    tokio::time::timeout(
        timeout,
        client.start_container(&name, None::<StartContainerOptions<String>>),
    )
    .await
    .map_err(|_| RuntimeError::Timeout("start_container".to_owned()))?
    .map_err(map_bollard_err)?;

    Ok(())
}

/// Inspect a container and build a `DeploymentStatus`. Returns `Unknown` status
/// if the container does not exist (not an error).
async fn inspect_to_status(
    client: &Docker,
    container_id: &ContainerId,
    timeout: Duration,
) -> Result<DeploymentStatus> {
    let name = DockerRuntime::container_name(container_id);

    let info = match tokio::time::timeout(
        timeout,
        client.inspect_container(&name, None::<InspectContainerOptions>),
    )
    .await
    {
        Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
        Ok(Err(ref e)) if is_not_found(e) => {
            return Ok(DeploymentStatus {
                container_id: container_id.clone(),
                state: RuntimeState::Unknown,
                replicas_live: 0,
                endpoint: None,
                message: None,
                restart_count: 0,
            });
        }
        Ok(Err(e)) => return Err(map_bollard_err(e)),
        Ok(Ok(info)) => info,
    };

    let container_state = info.state.as_ref();
    let status_enum = container_state.and_then(|s| s.status);
    let exit_code = container_state.and_then(|s| s.exit_code);
    let error_msg = container_state
        .and_then(|s| s.error.clone())
        .filter(|s| !s.is_empty());

    let state = map_container_state(status_enum, exit_code);
    let replicas_live = if state == RuntimeState::Running { 1 } else { 0 };

    let endpoint = if state == RuntimeState::Running {
        info.network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .and_then(extract_host_port)
            .map(|hp| format!("http://localhost:{hp}"))
    } else {
        None
    };

    Ok(DeploymentStatus {
        container_id: container_id.clone(),
        state,
        replicas_live,
        endpoint,
        message: error_msg,
        restart_count: 0,
    })
}

// ── ContainerRuntime impl ──────────────────────────────────────────────────────────

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    #[instrument(skip(self, spec), fields(container_id = %spec.container_id))]
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<DeploymentStatus> {
        spec.validate()?;

        let name = DockerRuntime::container_name(&spec.container_id);
        let timeout = self.config.operation_timeout;

        match tokio::time::timeout(
            timeout,
            self.client.inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        {
            Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
            Ok(Err(ref e)) if is_not_found(e) => {
                // Container does not exist: create and start it
                create_and_start(&self.client, spec, &self.config.bind_host, timeout).await?;
            }
            Ok(Err(e)) => return Err(map_bollard_err(e)),
            Ok(Ok(existing)) => {
                let existing_image = existing
                    .config
                    .as_ref()
                    .and_then(|c| c.image.as_deref())
                    .unwrap_or("");

                if existing_image == spec.image {
                    // Same image: ensure the container is running (idempotent)
                    let current_status = existing
                        .state
                        .as_ref()
                        .and_then(|s| s.status);

                    if current_status != Some(ContainerStateStatusEnum::RUNNING) {
                        tokio::time::timeout(
                            timeout,
                            self.client.start_container(&name, None::<StartContainerOptions<String>>),
                        )
                        .await
                        .map_err(|_| RuntimeError::Timeout("start_container".to_owned()))?
                        .or_else(|e| if is_not_modified(&e) { Ok(()) } else { Err(map_bollard_err(e)) })?;
                    }
                } else {
                    // Different image: stop → remove → recreate
                    tokio::time::timeout(
                        timeout,
                        self.client.stop_container(&name, None::<StopContainerOptions>),
                    )
                    .await
                    .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
                    .or_else(|e| if is_not_modified(&e) { Ok(()) } else { Err(map_bollard_err(e)) })?;

                    tokio::time::timeout(
                        timeout,
                        self.client.remove_container(
                            &name,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        ),
                    )
                    .await
                    .map_err(|_| RuntimeError::Timeout("remove_container".to_owned()))?
                    .map_err(map_bollard_err)?;

                    create_and_start(&self.client, spec, &self.config.bind_host, timeout).await?;
                }
            }
        }

        inspect_to_status(&self.client, &spec.container_id, timeout).await
    }

    #[instrument(skip(self))]
    async fn destroy(&self, container_id: &ContainerId) -> Result<()> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        match tokio::time::timeout(
            timeout,
            self.client.inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        {
            Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
            Ok(Err(ref e)) if is_not_found(e) => return Ok(()), // idempotent
            Ok(Err(e)) => return Err(map_bollard_err(e)),
            Ok(Ok(_)) => {}
        }

        // Stop first (ignore 304 = already stopped)
        tokio::time::timeout(
            timeout,
            self.client.stop_container(&name, None::<StopContainerOptions>),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
        .or_else(|e| if is_not_modified(&e) { Ok(()) } else { Err(map_bollard_err(e)) })?;

        tokio::time::timeout(
            timeout,
            self.client.remove_container(
                &name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            ),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("remove_container".to_owned()))?
        .map_err(map_bollard_err)?;

        Ok(())
    }

    /// Set the replica count.
    ///
    /// Docker has no native multi-replica concept. `replicas > 1` is logged as a
    /// warning and treated as 1. `replicas == 0` stops the container.
    #[instrument(skip(self))]
    async fn scale(&self, container_id: &ContainerId, replicas: u32) -> Result<()> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        // Verify the container exists
        match tokio::time::timeout(
            timeout,
            self.client.inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        {
            Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
            Ok(Err(ref e)) if is_not_found(e) => {
                return Err(RuntimeError::ContainerNotFound(container_id.clone()))
            }
            Ok(Err(e)) => return Err(map_bollard_err(e)),
            Ok(Ok(_)) => {}
        }

        if replicas == 0 {
            tokio::time::timeout(
                timeout,
                self.client.stop_container(&name, None::<StopContainerOptions>),
            )
            .await
            .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
            .or_else(|e| if is_not_modified(&e) { Ok(()) } else { Err(map_bollard_err(e)) })?;
        } else {
            if replicas > 1 {
                // Docker supports only a single container per name. Scaling to N > 1
                // requires an orchestrator (e.g. Docker Swarm or Kubernetes).
                // We clamp to 1 and emit a warning so the caller is aware.
                warn!(
                    container_id = %container_id,
                    requested_replicas = replicas,
                    "Docker backend does not support replicas > 1; clamping to 1"
                );
            }
            tokio::time::timeout(
                timeout,
                self.client.start_container(&name, None::<StartContainerOptions<String>>),
            )
            .await
            .map_err(|_| RuntimeError::Timeout("start_container".to_owned()))?
            .or_else(|e| if is_not_modified(&e) { Ok(()) } else { Err(map_bollard_err(e)) })?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus> {
        container_id.validate()?;
        inspect_to_status(&self.client, container_id, self.config.operation_timeout).await
    }

    #[instrument(skip(self))]
    async fn list(&self) -> Result<Vec<DeploymentStatus>> {
        let timeout = self.config.operation_timeout;
        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        // Anchor with ^/ to avoid substring matches (e.g. old-nasiko-agent-foo)
        filters.insert("name".to_owned(), vec!["^/nasiko-agent-".to_owned()]);

        let options = ListContainersOptions::<String> {
            all: true,
            filters,
            ..Default::default()
        };

        let containers = tokio::time::timeout(timeout, self.client.list_containers(Some(options)))
            .await
            .map_err(|_| RuntimeError::Timeout("list_containers".to_owned()))?
            .map_err(map_bollard_err)?;

        let mut statuses = Vec::with_capacity(containers.len());

        for container in containers {
            let name = container
                .names
                .as_deref()
                .and_then(|names| names.first())
                .map(String::as_str)
                .unwrap_or("");

            let Some(container_id) = DockerRuntime::container_id_from_name(name) else {
                continue;
            };

            let state = map_summary_state(container.state.as_deref());
            let replicas_live = if state == RuntimeState::Running { 1 } else { 0 };

            statuses.push(DeploymentStatus {
                container_id,
                state,
                replicas_live,
                endpoint: None, // List does not provide port bindings; use endpoint() or status()
                message: None,
                restart_count: 0,
            });
        }

        Ok(statuses)
    }

    #[instrument(skip(self))]
    async fn endpoint(&self, container_id: &ContainerId) -> Result<String> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        let info = tokio::time::timeout(
            timeout,
            self.client.inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("inspect_container".to_owned()))?
        .map_err(|e| {
            if is_not_found(&e) {
                RuntimeError::ContainerNotFound(container_id.clone())
            } else {
                map_bollard_err(e)
            }
        })?;

        let host_port = info
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .and_then(extract_host_port)
            .ok_or_else(|| {
                RuntimeError::Internal(format!(
                    "container {name} has no bound host port"
                ))
            })?;

        Ok(format!("http://localhost:{host_port}"))
    }

    #[instrument(skip(self))]
    async fn logs(&self, container_id: &ContainerId, tail: u32) -> Result<Vec<String>> {
        container_id.validate()?;
        let tail = tail.min(10_000);
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        let options = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            ..Default::default()
        };

        let stream_fut = async {
            let mut stream = self.client.logs(&name, Some(options));
            let mut lines = Vec::new();

            while let Some(chunk) = stream.next().await {
                let output = chunk.map_err(|e| {
                    if is_not_found(&e) {
                        RuntimeError::ContainerNotFound(container_id.clone())
                    } else {
                        map_bollard_err(e)
                    }
                })?;
                for line in output.to_string().lines() {
                    lines.push(line.to_owned());
                }
            }

            Ok::<Vec<String>, RuntimeError>(lines)
        };

        tokio::time::timeout(timeout, stream_fut)
            .await
            .map_err(|_| RuntimeError::Timeout("logs stream".to_owned()))?
    }

    #[instrument(skip(self, tar_context), fields(image_tag))]
    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<String> {
        validate_build_inputs(tar_context, image_tag)?;

        let options = BuildImageOptions {
            t: image_tag.to_owned(),
            rm: true,      // remove intermediate containers on success
            forcerm: true, // remove intermediate containers even on failure
            ..Default::default()
        };

        let body = tar_context.to_vec().into();

        let build_fut = async {
            let mut stream = self.client.build_image(options, None, Some(body));
            while let Some(msg) = stream.next().await {
                let info = msg.map_err(map_bollard_err)?;
                if let Some(err) = info.error {
                    let detail = info
                        .error_detail
                        .and_then(|d| d.message)
                        .unwrap_or_default();
                    let full = if detail.is_empty() {
                        err
                    } else {
                        format!("{err}: {detail}")
                    };
                    return if full.contains("pull access denied")
                        || full.contains("not found")
                        || full.contains("does not exist")
                    {
                        Err(RuntimeError::ImageNotFound(full))
                    } else {
                        Err(RuntimeError::Internal(full))
                    };
                }
                if let Some(line) = info.stream {
                    tracing::debug!(target: "agent_runtime::build", "{}", line.trim_end());
                }
            }
            Ok(image_tag.to_owned())
        };

        tokio::time::timeout(self.config.build_timeout, build_fut)
            .await
            .map_err(|_| RuntimeError::Timeout("image build".to_owned()))?
    }
}

