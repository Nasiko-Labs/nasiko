mod stats;
pub use stats::DockerStatsProvider;

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogsOptions;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions,
    RemoveContainerOptions, RestartContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::{BuildImageOptions, CreateImageOptions, ImportImageOptions};
use bollard::models::{ContainerStateStatusEnum, HostConfig, PortBinding};
use bollard::network::ConnectNetworkOptions;
use futures_util::StreamExt;
use std::sync::Arc;
use tracing::{info, instrument, warn};

use crate::{
    ContainerRuntime, ImageSource,
    error::{Result, RuntimeError},
    types::{
        ContainerId, DeploymentSpec, DeploymentStatus, InstanceInfo, RuntimeState,
        validate_build_inputs,
    },
};

// ── Config ─────────────────────────────────────────────────────────────────────

/// Configuration for the Docker runtime backend.
#[derive(Debug, Clone)]
pub struct DockerRuntimeConfig {
    /// IP address to bind container ports to.
    /// Default: `"127.0.0.1"` (loopback only). Use `"0.0.0.0"` for external access.
    pub bind_host: String,
    /// Docker network to attach agent containers to after they start.
    /// When set, `endpoint()` returns the container's IP on this network + internal port,
    /// so the server can reach agents even when running inside Docker itself.
    /// Example: `"nasiko-cloud-rs_default"` when running via docker-compose.
    /// Default: `None` (use host-mapped port at `localhost`).
    pub network: Option<String>,
    /// Per-operation timeout for Docker API calls (create, start, stop, inspect, logs).
    /// Default: 30 seconds.
    pub operation_timeout: Duration,
    /// Timeout for the entire image build stream. Docker builds can take minutes.
    /// This is intentionally separate from `operation_timeout` — never set them to the same value.
    /// Default: 30 minutes.
    pub build_timeout: Duration,
    /// OCI registry host (e.g. `"localhost:8443"`) to pull images from before creating containers.
    /// When set, images that don't already include a registry host are pulled from here first.
    /// Default: `None` (use Docker's local cache / Docker Hub).
    pub registry_host: Option<String>,
}

impl Default for DockerRuntimeConfig {
    fn default() -> Self {
        DockerRuntimeConfig {
            bind_host: "127.0.0.1".to_owned(),
            network: None,
            operation_timeout: Duration::from_secs(30),
            build_timeout: Duration::from_secs(30 * 60),
            registry_host: None,
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
    image_source: Option<Arc<dyn ImageSource>>,
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
                Ok(_) => {
                    return Ok(DockerRuntime {
                        client,
                        config,
                        image_source: None,
                    });
                }
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

    /// Satisfy image-cache misses from `source` (via `docker load`) before
    /// falling back to a registry pull. Wired at the composition root; see
    /// [`ImageSource`].
    pub fn with_image_source(mut self, source: Arc<dyn ImageSource>) -> Self {
        self.image_source = Some(source);
        self
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
        stripped.strip_prefix("nasiko-agent-").map(ContainerId::new)
    }

    /// Idempotently ensures a bridge network named `name` exists. Called once at
    /// server startup for `MCP_SERVERS_NETWORK` — never per-deploy, to avoid a
    /// create/create race between concurrent deploys. Inspect-then-create rather
    /// than `check_duplicate` (Docker networks are keyed by random ID, not name,
    /// so `check_duplicate` cannot fully guarantee no duplicates are created).
    pub async fn ensure_network(&self, name: &str) -> Result<()> {
        match self
            .client
            .inspect_network(
                name,
                None::<bollard::network::InspectNetworkOptions<String>>,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(ref e) if is_not_found(e) => {
                self.client
                    .create_network(bollard::network::CreateNetworkOptions {
                        name: name.to_owned(),
                        driver: "bridge".to_owned(),
                        ..Default::default()
                    })
                    .await
                    .map_err(map_bollard_err)?;
                Ok(())
            }
            Err(e) => Err(map_bollard_err(e)),
        }
    }

    /// Connect an arbitrary container (by name or ID) to a Docker network.
    /// Used at startup to attach the server's own container to the MCP servers
    /// network when it runs inside Docker.
    pub async fn connect_container_to_network(
        &self,
        container: &str,
        network: &str,
    ) -> Result<()> {
        self.client
            .connect_network(
                network,
                bollard::network::ConnectNetworkOptions {
                    container,
                    ..Default::default()
                },
            )
            .await
            .map_err(map_bollard_err)
    }

    /// Names of every Docker network `container_id`'s container is currently
    /// attached to. Introspection helper (not part of `ContainerRuntime` — this
    /// is Docker-specific, used to verify network-segmentation guarantees in
    /// tests, e.g. that an uploaded MCP server's container is on
    /// `mcp_servers_network` only, never the default network agents/DB/Redis
    /// share).
    pub async fn container_networks(&self, container_id: &ContainerId) -> Result<Vec<String>> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let info = self
            .client
            .inspect_container(&name, None::<InspectContainerOptions>)
            .await
            .map_err(|e| {
                if is_not_found(&e) {
                    RuntimeError::ContainerNotFound(container_id.clone())
                } else {
                    map_bollard_err(e)
                }
            })?;
        Ok(info
            .network_settings
            .and_then(|ns| ns.networks)
            .map(|nets| nets.into_keys().collect())
            .unwrap_or_default())
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

/// Parse Docker's `State.StartedAt`. Returns `None` for the "never started"
/// sentinel (`"0001-01-01T00:00:00Z"`) or unparseable values.
fn parse_docker_started_at(raw: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::Datelike;

    let parsed = chrono::DateTime::parse_from_rfc3339(raw?).ok()?;
    // Docker reports year 1 for containers that never started; treat anything
    // implausibly old as the sentinel rather than a real start time.
    if parsed.year() < 2000 {
        return None;
    }
    Some(parsed.with_timezone(&chrono::Utc))
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

/// Builds the `HostConfig` for a container, applying OS-level hardening
/// (read-only rootfs, dropped capabilities, no-new-privileges) when
/// `spec.harden` is set — see `DeploymentSpec::harden`'s doc comment. Pure and
/// hermetically testable: no Docker client involved.
fn build_host_config(spec: &DeploymentSpec, port_bindings: PortBindingsMap) -> HostConfig {
    let lim = spec.resources.as_ref().cloned().unwrap_or_default();
    let base = HostConfig {
        port_bindings: Some(port_bindings),
        memory: Some(parse_memory_bytes(&lim.memory)),
        nano_cpus: Some(lim.cpu_milli as i64 * 1_000_000),
        // Without this, Docker creates every container on the default "bridge"
        // network first; `create_and_start`'s later `connect_network` call
        // only ever ADDS the target network, it never removes "bridge" — so a
        // network-segmented deploy (network_override) ended up dual-homed on
        // both the isolated network and the default one, defeating the whole
        // point of segmentation (RUN-11, found via a real isolation test, not
        // theoretical). Only `network_override` deploys are re-homed this way;
        // ordinary agent deploys (which never set it) are unaffected.
        network_mode: spec.network_override.clone(),
        ..Default::default()
    };
    if spec.harden {
        HostConfig {
            readonly_rootfs: Some(true),
            tmpfs: Some(HashMap::from([("/tmp".to_owned(), "size=64m".to_owned())])),
            cap_drop: Some(vec!["ALL".to_owned()]),
            security_opt: Some(vec!["no-new-privileges:true".to_owned()]),
            ..base
        }
    } else {
        base
    }
}

/// Extract the first bound host port from a `NetworkSettings.Ports` map.
/// Ports are sorted numerically so the lowest container port is preferred.
/// Returns `None` if no bindings are present.
fn extract_host_port(ports: &HashMap<String, Option<Vec<PortBinding>>>) -> Option<String> {
    let mut keys: Vec<&String> = ports.keys().collect();
    // Numeric sort: "10000/tcp" < "9000/tcp" lexicographically but 9000 < 10000 numerically
    keys.sort_by_key(|k| {
        k.split('/')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(0)
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
        n.parse::<i64>()
            .expect(msg)
            .checked_mul(1024 * 1024 * 1024)
            .expect(overflow)
    } else if let Some(n) = s.strip_suffix("Mi") {
        n.parse::<i64>()
            .expect(msg)
            .checked_mul(1024 * 1024)
            .expect(overflow)
    } else if let Some(n) = s.strip_suffix("G") {
        n.parse::<i64>()
            .expect(msg)
            .checked_mul(1_000_000_000)
            .expect(overflow)
    } else if let Some(n) = s.strip_suffix("M") {
        n.parse::<i64>()
            .expect(msg)
            .checked_mul(1_000_000)
            .expect(overflow)
    } else {
        s.parse::<i64>().expect(msg)
    }
}

/// Resolve the endpoint URL for a running container.
///
/// When `network` is `Some(name)`, looks up the container's IP on that network
/// and uses the lowest *container* port (not the host-mapped port). This lets
/// the server reach agents when both run inside Docker on the same network.
///
/// Falls back to `http://localhost:<host_port>` when:
/// - `network` is `None`, or
/// - the container is not connected to the named network.
fn extract_endpoint(
    network_settings: &Option<bollard::models::NetworkSettings>,
    network: Option<&str>,
) -> Option<String> {
    let ns = network_settings.as_ref()?;

    // Try the named network first: use container IP + lowest container port
    if let Some(net_name) = network {
        let ip = ns
            .networks
            .as_ref()
            .and_then(|nets| nets.get(net_name))
            .and_then(|n| n.ip_address.as_deref())
            .filter(|ip| !ip.is_empty());

        if let Some(ip) = ip {
            // Pick the lowest container port from exposed bindings
            if let Some(ports) = ns.ports.as_ref() {
                let mut keys: Vec<&String> = ports.keys().collect();
                keys.sort_by_key(|k| {
                    k.split('/')
                        .next()
                        .and_then(|p| p.parse::<u16>().ok())
                        .unwrap_or(0)
                });
                for key in keys {
                    if let Some(container_port) =
                        key.split('/').next().and_then(|p| p.parse::<u16>().ok())
                    {
                        return Some(format!("http://{ip}:{container_port}"));
                    }
                }
            }
        }
    }

    // Try any network the container is actually on (covers network_override
    // containers whose network differs from the runtime's default).
    if let Some(nets) = ns.networks.as_ref() {
        for ep_net in nets.values() {
            let ip = ep_net
                .ip_address
                .as_deref()
                .filter(|ip| !ip.is_empty());
            if let Some(ip) = ip {
                if let Some(ports) = ns.ports.as_ref() {
                    let mut keys: Vec<&String> = ports.keys().collect();
                    keys.sort_by_key(|k| {
                        k.split('/')
                            .next()
                            .and_then(|p| p.parse::<u16>().ok())
                            .unwrap_or(0)
                    });
                    for key in &keys {
                        if let Some(container_port) =
                            key.split('/').next().and_then(|p| p.parse::<u16>().ok())
                        {
                            return Some(format!("http://{ip}:{container_port}"));
                        }
                    }
                }
            }
        }
    }

    // Fall back to host-mapped port
    ns.ports
        .as_ref()
        .and_then(extract_host_port)
        .map(|hp| format!("http://localhost:{hp}"))
}

/// Label recording the JSON-serialized `DeploymentSpec::env_vars` applied at
/// container-creation time, so `deploy()` can detect env/secret changes on a
/// redeploy with an unchanged image tag (RUN-10a).
///
/// `inspect_container().config.env` is NOT usable for this: it reports the full
/// *merged* env (image-declared vars like `PATH` plus what we passed in), which is
/// always a superset of `spec.env_vars` — comparing against it produces a false
/// "changed" on every deploy. Recording exactly what we asked for, and diffing
/// against that, avoids the false positive.
const ENV_VARS_LABEL: &str = "nasiko.com/env-vars";

/// Read back the env vars recorded in [`ENV_VARS_LABEL`] on a running container.
/// Returns `None` if the label is missing (pre-fix container) or fails to parse —
/// callers should treat that as "unknown, assume changed" rather than as "unchanged".
fn stored_env_vars(
    config: Option<&bollard::models::ContainerConfig>,
) -> Option<HashMap<String, String>> {
    config
        .and_then(|c| c.labels.as_ref())
        .and_then(|l| l.get(ENV_VARS_LABEL))
        .and_then(|v| serde_json::from_str(v).ok())
}

// ── Deploy helpers ─────────────────────────────────────────────────────────────

/// Create and start a container from a `DeploymentSpec`.
/// If `network` is `Some`, the container is also connected to that Docker network
/// after starting so that server-side code running inside Docker can reach it.
async fn create_and_start(
    client: &Docker,
    spec: &DeploymentSpec,
    bind_host: &str,
    network: Option<&str>,
    timeout: Duration,
    registry_host: Option<&str>,
    image_source: Option<&dyn ImageSource>,
) -> Result<()> {
    let name = DockerRuntime::container_name(&spec.container_id);

    ensure_image_present(client, &spec.image, registry_host, image_source).await?;

    let env_vec: Vec<String> = spec
        .env_vars
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let (port_bindings, exposed_ports) = build_port_config(&spec.ports, bind_host);

    let host_config = build_host_config(spec, port_bindings);

    // Record the env vars we actually asked for as a label (see ENV_VARS_LABEL) so a
    // later deploy() can detect changes without being confused by image-baked-in vars
    // (e.g. PATH) that `inspect_container` reports as part of the merged env.
    let env_json = serde_json::to_string(&spec.env_vars).unwrap_or_default();
    let labels = HashMap::from([(ENV_VARS_LABEL.to_owned(), env_json)]);

    // Conventional "nobody" uid:gid — same value `ee/k8s-runtime` uses for its
    // non-root pod security context, kept consistent across editions.
    let user = spec.harden.then(|| "65534:65534".to_owned());

    let config = Config {
        image: Some(spec.image.clone()),
        env: Some(env_vec),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        labels: Some(labels),
        user,
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

    // `network_override` deploys already got `net` as their sole `NetworkMode`
    // at creation (see `build_host_config`) — connecting again here would just
    // error "already attached". Only deploys that rely on the runtime's
    // *default* network (agents, which never set `network_override`) still
    // need this explicit post-start connect, since their `NetworkMode` at
    // creation was Docker's own default ("bridge"), not `net`.
    if let Some(net) = network
        && spec.network_override.is_none()
    {
        let connect_opts = ConnectNetworkOptions {
            container: name.as_str(),
            ..Default::default()
        };
        tokio::time::timeout(timeout, client.connect_network(net, connect_opts))
            .await
            .map_err(|_| RuntimeError::Timeout("connect_network".to_owned()))?
            .map_err(map_bollard_err)?;
    }

    Ok(())
}

/// Make `image` available in the daemon's local cache: a no-op when already
/// present (local dev builds it via `docker build`), then a `docker load`
/// from `image_source` when one is wired, and a registry pull as the last
/// resort.
async fn ensure_image_present(
    client: &Docker,
    image: &str,
    registry_host: Option<&str>,
    image_source: Option<&dyn ImageSource>,
) -> Result<()> {
    if client.inspect_image(image).await.is_ok() {
        return Ok(());
    }
    if let Some(source) = image_source
        && load_image_from_source(client, image, source).await
    {
        return Ok(());
    }
    pull_image(client, image, registry_host).await
}

/// `docker load` the image from `source`. Every failure degrades to the pull
/// path — logged, never fatal — so a broken source cannot take down deploys
/// that a registry pull would have served.
async fn load_image_from_source(client: &Docker, image: &str, source: &dyn ImageSource) -> bool {
    let archive = match source.docker_archive(image).await {
        Ok(Some(archive)) => archive,
        Ok(None) => return false,
        Err(e) => {
            warn!(image, error = %e, "image source failed; falling back to registry pull");
            return false;
        }
    };

    let opts = ImportImageOptions { quiet: true };
    let mut stream = client.import_image(opts, bytes::Bytes::from(archive), None);
    while let Some(res) = stream.next().await {
        if let Err(e) = res {
            warn!(image, error = %e, "docker load failed; falling back to registry pull");
            return false;
        }
    }

    // Trust the daemon, not the load stream: only a ref that now resolves counts.
    let loaded = client.inspect_image(image).await.is_ok();
    if loaded {
        info!(image, "image loaded from image source");
    } else {
        warn!(
            image,
            "docker load completed but image still unresolved; falling back to pull"
        );
    }
    loaded
}

async fn pull_image(client: &Docker, image: &str, registry_host: Option<&str>) -> Result<()> {
    // Prefer the registry-qualified ref when a registry_host is configured.
    let pull_ref = match registry_host {
        Some(host) if !image.starts_with(host) => format!("{host}/{image}"),
        _ => image.to_owned(),
    };
    let opts = CreateImageOptions {
        from_image: pull_ref.as_str(),
        ..Default::default()
    };
    let mut stream = client.create_image(Some(opts), None, None);
    while let Some(res) = stream.next().await {
        if let Err(e) = res {
            return Err(RuntimeError::ImageNotFound(format!(
                "pull {pull_ref} failed: {e}"
            )));
        }
    }

    // A qualified pull lands as `{host}/{image}` in the daemon, but the
    // container is created with the bare `image` ref — without a retag the
    // create that follows would fail with "no such image".
    if pull_ref != image {
        tag_as_bare_ref(client, &pull_ref, image).await?;
    }
    Ok(())
}

/// Tag `pull_ref` so it also resolves as the bare `image` ref.
async fn tag_as_bare_ref(client: &Docker, pull_ref: &str, image: &str) -> Result<()> {
    let (repo, tag) = split_repo_tag(image);
    client
        .tag_image(
            pull_ref,
            Some(bollard::image::TagImageOptions { repo, tag }),
        )
        .await
        .map_err(|e| RuntimeError::ImageNotFound(format!("tag {pull_ref} as {image} failed: {e}")))
}

/// Split `repo:tag` on the tag separator; a `:` inside the last path segment
/// only (a colon before a `/` is a registry port, not a tag).
fn split_repo_tag(image: &str) -> (&str, &str) {
    match image.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') => (repo, tag),
        _ => (image, "latest"),
    }
}

/// Inspect a container and build a `DeploymentStatus`. Returns `Unknown` status
/// if the container does not exist (not an error).
///
/// When `network` is `Some`, the endpoint is the container's IP on that network
/// plus the lowest exposed port — so in-Docker callers can reach the agent directly.
/// Falls back to `localhost:host_port` when `network` is `None` or the container
/// is not connected to the named network.
async fn inspect_to_status(
    client: &Docker,
    container_id: &ContainerId,
    network: Option<&str>,
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
        extract_endpoint(&info.network_settings, network)
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
        // Only the MCP-server-upload build path sets `network_override` — every
        // existing agent deploy falls through to the runtime's default network.
        let network = spec
            .network_override
            .as_deref()
            .or(self.config.network.as_deref());

        match tokio::time::timeout(
            timeout,
            self.client
                .inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        {
            Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
            Ok(Err(ref e)) if is_not_found(e) => {
                // Container does not exist: create and start it
                create_and_start(
                    &self.client,
                    spec,
                    &self.config.bind_host,
                    self.config.network.as_deref(),
                    timeout,
                    self.config.registry_host.as_deref(),
                    self.image_source.as_deref(),
                )
                .await?;
            }
            Ok(Err(e)) => return Err(map_bollard_err(e)),
            Ok(Ok(existing)) => {
                let existing_image = existing
                    .config
                    .as_ref()
                    .and_then(|c| c.image.as_deref())
                    .unwrap_or("");
                // Docker containers can't have their env vars updated in-place — the only
                // way to apply changed env/secrets is to recreate the container. Without
                // this check, redeploying with the same image tag but rotated secrets was
                // a silent no-op (RUN-10a). K8s handles this correctly via its Secret +
                // Deployment reconcile; this mirrors that behavior for Docker.
                // A missing/unparseable label (pre-fix container) is treated as "changed"
                // — recreating once is safe; silently keeping stale env is not.
                let env_changed = stored_env_vars(existing.config.as_ref())
                    .is_none_or(|stored| stored != spec.env_vars);

                if existing_image == spec.image && !env_changed {
                    // Same image, same env: ensure the container is running (idempotent)
                    let current_status = existing.state.as_ref().and_then(|s| s.status);

                    if current_status != Some(ContainerStateStatusEnum::RUNNING) {
                        tokio::time::timeout(
                            timeout,
                            self.client
                                .start_container(&name, None::<StartContainerOptions<String>>),
                        )
                        .await
                        .map_err(|_| RuntimeError::Timeout("start_container".to_owned()))?
                        .or_else(|e| {
                            if is_not_modified(&e) {
                                Ok(())
                            } else {
                                Err(map_bollard_err(e))
                            }
                        })?;
                    }
                } else {
                    // Different image or changed env/secrets: stop → remove → recreate.
                    // Same container name is reused, so nothing else in the system (which
                    // addresses the agent by ContainerId → container name) needs to know
                    // the container was recreated rather than left running.
                    tokio::time::timeout(
                        timeout,
                        self.client
                            .stop_container(&name, None::<StopContainerOptions>),
                    )
                    .await
                    .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
                    .or_else(|e| {
                        if is_not_modified(&e) {
                            Ok(())
                        } else {
                            Err(map_bollard_err(e))
                        }
                    })?;

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

                    create_and_start(
                        &self.client,
                        spec,
                        &self.config.bind_host,
                        self.config.network.as_deref(),
                        timeout,
                        self.config.registry_host.as_deref(),
                        self.image_source.as_deref(),
                    )
                    .await?;
                }
            }
        }

        inspect_to_status(&self.client, &spec.container_id, network, timeout).await
    }

    #[instrument(skip(self))]
    async fn destroy(&self, container_id: &ContainerId) -> Result<()> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        match tokio::time::timeout(
            timeout,
            self.client
                .inspect_container(&name, None::<InspectContainerOptions>),
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
            self.client
                .stop_container(&name, None::<StopContainerOptions>),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
        .or_else(|e| {
            if is_not_modified(&e) {
                Ok(())
            } else {
                Err(map_bollard_err(e))
            }
        })?;

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
            self.client
                .inspect_container(&name, None::<InspectContainerOptions>),
        )
        .await
        {
            Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
            Ok(Err(ref e)) if is_not_found(e) => {
                return Err(RuntimeError::ContainerNotFound(container_id.clone()));
            }
            Ok(Err(e)) => return Err(map_bollard_err(e)),
            Ok(Ok(_)) => {}
        }

        if replicas == 0 {
            tokio::time::timeout(
                timeout,
                self.client
                    .stop_container(&name, None::<StopContainerOptions>),
            )
            .await
            .map_err(|_| RuntimeError::Timeout("stop_container".to_owned()))?
            .or_else(|e| {
                if is_not_modified(&e) {
                    Ok(())
                } else {
                    Err(map_bollard_err(e))
                }
            })?;
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
                self.client
                    .start_container(&name, None::<StartContainerOptions<String>>),
            )
            .await
            .map_err(|_| RuntimeError::Timeout("start_container".to_owned()))?
            .or_else(|e| {
                if is_not_modified(&e) {
                    Ok(())
                } else {
                    Err(map_bollard_err(e))
                }
            })?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn restart(&self, container_id: &ContainerId) -> Result<()> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        tokio::time::timeout(
            timeout,
            self.client
                .restart_container(&name, None::<RestartContainerOptions>),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("restart_container".to_owned()))?
        .map_err(|e| {
            if is_not_found(&e) {
                RuntimeError::ContainerNotFound(container_id.clone())
            } else {
                map_bollard_err(e)
            }
        })?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus> {
        container_id.validate()?;
        inspect_to_status(
            &self.client,
            container_id,
            self.config.network.as_deref(),
            self.config.operation_timeout,
        )
        .await
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

    /// One entry per running `nasiko-agent-*` container, with the Docker
    /// container ID as `instance_key` and the true `State.StartedAt`.
    ///
    /// Error policy: any transport-level failure (list, non-404 inspect) fails
    /// the whole call — a partial list would make the container-hours meter
    /// mass-close billing sessions for instances that are still alive. Only a
    /// container that disappeared between list and inspect (404) is skipped.
    #[instrument(skip(self))]
    async fn list_instances(&self) -> Result<Vec<InstanceInfo>> {
        let timeout = self.config.operation_timeout;
        let mut filters: HashMap<String, Vec<String>> = HashMap::new();
        // Anchor with ^/ to avoid substring matches (e.g. old-nasiko-agent-foo)
        filters.insert("name".to_owned(), vec!["^/nasiko-agent-".to_owned()]);

        let options = ListContainersOptions::<String> {
            all: false, // running containers only — stopped instances are not billable
            filters,
            ..Default::default()
        };

        let containers = tokio::time::timeout(timeout, self.client.list_containers(Some(options)))
            .await
            .map_err(|_| RuntimeError::Timeout("list_containers".to_owned()))?
            .map_err(map_bollard_err)?;

        let mut instances = Vec::with_capacity(containers.len());

        for container in containers {
            let Some(docker_id) = container.id.as_deref() else {
                continue;
            };

            let name = container
                .names
                .as_deref()
                .and_then(|names| names.first())
                .map(String::as_str)
                .unwrap_or("");

            let Some(container_id) = DockerRuntime::container_id_from_name(name) else {
                continue;
            };

            // Inspect for the true StartedAt and authoritative state. A container
            // that stopped between list and inspect is skipped, not an error.
            let info = match tokio::time::timeout(
                timeout,
                self.client
                    .inspect_container(docker_id, None::<InspectContainerOptions>),
            )
            .await
            {
                Err(_) => return Err(RuntimeError::Timeout("inspect_container".to_owned())),
                Ok(Err(ref e)) if is_not_found(e) => continue,
                Ok(Err(e)) => return Err(map_bollard_err(e)),
                Ok(Ok(info)) => info,
            };

            let container_state = info.state.as_ref();
            let state = map_container_state(
                container_state.and_then(|s| s.status),
                container_state.and_then(|s| s.exit_code),
            );
            let started_at =
                parse_docker_started_at(container_state.and_then(|s| s.started_at.as_deref()));

            instances.push(InstanceInfo {
                container_id,
                // Docker container ID (64-hex), NOT the name: names are reused across
                // recreate cycles, and `docker restart` keeps the ID but resets
                // StartedAt — the (instance_key, started_at) identity the meter keys on.
                instance_key: docker_id.to_owned(),
                started_at,
                ready: state == RuntimeState::Running,
            });
        }

        Ok(instances)
    }

    #[instrument(skip(self))]
    async fn endpoint(&self, container_id: &ContainerId) -> Result<String> {
        container_id.validate()?;
        let name = DockerRuntime::container_name(container_id);
        let timeout = self.config.operation_timeout;

        let info = tokio::time::timeout(
            timeout,
            self.client
                .inspect_container(&name, None::<InspectContainerOptions>),
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

        extract_endpoint(&info.network_settings, self.config.network.as_deref()).ok_or_else(|| {
            RuntimeError::Internal(format!("container {name} has no reachable endpoint"))
        })
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

#[cfg(test)]
mod hardening_tests {
    use super::*;

    fn spec(harden: bool) -> DeploymentSpec {
        DeploymentSpec {
            container_id: ContainerId::new("test-hardening"),
            name: "test-hardening".to_owned(),
            image: "alpine:latest".to_owned(),
            min_replicas: 1,
            max_replicas: 1,
            env_vars: HashMap::new(),
            ports: vec![8080],
            resources: None,
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
            harden,
            network_override: None,
            workload_kind: Default::default(),
        }
    }

    #[test]
    fn harden_true_sets_all_hardening_fields() {
        let (bindings, _) = build_port_config(&[8080], "127.0.0.1");
        let hc = build_host_config(&spec(true), bindings);
        assert_eq!(hc.readonly_rootfs, Some(true));
        assert_eq!(
            hc.tmpfs,
            Some(HashMap::from([("/tmp".to_owned(), "size=64m".to_owned())]))
        );
        assert_eq!(hc.cap_drop, Some(vec!["ALL".to_owned()]));
        assert_eq!(
            hc.security_opt,
            Some(vec!["no-new-privileges:true".to_owned()])
        );
    }

    #[test]
    fn harden_false_leaves_hardening_fields_unset() {
        let (bindings, _) = build_port_config(&[8080], "127.0.0.1");
        let hc = build_host_config(&spec(false), bindings);
        assert_eq!(hc.readonly_rootfs, None);
        assert_eq!(hc.tmpfs, None);
        assert_eq!(hc.cap_drop, None);
        assert_eq!(hc.security_opt, None);
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_docker_started_at_accepts_valid_rfc3339() {
        let parsed = parse_docker_started_at(Some("2026-07-21T10:00:05.123456789Z"))
            .expect("valid RFC3339 timestamp should parse");
        assert_eq!(parsed.to_rfc3339(), "2026-07-21T10:00:05.123456789+00:00");
    }

    #[test]
    fn parse_docker_started_at_rejects_never_started_sentinel() {
        assert_eq!(parse_docker_started_at(Some("0001-01-01T00:00:00Z")), None);
    }

    #[test]
    fn parse_docker_started_at_rejects_garbage_and_none() {
        assert_eq!(parse_docker_started_at(Some("not-a-timestamp")), None);
        assert_eq!(parse_docker_started_at(None), None);
    }

    #[test]
    fn split_repo_tag_separates_repo_and_tag() {
        assert_eq!(
            split_repo_tag("nasiko/nutrition:1.0.1"),
            ("nasiko/nutrition", "1.0.1")
        );
    }

    #[test]
    fn split_repo_tag_defaults_untagged_to_latest() {
        assert_eq!(
            split_repo_tag("nasiko/nutrition"),
            ("nasiko/nutrition", "latest")
        );
    }

    #[test]
    fn split_repo_tag_ignores_registry_port_colon() {
        assert_eq!(
            split_repo_tag("localhost:5000/nutrition"),
            ("localhost:5000/nutrition", "latest")
        );
        assert_eq!(
            split_repo_tag("localhost:5000/nutrition:2.0"),
            ("localhost:5000/nutrition", "2.0")
        );
    }
}
