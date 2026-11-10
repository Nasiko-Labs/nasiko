//! `SandboxProvider` over the local Docker daemon: one container per sandbox
//! key, stopped between turns and started again on the next `acquire`.
//!
//! Everything a later call needs (first port, transport, key) is recorded as
//! container labels at creation, because `attach`/`list` only get a handle and
//! not the spec. A resumed container is never recreated, so those labels are
//! also what "resumed keeps the original spec" means in practice.
//!
//! Not hardened yet: no read-only rootfs, tmpfs or capability drop. The harness images
//! still write outside `/workspace`, so confining them would break them until they are
//! aligned. TODO NAS-1102: apply the `DockerRuntime` hardening once they are.

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions,
    RemoveContainerOptions, StartContainerOptions, StatsOptions, StopContainerOptions,
};
use bollard::models::{
    ContainerInspectResponse, ContainerStateStatusEnum, HostConfig, Mount, PortBinding,
};
use bollard::volume::CreateVolumeOptions;
use futures_util::StreamExt;
use tokio::net::TcpStream;
use tracing::warn;
use uuid::Uuid;

use super::{
    DockerRuntime, DockerRuntimeConfig, build_port_config, ensure_agent_memory_subdir,
    ensure_image_present, is_conflict, is_not_found, is_not_modified, map_bollard_err, network_ip,
    parse_memory_bytes, volume_subpath_mount,
};
use crate::error::{Result, RuntimeError};
use crate::sandbox::{
    HARNESS_SCRATCH_DIR, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider,
    SandboxProviderKind, SandboxSpec, SandboxState, SandboxStats, SandboxTransport, TransportKind,
    WORKSPACE_MOUNT,
};
use crate::stats::cpu_percent;
use crate::{ImageSource, ResourceLimits};

const NAME_PREFIX: &str = "nasiko-sbx-";
/// Appended to the container name to name the sandbox's scratch volume.
const SCRATCH_VOLUME_SUFFIX: &str = "-scratch";
const LABEL_KEY: &str = "nasiko.com/sandbox-key";
const LABEL_KIND: &str = "nasiko.com/sandbox-kind";
const LABEL_ID: &str = "nasiko.com/sandbox-id";
const LABEL_CONTEXT: &str = "nasiko.com/sandbox-context-id";
const LABEL_IMAGE: &str = "nasiko.com/sandbox-image";
const LABEL_ENV_KEYS: &str = "nasiko.com/sandbox-env-keys";
const LABEL_PORT: &str = "nasiko.com/sandbox-port";
const LABEL_TRANSPORT: &str = "nasiko.com/sandbox-transport";
const LABEL_WORKLOAD: &str = "nasiko.com/workload";
const WORKLOAD_SANDBOX: &str = "sandbox";
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
const STOP_GRACE_SECS: i64 = 10;

/// Docker-backed [`SandboxProvider`]: container `nasiko-sbx-<key name>` per sandbox.
///
/// Suspend is `docker stop`, not `pause`: a paused container still holds its RAM and does
/// not survive a daemon restart, a stopped one does. Host ports are ephemeral, so a
/// resumed sandbox may come back on a different port; callers re-`attach` after `acquire`.
///
/// The handle's `backend_ref` is the container NAME (`nasiko-sbx-<key>`), deterministic per key,
/// so a persisted ref stays valid when a dead container is removed and recreated.
///
/// `operation_timeout` bounds each daemon call and the readiness wait; it does NOT cover
/// the image pull, which has its own `build_timeout` budget.
pub struct DockerSandboxProvider {
    /// Reuses `DockerRuntime`'s client, config and image-source plumbing.
    runtime: DockerRuntime,
}

impl std::fmt::Debug for DockerSandboxProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerSandboxProvider")
            .finish_non_exhaustive()
    }
}

type DaemonResult<T> = std::result::Result<T, bollard::errors::Error>;

impl DockerSandboxProvider {
    /// Builds a provider over an already-connected daemon client. `image_source`, when set,
    /// satisfies image-cache misses before a registry pull, exactly as for `DockerRuntime`.
    pub fn new(
        client: Docker,
        config: DockerRuntimeConfig,
        image_source: Option<Arc<dyn ImageSource>>,
    ) -> Self {
        Self {
            runtime: DockerRuntime {
                client,
                config,
                image_source,
            },
        }
    }

    fn client(&self) -> &Docker {
        &self.runtime.client
    }

    fn config(&self) -> &DockerRuntimeConfig {
        &self.runtime.config
    }

    /// Runs one daemon call under `budget`. An error for which `tolerated` is true becomes
    /// `Ok(None)` (the caller decides what "already gone" or "already done" means).
    async fn timed<T>(
        &self,
        budget: Duration,
        what: &str,
        call: impl std::future::Future<Output = DaemonResult<T>>,
        tolerated: fn(&bollard::errors::Error) -> bool,
    ) -> Result<Option<T>> {
        match tokio::time::timeout(budget, call).await {
            Err(_) => Err(RuntimeError::Timeout(what.to_owned())),
            Ok(Err(e)) if tolerated(&e) => Ok(None),
            Ok(Err(e)) => Err(map_bollard_err(e)),
            Ok(Ok(v)) => Ok(Some(v)),
        }
    }

    /// One daemon call under `operation_timeout`, every error mapped.
    async fn bounded<T>(
        &self,
        what: &str,
        call: impl std::future::Future<Output = DaemonResult<T>>,
    ) -> Result<T> {
        tokio::time::timeout(self.config().operation_timeout, call)
            .await
            .map_err(|_| RuntimeError::Timeout(what.to_owned()))?
            .map_err(map_bollard_err)
    }

    /// [`Self::timed`] under `operation_timeout`.
    async fn bounded_tolerating<T>(
        &self,
        what: &str,
        call: impl std::future::Future<Output = DaemonResult<T>>,
        tolerated: fn(&bollard::errors::Error) -> bool,
    ) -> Result<Option<T>> {
        let budget = self.config().operation_timeout;
        self.timed(budget, what, call, tolerated).await
    }

    /// `None` when the container does not exist.
    async fn inspect(&self, name_or_id: &str) -> Result<Option<ContainerInspectResponse>> {
        let call = self
            .client()
            .inspect_container(name_or_id, None::<InspectContainerOptions>);
        self.bounded_tolerating("inspect_container", call, is_not_found)
            .await
    }

    /// Starts a container. `Ok(None)` means it vanished (a concurrent `release`); a 304
    /// (already running) is success.
    async fn start(&self, name_or_id: &str) -> Result<Option<()>> {
        let call = async {
            match self
                .client()
                .start_container(name_or_id, None::<StartContainerOptions<String>>)
                .await
            {
                Err(e) if is_not_modified(&e) => Ok(()),
                other => other,
            }
        };
        self.bounded_tolerating("start_container", call, is_not_found)
            .await
    }

    /// Brings an existing container back: no-op if running, `unpause` if paused (`start`
    /// would be rejected), `start` otherwise. `Ok(None)` means it vanished meanwhile.
    async fn resume(&self, name: &str, info: &ContainerInspectResponse) -> Result<Option<()>> {
        match info.state.as_ref().and_then(|s| s.status) {
            Some(ContainerStateStatusEnum::RUNNING) => Ok(Some(())),
            Some(ContainerStateStatusEnum::PAUSED) => {
                let call = self.client().unpause_container(name);
                self.bounded_tolerating("unpause_container", call, is_not_found)
                    .await
            }
            _ => self.start(name).await,
        }
    }

    /// Creates the scratch volume of `key`'s sandbox, labelled with the key, so a volume a failed
    /// removal leaves behind can still be traced to its sandbox. Idempotent.
    async fn create_scratch_volume(&self, key: &SandboxKey) -> Result<()> {
        let options = CreateVolumeOptions {
            name: scratch_volume(key),
            labels: scratch_volume_labels(key),
            ..Default::default()
        };
        let call = self.client().create_volume(options);
        self.bounded("create_volume", call).await.map(drop)
    }

    /// Removes the scratch volume of `key`'s sandbox (see [`scratch_mount`]), if its key has one.
    /// Its contents regenerate, so a volume that cannot be removed is logged rather than failing
    /// the caller, and the next sandbox under the key reuses it.
    async fn remove_scratch(&self, key: &SandboxKey) {
        if !key.has_scratch() {
            return;
        }
        let volume = scratch_volume(key);
        let call = self.client().remove_volume(&volume, None);
        if let Err(e) = self
            .bounded_tolerating("remove_volume", call, is_not_found)
            .await
        {
            warn!(%volume, error = %e, "could not remove a sandbox's scratch volume");
        }
    }

    /// `rm -f`; Ok when the container is already gone.
    async fn remove(&self, name_or_id: &str) -> Result<()> {
        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        let call = self.client().remove_container(name_or_id, Some(options));
        self.bounded_tolerating("remove_container", call, is_not_found)
            .await
            .map(drop)
    }

    /// Pulls/loads the image under `build_timeout` (a pull is far slower than any other
    /// daemon call, so `operation_timeout` does not apply).
    async fn ensure_image(&self, image: &str) -> Result<()> {
        let creds = self.runtime.registry_credentials();
        let ensure = ensure_image_present(
            self.client(),
            image,
            self.config().registry_host.as_deref(),
            creds.as_ref(),
            self.runtime.image_source.as_deref(),
            false,
        );
        tokio::time::timeout(self.config().build_timeout, ensure)
            .await
            .map_err(|_| RuntimeError::Timeout("image pull".to_owned()))?
    }

    /// Ensures the image, prepares the per-key workspace subdirectory and scratch volume, creates
    /// the container. A 409 means a concurrent `acquire` created it first, which is fine.
    async fn create(&self, spec: &SandboxSpec) -> Result<()> {
        self.ensure_image(&spec.image).await?;
        if let Some(ws) = &spec.workspace {
            ensure_agent_memory_subdir(
                self.client(),
                self.config().operation_timeout,
                &self.config().agent_memory_volume,
                &self.config().agent_memory_init_image,
                &workspace_subpath(ws.owner_id, &ws.container_id, &spec.key),
            )
            .await?;
        }
        if spec.key.has_scratch() {
            self.create_scratch_volume(&spec.key).await?;
            // The same init helper as the workspace: a fresh volume is root's, and the helper
            // hands the directory that gets mounted to uid 65534. Its own `create_volume` finds
            // the volume above, labels and all.
            ensure_agent_memory_subdir(
                self.client(),
                self.config().operation_timeout,
                &scratch_volume(&spec.key),
                &self.config().agent_memory_init_image,
                &spec.key.name(),
            )
            .await?;
        }
        let name = container_name(&spec.key);
        let options = CreateContainerOptions {
            name: name.as_str(),
            platform: None,
        };
        let call = self
            .client()
            .create_container(Some(options), container_config(spec, self.config()));
        self.bounded_tolerating("create_container", call, is_conflict)
            .await
            .map(drop)
    }

    /// One inspect-then-act pass. `Ok(None)` means the container vanished mid-pass.
    async fn acquire_once(&self, spec: &SandboxSpec) -> Result<Option<()>> {
        let name = container_name(&spec.key);
        let mut existing = self.inspect(&name).await?;
        // A `dead` container cannot be started again; remove it and recreate.
        if existing.as_ref().is_some_and(is_dead) {
            self.remove(&name).await?;
            existing = None;
        }
        let created = existing.is_none();
        let outcome = self.bring_up(spec, &name, existing).await;
        // A first acquire that fails leaves nothing behind: a container this call created and
        // could not bring up (never ready, or exited at once) would otherwise sit there, and the
        // caller, seeing the error, would treat the sandbox as not existing. A resume that fails
        // leaves the container, which was there before this call and may still be good.
        if outcome.is_err() && rolls_back(created) {
            match self.remove(&name).await {
                Ok(()) => self.remove_scratch(&spec.key).await,
                Err(e) => {
                    warn!(sandbox = %name, error = %e, "could not remove a sandbox that failed to start")
                }
            }
        }
        outcome
    }

    /// Create (or resume) the container and wait until it accepts TCP.
    async fn bring_up(
        &self,
        spec: &SandboxSpec,
        name: &str,
        existing: Option<ContainerInspectResponse>,
    ) -> Result<Option<()>> {
        let resumed = match existing {
            None => {
                self.create(spec).await?;
                self.start(name).await?
            }
            // A resumed sandbox keeps its original spec: resume it, never recreate it.
            Some(info) => {
                warn_on_drift(name, &info, spec);
                self.resume(name, &info).await?
            }
        };
        match resumed {
            Some(()) => self.wait_ready(name).await.map(Some),
            None => Ok(None),
        }
    }

    /// Polls until the container runs and accepts TCP on its first port, or times out.
    /// "Running" for this provider therefore means "accepting TCP", not just "process
    /// started". A container seen exited or dead fails at once instead of timing out.
    async fn wait_ready(&self, name: &str) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.config().operation_timeout;
        loop {
            let info = self
                .inspect(name)
                .await?
                .ok_or_else(|| RuntimeError::SandboxNotFound(format!("docker:{name}")))?;
            fail_if_exited(name, &info)?;
            if state_of(&info) == SandboxState::Running
                && let Some(addr) = self.address(&info)?
                && accepts_tcp(&addr).await
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RuntimeError::Timeout(format!(
                    "sandbox {name} not ready within {:?}",
                    self.config().operation_timeout
                )));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Where the first port is reachable from this process, if it is yet.
    fn address(&self, info: &ContainerInspectResponse) -> Result<Option<Addr>> {
        let target = AttachTarget::from_inspect(info)?;
        Ok(endpoint_addr(
            info,
            &target,
            self.config().network.as_deref(),
            &self.config().bind_host,
        ))
    }
}

/// Whether a failed `acquire` removes the container: only one this call created.
fn rolls_back(created_by_this_call: bool) -> bool {
    created_by_this_call
}

/// When a container last started, from its inspect, if it ever did.
fn started_at_of(info: &ContainerInspectResponse) -> Option<chrono::DateTime<chrono::Utc>> {
    super::parse_docker_started_at(
        info.state
            .as_ref()
            .and_then(|state| state.started_at.as_deref()),
    )
}

/// A host and port a client can connect to.
#[derive(Debug, PartialEq, Eq)]
struct Addr {
    host: String,
    port: String,
}

async fn accepts_tcp(addr: &Addr) -> bool {
    let target = format!("{}:{}", addr.host, addr.port);
    matches!(
        tokio::time::timeout(CONNECT_ATTEMPT_TIMEOUT, TcpStream::connect(target)).await,
        Ok(Ok(_))
    )
}

/// Errors when a container we just started is already exited or dead.
fn fail_if_exited(name: &str, info: &ContainerInspectResponse) -> Result<()> {
    let state = info.state.as_ref();
    if matches!(
        state.and_then(|s| s.status),
        Some(ContainerStateStatusEnum::EXITED | ContainerStateStatusEnum::DEAD)
    ) {
        let code = state.and_then(|s| s.exit_code).unwrap_or(-1);
        warn!(sandbox = name, code, "sandbox exited right after start");
        return Err(RuntimeError::Internal(format!(
            "sandbox {name} exited with status {code}"
        )));
    }
    Ok(())
}

/// Warns when a resumed container's recorded image or env keys differ from the spec: the
/// container keeps what it was created with.
fn warn_on_drift(name: &str, info: &ContainerInspectResponse, spec: &SandboxSpec) {
    let labels = info.config.as_ref().and_then(|c| c.labels.as_ref());
    let drift = spec_drift(labels, spec);
    if !drift.is_empty() {
        warn!(
            sandbox = name,
            ?drift,
            "resuming with a different image/env; release first to apply"
        );
    }
}

/// Which of `image` / `env` differ between the container's labels and `spec`.
fn spec_drift(labels: Option<&HashMap<String, String>>, spec: &SandboxSpec) -> Vec<&'static str> {
    let Some(labels) = labels else {
        return Vec::new();
    };
    let mut drift = Vec::new();
    if labels.get(LABEL_IMAGE).is_some_and(|i| *i != spec.image) {
        drift.push("image");
    }
    if labels
        .get(LABEL_ENV_KEYS)
        .is_some_and(|k| *k != env_keys_label(spec))
    {
        drift.push("env");
    }
    drift
}

/// The first port and transport recorded on a container at creation.
struct AttachTarget {
    port: u16,
    transport: TransportKind,
}

impl AttachTarget {
    /// A container without these labels is not one of ours: an error, never a guess.
    fn from_inspect(info: &ContainerInspectResponse) -> Result<Self> {
        let labels = info
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .ok_or_else(|| RuntimeError::Internal("sandbox container has no labels".to_owned()))?;
        Self::from_labels(labels)
    }

    fn from_labels(labels: &HashMap<String, String>) -> Result<Self> {
        let bad = |what: &str| RuntimeError::Internal(format!("sandbox label {what} is invalid"));
        let port = labels
            .get(LABEL_PORT)
            .ok_or_else(|| bad(LABEL_PORT))?
            .parse()
            .map_err(|_| bad(LABEL_PORT))?;
        let transport = match labels.get(LABEL_TRANSPORT).map(String::as_str) {
            Some("http") => TransportKind::Http,
            Some("websocket") => TransportKind::WebSocket,
            _ => return Err(bad(LABEL_TRANSPORT)),
        };
        Ok(Self { port, transport })
    }
}

// ── Pure helpers ───────────────────────────────────────────────────────────────

/// `nasiko-sbx-<key name>`.
fn container_name(key: &SandboxKey) -> String {
    format!("{NAME_PREFIX}{}", key.name())
}

/// The workspace directory a sandbox mounts.
///
/// An `Agent` sandbox IS the whole agent, so its workspace is the agent's own directory,
/// `{owner_id}/{container_id}`: exactly what a resident `--writable` deploy mounts, so a
/// parked agent finds its files again on wake. `Session` and `AgentContext` sandboxes are
/// sub-units of a harness or agent and keep a per-key subdirectory below it.
fn workspace_subpath(owner: Uuid, container_id: &crate::ContainerId, key: &SandboxKey) -> String {
    match key {
        SandboxKey::Agent { .. } => format!("{owner}/{}", container_id.as_str()),
        SandboxKey::Session { .. } | SandboxKey::AgentContext { .. } => {
            format!("{owner}/{}/{}", container_id.as_str(), key.name())
        }
    }
}

fn kind_label(key: &SandboxKey) -> &'static str {
    match key {
        SandboxKey::Session { .. } => "session",
        SandboxKey::Agent { .. } => "agent",
        SandboxKey::AgentContext { .. } => "agent-context",
    }
}

fn transport_label(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Http => "http",
        TransportKind::WebSocket => "websocket",
    }
}

/// Sorted JSON of the env KEYS (never values, which may be secrets), for drift detection.
fn env_keys_label(spec: &SandboxSpec) -> String {
    let keys: BTreeSet<&String> = spec.env.keys().collect();
    serde_json::to_string(&keys).unwrap_or_default()
}

/// Every label a sandbox container carries.
fn sandbox_labels(spec: &SandboxSpec) -> HashMap<String, String> {
    let mut labels = HashMap::from([
        (LABEL_KEY.to_owned(), spec.key.name()),
        (LABEL_KIND.to_owned(), kind_label(&spec.key).to_owned()),
        (LABEL_WORKLOAD.to_owned(), WORKLOAD_SANDBOX.to_owned()),
        (LABEL_IMAGE.to_owned(), spec.image.clone()),
        (LABEL_ENV_KEYS.to_owned(), env_keys_label(spec)),
        (LABEL_PORT.to_owned(), spec.ports[0].to_string()),
        (
            LABEL_TRANSPORT.to_owned(),
            transport_label(spec.transport).to_owned(),
        ),
    ]);
    match &spec.key {
        SandboxKey::Session { session_id } => {
            labels.insert(LABEL_ID.to_owned(), session_id.to_string());
        }
        SandboxKey::Agent { agent_id } => {
            labels.insert(LABEL_ID.to_owned(), agent_id.to_string());
        }
        SandboxKey::AgentContext {
            agent_id,
            context_id,
        } => {
            labels.insert(LABEL_ID.to_owned(), agent_id.to_string());
            // Stored verbatim: the name only carries a hash, and `list` must rebuild the key.
            labels.insert(LABEL_CONTEXT.to_owned(), context_id.clone());
        }
    }
    labels
}

/// Rebuilds the key a container was created for. `None` when labels are missing or corrupt.
fn key_from_labels(labels: &HashMap<String, String>) -> Option<SandboxKey> {
    let id: Uuid = labels.get(LABEL_ID)?.parse().ok()?;
    match labels.get(LABEL_KIND)?.as_str() {
        "session" => Some(SandboxKey::Session { session_id: id }),
        "agent" => Some(SandboxKey::Agent { agent_id: id }),
        "agent-context" => Some(SandboxKey::AgentContext {
            agent_id: id,
            context_id: labels.get(LABEL_CONTEXT)?.clone(),
        }),
        _ => None,
    }
}

/// Docker status to sandbox state. Docker has no observable "starting" once `start` has
/// returned, so only `restarting` maps to Starting. `created`, `exited` and `paused` are
/// Suspended (`acquire` starts or unpauses them); `dead` is Suspended too, but `acquire`
/// removes and recreates it. `removing` and an unknown status are Absent.
fn map_status(status: Option<ContainerStateStatusEnum>) -> SandboxState {
    match status {
        Some(ContainerStateStatusEnum::RUNNING) => SandboxState::Running,
        Some(ContainerStateStatusEnum::RESTARTING) => SandboxState::Starting,
        Some(ContainerStateStatusEnum::REMOVING) | None => SandboxState::Absent,
        Some(_) => SandboxState::Suspended,
    }
}

/// Contract for `stats`: Absent is `SandboxNotFound`, Starting/Suspended `SandboxNotRunning`.
fn require_running_for_stats(state: Option<SandboxState>, handle: &SandboxHandle) -> Result<()> {
    match state {
        Some(SandboxState::Running) => Ok(()),
        Some(SandboxState::Starting | SandboxState::Suspended) => {
            Err(RuntimeError::SandboxNotRunning(handle.sandbox_ref()))
        }
        Some(SandboxState::Absent) | None => {
            Err(RuntimeError::SandboxNotFound(handle.sandbox_ref()))
        }
    }
}

fn is_dead(info: &ContainerInspectResponse) -> bool {
    info.state.as_ref().and_then(|s| s.status) == Some(ContainerStateStatusEnum::DEAD)
}

fn state_of(info: &ContainerInspectResponse) -> SandboxState {
    map_status(info.state.as_ref().and_then(|s| s.status))
}

/// The host port bound to container `port`, if Docker has published it yet.
fn host_port_for(ports: &HashMap<String, Option<Vec<PortBinding>>>, port: u16) -> Option<String> {
    ports
        .get(&format!("{port}/tcp"))?
        .as_ref()?
        .first()?
        .host_port
        .clone()
        .filter(|p| !p.is_empty())
}

/// A wildcard bind address is not connectable; loopback is.
fn connect_host(bind_host: &str) -> &str {
    match bind_host {
        "0.0.0.0" | "::" | "" => "127.0.0.1",
        other => other,
    }
}

/// Where the first port can be reached. When the runtime is configured with a Docker
/// `network` (the server itself runs in a container on it, where the host's loopback is
/// not the Docker host) and the container has an IP there, that IP and the container port
/// are used; otherwise the published host port on `bind_host`. `None` until reachable.
fn endpoint_addr(
    info: &ContainerInspectResponse,
    target: &AttachTarget,
    network: Option<&str>,
    bind_host: &str,
) -> Option<Addr> {
    let ns = info.network_settings.as_ref()?;
    if let Some(ip) = network.and_then(|n| network_ip(ns, n)) {
        return Some(Addr {
            host: ip.to_owned(),
            port: target.port.to_string(),
        });
    }
    let port = host_port_for(ns.ports.as_ref()?, target.port)?;
    Some(Addr {
        host: connect_host(bind_host).to_owned(),
        port,
    })
}

fn transport_url(kind: TransportKind, addr: &Addr) -> SandboxTransport {
    let Addr { host, port } = addr;
    match kind {
        TransportKind::Http => SandboxTransport::Http {
            url: format!("http://{host}:{port}"),
        },
        TransportKind::WebSocket => SandboxTransport::WebSocket {
            url: format!("ws://{host}:{port}/acp"),
            headers: vec![],
            tls: None,
        },
    }
}

/// Env as `K=V`. `HOME` is not defaulted: the image's own wins unless the caller passes one.
fn env_vec(spec: &SandboxSpec) -> Vec<String> {
    spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Limits, the per-key workspace and scratch mounts, the configured network and the requested
/// runtime class (e.g. `runsc`). No hardening: see the module docs.
fn sandbox_host_config(spec: &SandboxSpec, config: &DockerRuntimeConfig) -> HostConfig {
    let limits: ResourceLimits = spec.resources.clone().unwrap_or_default();
    let (port_bindings, _) = build_port_config(&spec.ports, &config.bind_host);
    let workspace = spec.workspace.as_ref().map(|ws| {
        volume_subpath_mount(
            WORKSPACE_MOUNT,
            &config.agent_memory_volume,
            &workspace_subpath(ws.owner_id, &ws.container_id, &spec.key),
        )
    });
    let scratch = spec.key.has_scratch().then(|| scratch_mount(&spec.key));
    let mounts: Vec<Mount> = workspace.into_iter().chain(scratch).collect();
    HostConfig {
        port_bindings: Some(port_bindings),
        memory: Some(parse_memory_bytes(&limits.memory)),
        nano_cpus: Some(i64::from(limits.cpu_milli) * 1_000_000),
        network_mode: config.network.clone(),
        runtime: spec.runtime_class.clone(),
        mounts: (!mounts.is_empty()).then_some(mounts),
        ..Default::default()
    }
}

/// The sandbox's scratch area at [`HARNESS_SCRATCH_DIR`]: a Docker volume of its own on the
/// daemon's local disk, kept while the container is stopped and removed with it on `release`.
/// Docker does not cap a local volume's size. The directory mounted is one inside the volume,
/// because the init helper hands a directory, not a volume's root, to uid 65534.
fn scratch_mount(key: &SandboxKey) -> Mount {
    volume_subpath_mount(HARNESS_SCRATCH_DIR, &scratch_volume(key), &key.name())
}

/// `nasiko-sbx-<key name>-scratch`: one volume per sandbox, so removing it takes exactly that
/// sandbox's scratch.
fn scratch_volume(key: &SandboxKey) -> String {
    format!("{}{SCRATCH_VOLUME_SUFFIX}", container_name(key))
}

/// The sandbox key and kind, as its container carries them
/// (`docker volume ls --filter label=nasiko.com/sandbox-key=<key name>`).
fn scratch_volume_labels(key: &SandboxKey) -> HashMap<String, String> {
    HashMap::from([
        (LABEL_KEY.to_owned(), key.name()),
        (LABEL_KIND.to_owned(), kind_label(key).to_owned()),
    ])
}

/// Sandboxes with a workspace run as uid:gid 65534, the uid `KubeRuntime` gives every pod and
/// the one the workspace init step chowns the subdirectory to. Harness images are being
/// aligned to the same uid; without a workspace the image's own user is kept.
fn workspace_user(spec: &SandboxSpec) -> Option<String> {
    spec.workspace.is_some().then(|| "65534:65534".to_owned())
}

fn container_config(spec: &SandboxSpec, config: &DockerRuntimeConfig) -> Config<String> {
    let (_, exposed_ports) = build_port_config(&spec.ports, &config.bind_host);
    Config {
        image: Some(spec.image.clone()),
        cmd: (!spec.command.is_empty()).then(|| spec.command.clone()),
        env: Some(env_vec(spec)),
        user: workspace_user(spec),
        exposed_ports: Some(exposed_ports),
        host_config: Some(sandbox_host_config(spec, config)),
        labels: Some(sandbox_labels(spec)),
        ..Default::default()
    }
}

/// Millicores from two cumulative samples. `None` (never 0) when there is no figure: no
/// system delta, or no CPU count (`online_cpus`, else the length of the per-CPU list).
fn millicores_from(
    cpu: (u64, u64),
    system: (u64, u64),
    online_cpus: Option<u64>,
    percpu_len: usize,
) -> Option<u32> {
    let cpus = online_cpus
        .filter(|n| *n > 0)
        .or_else(|| Some(percpu_len as u64).filter(|n| *n > 0))?;
    let pct = cpu_percent(cpu.0, cpu.1, system.0, system.1, cpus)?;
    Some((pct * 10.0).round() as u32)
}

fn millicores(stats: &bollard::container::Stats) -> Option<u32> {
    let (now, pre) = (&stats.cpu_stats, &stats.precpu_stats);
    millicores_from(
        (now.cpu_usage.total_usage, pre.cpu_usage.total_usage),
        (
            now.system_cpu_usage.unwrap_or(0),
            pre.system_cpu_usage.unwrap_or(0),
        ),
        now.online_cpus,
        now.cpu_usage.percpu_usage.as_ref().map_or(0, Vec::len),
    )
}

// ── Trait impl ─────────────────────────────────────────────────────────────────

#[async_trait]
impl SandboxProvider for DockerSandboxProvider {
    fn kind(&self) -> SandboxProviderKind {
        SandboxProviderKind::Docker
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        spec.validate()?;
        // A concurrent `release` can remove the container between our inspect and our
        // start; redo the whole pass once before giving up.
        for _ in 0..2 {
            if self.acquire_once(spec).await?.is_some() {
                // The container NAME is the backend ref: deterministic per key, so a
                // persisted ref survives a dead-container remove-and-recreate (a container
                // id would not). Docker accepts a name wherever it takes an id.
                return Ok(SandboxHandle {
                    key: spec.key.clone(),
                    provider: SandboxProviderKind::Docker,
                    backend_ref: container_name(&spec.key),
                });
            }
        }
        Err(RuntimeError::SandboxNotFound(format!(
            "docker:{}",
            container_name(&spec.key)
        )))
    }

    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport> {
        let info = self
            .inspect(&handle.backend_ref)
            .await?
            .ok_or_else(|| RuntimeError::SandboxNotFound(handle.sandbox_ref()))?;
        if state_of(&info) != SandboxState::Running {
            return Err(RuntimeError::SandboxNotRunning(handle.sandbox_ref()));
        }
        let target = AttachTarget::from_inspect(&info)?;
        let addr = self
            .address(&info)?
            .ok_or_else(|| RuntimeError::SandboxNotRunning(handle.sandbox_ref()))?;
        Ok(transport_url(target.transport, &addr))
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        Ok(self
            .inspect(&handle.backend_ref)
            .await?
            .map_or(SandboxState::Absent, |info| state_of(&info)))
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        let call = self.client().stop_container(
            &handle.backend_ref,
            Some(StopContainerOptions { t: STOP_GRACE_SECS }),
        );
        let budget = self.config().operation_timeout + Duration::from_secs(STOP_GRACE_SECS as u64);
        self.timed(budget, "stop_container", call, |e| {
            is_not_found(e) || is_not_modified(e)
        })
        .await
        .map(drop)
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        // The workspace subdirectory is left on disk: it is persistent by contract. The scratch
        // volume goes with the container: what it holds regenerates.
        self.remove(&handle.backend_ref).await?;
        self.remove_scratch(&handle.key).await;
        Ok(())
    }

    /// A resumed container is the one created, env and all: only `release` (which keeps the
    /// workspace subdirectory) and a fresh `acquire` change its env.
    async fn resume_takes_new_env(&self, _handle: &SandboxHandle) -> Result<bool> {
        Ok(false)
    }

    /// Samples with `one_shot: false`: the one-shot form returns zeroed `precpu_stats`, which
    /// would make every CPU delta indeterminate. `stream: false` still yields a single frame.
    ///
    /// That frame costs about one collector interval (~1 s), so a caller sampling many
    /// sandboxes must do so concurrently. Never reports 0 for "no sample": when no CPU figure
    /// can be derived the result is `Internal("no cpu sample")` so the caller skips the tick.
    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats> {
        let info = self.inspect(&handle.backend_ref).await?;
        require_running_for_stats(info.as_ref().map(state_of), handle)?;
        let opts = StatsOptions {
            stream: false,
            one_shot: false,
        };
        let mut stream = self.client().stats(&handle.backend_ref, Some(opts));
        let frame = tokio::time::timeout(self.config().operation_timeout, stream.next())
            .await
            .map_err(|_| RuntimeError::Timeout("stats".to_owned()))?;
        match frame {
            Some(Ok(stats)) => Ok(SandboxStats {
                cpu_millicores: millicores(&stats)
                    .ok_or_else(|| RuntimeError::Internal("no cpu sample".to_owned()))?,
                memory_bytes: stats.memory_stats.usage.unwrap_or(0),
                sampled_at: chrono::Utc::now(),
                ..Default::default()
            }),
            Some(Err(e)) if is_not_found(&e) => {
                Err(RuntimeError::SandboxNotFound(handle.sandbox_ref()))
            }
            Some(Err(e)) => Err(map_bollard_err(e)),
            None => Err(RuntimeError::SandboxNotFound(handle.sandbox_ref())),
        }
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        let options = ListContainersOptions {
            all: true,
            filters: HashMap::from([(
                "label".to_owned(),
                vec![format!("{LABEL_WORKLOAD}={WORKLOAD_SANDBOX}")],
            )]),
            ..Default::default()
        };
        let call = self.client().list_containers(Some(options));
        let summaries = self.bounded("list_containers", call).await?;
        let mut out = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let labels = summary.labels.unwrap_or_default();
            let Some(key) = key_from_labels(&labels) else {
                warn!(
                    id = ?summary.id,
                    names = ?summary.names,
                    "skipping sandbox-labelled container with unreadable key labels"
                );
                continue;
            };
            let status = summary
                .state
                .as_deref()
                .and_then(|s| ContainerStateStatusEnum::from_str(s).ok());
            let backend_ref = container_name(&key);
            let state = map_status(status);
            // The list API omits `StartedAt`, so a running sandbox is inspected for it: the
            // FinOps meter keys a sandbox's minutes on it, and without it falls back to when
            // it was first seen.
            let started_at = if state == SandboxState::Running {
                match self.inspect(&backend_ref).await {
                    Ok(Some(info)) => started_at_of(&info),
                    _ => None,
                }
            } else {
                None
            };
            out.push(SandboxInfo {
                handle: SandboxHandle {
                    backend_ref,
                    key,
                    provider: SandboxProviderKind::Docker,
                },
                state,
                started_at,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{ContainerConfig, ContainerState, EndpointSettings, NetworkSettings};

    #[test]
    fn only_a_container_this_call_created_is_rolled_back() {
        assert!(rolls_back(true));
        assert!(!rolls_back(false));
    }

    #[test]
    fn started_at_is_read_from_the_inspect_state() {
        let info = |started: Option<&str>| ContainerInspectResponse {
            state: Some(ContainerState {
                started_at: started.map(str::to_owned),
                ..Default::default()
            }),
            ..Default::default()
        };
        let parsed = started_at_of(&info(Some("2026-07-21T10:00:05.123456789Z"))).unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-07-21T10:00:05.123456789+00:00");
        // Docker's "never started" sentinel, an absent value and garbage are all "no start time".
        assert_eq!(started_at_of(&info(Some("0001-01-01T00:00:00Z"))), None);
        assert_eq!(started_at_of(&info(None)), None);
        assert_eq!(started_at_of(&info(Some("yesterday"))), None);
        assert_eq!(started_at_of(&ContainerInspectResponse::default()), None);
    }

    fn id() -> Uuid {
        Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap()
    }

    fn spec(key: SandboxKey) -> SandboxSpec {
        SandboxSpec {
            key,
            image: "img:1".to_owned(),
            command: vec![],
            env: HashMap::from([("SECRET".to_owned(), "hunter2".to_owned())]),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8000, 9000],
            transport: TransportKind::WebSocket,
            image_pull_secret: None,
        }
    }

    fn agent_spec() -> SandboxSpec {
        spec(SandboxKey::Agent { agent_id: id() })
    }

    fn handle() -> SandboxHandle {
        SandboxHandle {
            key: SandboxKey::Agent { agent_id: id() },
            provider: SandboxProviderKind::Docker,
            backend_ref: "abc".to_owned(),
        }
    }

    /// An inspect response with our labels, optional network IP and optional host port.
    fn inspect(net_ip: Option<(&str, &str)>, host_port: Option<&str>) -> ContainerInspectResponse {
        let bound = |p: &str| {
            Some(vec![PortBinding {
                host_ip: None,
                host_port: Some(p.to_owned()),
            }])
        };
        ContainerInspectResponse {
            id: Some("cid".to_owned()),
            config: Some(ContainerConfig {
                labels: Some(sandbox_labels(&agent_spec())),
                ..Default::default()
            }),
            network_settings: Some(NetworkSettings {
                ports: host_port.map(|p| HashMap::from([("8000/tcp".to_owned(), bound(p))])),
                networks: net_ip.map(|(net, ip)| {
                    HashMap::from([(
                        net.to_owned(),
                        EndpointSettings {
                            ip_address: Some(ip.to_owned()),
                            ..Default::default()
                        },
                    )])
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn target() -> AttachTarget {
        AttachTarget {
            port: 8000,
            transport: TransportKind::Http,
        }
    }

    #[test]
    fn container_name_prefixes_key_name() {
        let key = SandboxKey::Session { session_id: id() };
        assert_eq!(
            container_name(&key),
            "nasiko-sbx-s-123e4567-e89b-12d3-a456-426614174000"
        );
    }

    #[test]
    fn labels_record_key_target_and_workload() {
        let labels = sandbox_labels(&agent_spec());
        assert_eq!(labels[LABEL_KIND], "agent");
        assert_eq!(labels[LABEL_WORKLOAD], "sandbox");
        assert_eq!(labels[LABEL_PORT], "8000");
        assert_eq!(labels[LABEL_TRANSPORT], "websocket");
    }

    #[test]
    fn labels_record_env_keys_but_never_values() {
        let labels = sandbox_labels(&agent_spec());
        assert_eq!(labels[LABEL_ENV_KEYS], r#"["SECRET"]"#);
        assert!(labels.values().all(|v| !v.contains("hunter2")));
    }

    #[test]
    fn every_key_variant_round_trips_through_labels() {
        let keys = [
            SandboxKey::Session { session_id: id() },
            SandboxKey::Agent { agent_id: id() },
            SandboxKey::AgentContext {
                agent_id: id(),
                context_id: "Ünï ctx / with spaces".to_owned(),
            },
        ];
        for key in keys {
            let labels = sandbox_labels(&spec(key.clone()));
            assert_eq!(key_from_labels(&labels), Some(key));
        }
    }

    #[test]
    fn corrupt_key_labels_yield_none() {
        let mut labels = sandbox_labels(&agent_spec());
        labels.insert(LABEL_KIND.to_owned(), "bogus".to_owned());
        assert_eq!(key_from_labels(&labels), None);
        labels.remove(LABEL_ID);
        assert_eq!(key_from_labels(&labels), None);
    }

    #[test]
    fn running_and_restarting_map_to_running_and_starting() {
        use ContainerStateStatusEnum as S;
        assert_eq!(map_status(Some(S::RUNNING)), SandboxState::Running);
        assert_eq!(map_status(Some(S::RESTARTING)), SandboxState::Starting);
    }

    #[test]
    fn stopped_like_states_are_suspended_and_removal_is_absent() {
        use ContainerStateStatusEnum as S;
        for s in [S::CREATED, S::EXITED, S::PAUSED, S::DEAD] {
            assert_eq!(map_status(Some(s)), SandboxState::Suspended);
        }
        assert_eq!(map_status(Some(S::REMOVING)), SandboxState::Absent);
        assert_eq!(map_status(None), SandboxState::Absent);
    }

    #[test]
    fn list_state_strings_parse_through_the_same_mapping() {
        let parse = |s: &str| ContainerStateStatusEnum::from_str(s).ok();
        assert_eq!(map_status(parse("exited")), SandboxState::Suspended);
        assert_eq!(map_status(parse("running")), SandboxState::Running);
        assert_eq!(map_status(parse("nonsense")), SandboxState::Absent);
    }

    #[test]
    fn stats_gate_allows_only_running() {
        assert!(require_running_for_stats(Some(SandboxState::Running), &handle()).is_ok());
    }

    #[test]
    fn stats_gate_maps_stopped_and_missing_to_contract_errors() {
        for st in [SandboxState::Suspended, SandboxState::Starting] {
            assert!(matches!(
                require_running_for_stats(Some(st), &handle()),
                Err(RuntimeError::SandboxNotRunning(_))
            ));
        }
        for st in [None, Some(SandboxState::Absent)] {
            assert!(matches!(
                require_running_for_stats(st, &handle()),
                Err(RuntimeError::SandboxNotFound(_))
            ));
        }
    }

    #[test]
    fn urls_use_the_address_and_acp_path_for_websocket() {
        let addr = Addr {
            host: "127.0.0.1".to_owned(),
            port: "49153".to_owned(),
        };
        assert_eq!(
            transport_url(TransportKind::Http, &addr),
            SandboxTransport::Http {
                url: "http://127.0.0.1:49153".to_owned()
            }
        );
        assert_eq!(
            transport_url(TransportKind::WebSocket, &addr),
            SandboxTransport::WebSocket {
                url: "ws://127.0.0.1:49153/acp".to_owned(),
                headers: vec![],
                tls: None,
            }
        );
    }

    #[test]
    fn connect_host_maps_wildcards_to_loopback() {
        for wildcard in ["0.0.0.0", "::", ""] {
            assert_eq!(connect_host(wildcard), "127.0.0.1");
        }
        assert_eq!(connect_host("10.1.2.3"), "10.1.2.3");
    }

    #[test]
    fn endpoint_prefers_the_network_ip_and_container_port() {
        let info = inspect(Some(("nasiko_default", "172.18.0.5")), Some("49153"));
        let addr = endpoint_addr(&info, &target(), Some("nasiko_default"), "0.0.0.0");
        assert_eq!(
            addr,
            Some(Addr {
                host: "172.18.0.5".to_owned(),
                port: "8000".to_owned()
            })
        );
    }

    #[test]
    fn endpoint_falls_back_to_the_host_binding_without_a_usable_network() {
        let info = inspect(Some(("other", "172.18.0.5")), Some("49153"));
        let expected = Some(Addr {
            host: "127.0.0.1".to_owned(),
            port: "49153".to_owned(),
        });
        // No network configured, and configured but the container is not on it.
        assert_eq!(endpoint_addr(&info, &target(), None, "0.0.0.0"), expected);
        assert_eq!(
            endpoint_addr(&info, &target(), Some("nasiko_default"), "0.0.0.0"),
            expected
        );
    }

    #[test]
    fn endpoint_is_none_until_the_port_is_bound() {
        let info = inspect(None, Some(""));
        assert_eq!(endpoint_addr(&info, &target(), None, "127.0.0.1"), None);
    }

    #[test]
    fn host_port_is_found_by_container_port_and_ignores_unbound() {
        let bound = |p: &str| {
            Some(vec![PortBinding {
                host_ip: None,
                host_port: Some(p.to_owned()),
            }])
        };
        let ports = HashMap::from([
            ("8000/tcp".to_owned(), bound("")),
            ("9000/tcp".to_owned(), bound("50000")),
        ]);
        assert_eq!(host_port_for(&ports, 8000), None);
        assert_eq!(host_port_for(&ports, 9000).as_deref(), Some("50000"));
        assert_eq!(host_port_for(&ports, 1), None);
    }

    #[test]
    fn empty_command_leaves_image_default() {
        let cfg = container_config(&agent_spec(), &DockerRuntimeConfig::default());
        assert!(cfg.cmd.is_none());
        assert!(cfg.user.is_none());
    }

    fn workspace_spec() -> SandboxSpec {
        let mut s = agent_spec();
        s.workspace = Some(crate::WorkspaceRef {
            owner_id: id(),
            container_id: crate::ContainerId::from_uuid(id()),
        });
        s
    }

    #[test]
    fn workspace_runs_as_nobody_and_does_not_override_home() {
        let cfg = container_config(&workspace_spec(), &DockerRuntimeConfig::default());
        assert_eq!(cfg.user.as_deref(), Some("65534:65534"));
        assert!(!cfg.env.unwrap().iter().any(|e| e.starts_with("HOME=")));
    }

    #[test]
    fn an_agent_workspace_mounts_the_agents_own_directory() {
        let cfg = container_config(&workspace_spec(), &DockerRuntimeConfig::default());
        let mounts = cfg.host_config.unwrap().mounts.unwrap();
        let sub = mounts[0].volume_options.as_ref().unwrap().subpath.clone();
        assert_eq!(sub.unwrap(), format!("{0}/{0}", id()));
        assert_eq!(mounts[0].target.as_deref(), Some("/workspace"));
    }

    fn session_key() -> SandboxKey {
        SandboxKey::Session { session_id: id() }
    }

    #[test]
    fn a_session_sandbox_mounts_its_own_scratch_volume_after_the_workspace() {
        let mut s = workspace_spec();
        s.key = session_key();
        let cfg = container_config(&s, &DockerRuntimeConfig::default());
        let mounts = cfg.host_config.unwrap().mounts.unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].target.as_deref(), Some("/workspace"));
        let scratch = &mounts[1];
        assert_eq!(scratch.target.as_deref(), Some("/scratch"));
        assert_eq!(scratch.typ, Some(bollard::models::MountTypeEnum::VOLUME));
        let volume = format!("nasiko-sbx-s-{}-scratch", id());
        assert_eq!(scratch.source.as_deref(), Some(volume.as_str()));
        let subpath = scratch.volume_options.as_ref().unwrap().subpath.clone();
        assert_eq!(subpath.unwrap(), format!("s-{}", id()));
    }

    #[test]
    fn a_session_sandbox_without_a_workspace_still_gets_scratch() {
        let cfg = container_config(&spec(session_key()), &DockerRuntimeConfig::default());
        let mounts = cfg.host_config.unwrap().mounts.unwrap();
        let targets: Vec<_> = mounts.iter().map(|m| m.target.as_deref()).collect();
        assert_eq!(targets, [Some("/scratch")]);
    }

    #[test]
    fn an_agent_sandbox_gets_no_scratch() {
        let cfg = container_config(&agent_spec(), &DockerRuntimeConfig::default());
        assert!(cfg.host_config.unwrap().mounts.is_none());
        let cfg = container_config(&workspace_spec(), &DockerRuntimeConfig::default());
        let mounts = cfg.host_config.unwrap().mounts.unwrap();
        assert!(
            mounts
                .iter()
                .all(|m| m.target.as_deref() != Some("/scratch"))
        );
    }

    #[test]
    fn a_scratch_volume_carries_its_sandboxs_key_and_kind() {
        let labels = scratch_volume_labels(&session_key());
        assert_eq!(labels[LABEL_KEY], format!("s-{}", id()));
        assert_eq!(labels[LABEL_KIND], "session");
        assert_eq!(labels.len(), 2);
    }

    #[test]
    fn each_sandbox_has_a_scratch_volume_of_its_own() {
        let other = SandboxKey::Session {
            session_id: Uuid::nil(),
        };
        assert_ne!(scratch_volume(&session_key()), scratch_volume(&other));
        assert_eq!(
            scratch_volume(&session_key()),
            format!("{}-scratch", container_name(&session_key()))
        );
    }

    #[test]
    fn workspace_subpath_is_per_key_except_for_the_agent_itself() {
        let cid = crate::ContainerId::new("agent-1");
        let session = SandboxKey::Session { session_id: id() };
        assert_eq!(
            workspace_subpath(id(), &cid, &session),
            format!("{0}/agent-1/s-{0}", id())
        );
        let agent = SandboxKey::Agent { agent_id: id() };
        assert_eq!(
            workspace_subpath(id(), &cid, &agent),
            format!("{}/agent-1", id())
        );
    }

    #[test]
    fn runtime_class_reaches_the_host_config() {
        let mut s = agent_spec();
        s.runtime_class = Some("runsc".to_owned());
        let host = sandbox_host_config(&s, &DockerRuntimeConfig::default());
        assert_eq!(host.runtime.as_deref(), Some("runsc"));
    }

    #[test]
    fn target_requires_valid_labels() {
        let labels = sandbox_labels(&agent_spec());
        let t = AttachTarget::from_labels(&labels).unwrap();
        assert_eq!((t.port, t.transport), (8000, TransportKind::WebSocket));
        let mut bad = labels;
        bad.insert(LABEL_PORT.to_owned(), "x".to_owned());
        assert!(AttachTarget::from_labels(&bad).is_err());
    }

    #[test]
    fn target_without_any_labels_is_an_error() {
        let info = ContainerInspectResponse::default();
        assert!(AttachTarget::from_inspect(&info).is_err());
    }

    #[test]
    fn millicores_is_fifty_percent_of_one_cpu_as_500() {
        // 50 of 100 system ticks on 1 cpu = 50% = 500 millicores.
        assert_eq!(
            millicores_from((150, 100), (1100, 1000), Some(1), 0),
            Some(500)
        );
    }

    #[test]
    fn millicores_falls_back_to_percpu_length_and_never_invents_zero() {
        assert_eq!(
            millicores_from((150, 100), (1100, 1000), None, 2),
            Some(1000)
        );
        assert_eq!(millicores_from((150, 100), (1100, 1000), None, 0), None);
        assert_eq!(millicores_from((150, 100), (1000, 1000), Some(1), 0), None);
    }

    #[test]
    fn drift_flags_changed_image_and_env_keys() {
        let labels = sandbox_labels(&agent_spec());
        assert!(spec_drift(Some(&labels), &agent_spec()).is_empty());
        let mut changed = agent_spec();
        changed.image = "img:2".to_owned();
        changed.env.insert("EXTRA".to_owned(), "1".to_owned());
        assert_eq!(spec_drift(Some(&labels), &changed), vec!["image", "env"]);
    }

    #[test]
    fn exited_container_fails_fast_with_its_status() {
        let info = ContainerInspectResponse {
            state: Some(ContainerState {
                status: Some(ContainerStateStatusEnum::EXITED),
                exit_code: Some(3),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = fail_if_exited("sbx", &info).unwrap_err();
        assert!(err.to_string().contains("exited with status 3"));
        assert!(fail_if_exited("sbx", &inspect(None, None)).is_ok());
    }
}
