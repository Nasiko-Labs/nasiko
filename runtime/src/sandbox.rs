//! Session-shaped sandboxes, beside the deployment-shaped [`crate::ContainerRuntime`].
//!
//! A `ContainerRuntime` deploys a long-lived service per agent. A [`SandboxProvider`] hands out
//! a sandbox that exists only while it is serving: acquired on demand, suspended when idle,
//! resumed by the next request. Backends (Docker, bare pod, shared runtime pods, per-user harness
//! agents) implement the trait; this module holds only the trait and its types.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{ResourceLimits, Result, RuntimeError, WorkspaceRef};

/// Which backend owns a sandbox. Persisted as the prefix of [`SandboxHandle::sandbox_ref`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SandboxProviderKind {
    /// The Docker backend (OSS).
    Docker,
    /// The bare Kubernetes pod backend.
    Pod,
    /// Shared runtime pods: many sandboxes packed onto replicas of one StatefulSet.
    SharedPod,
    /// Each user's own harness agent: every session of one user and harness is a process in
    /// that agent's workload.
    HarnessAgent,
    /// A test double. The composition root rejects a stored ref whose kind differs from the
    /// wired provider, so a `fake:` row cannot be acted on in production.
    Fake,
}

impl std::fmt::Display for SandboxProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Docker => "docker",
            Self::Pod => "pod",
            Self::SharedPod => "shared-pod",
            Self::HarnessAgent => "harness-agent",
            Self::Fake => "fake",
        })
    }
}

impl std::str::FromStr for SandboxProviderKind {
    type Err = RuntimeError;

    /// An unknown name is a corrupt persisted value, not a bad caller spec, so the error is
    /// [`RuntimeError::Internal`].
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "docker" => Ok(Self::Docker),
            "pod" => Ok(Self::Pod),
            "shared-pod" => Ok(Self::SharedPod),
            "harness-agent" => Ok(Self::HarnessAgent),
            "fake" => Ok(Self::Fake),
            other => Err(RuntimeError::Internal(format!(
                "unknown sandbox provider: {other}"
            ))),
        }
    }
}

/// What a sandbox is keyed by. Stable, unique, and safe as a container/pod name fragment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SandboxKey {
    /// One per user session of a harness (Claude Code / Codex / OpenCode).
    Session {
        /// The harness session.
        session_id: Uuid,
    },
    /// One per agent, shared by all callers (a framework A2A agent).
    Agent {
        /// The agent.
        agent_id: Uuid,
    },
    /// One per (agent, A2A contextId) for agents that keep per-conversation state in memory.
    ///
    /// Deliberately not user-scoped. SAFE ONLY IF the caller authorises contextId ownership before
    /// acquiring (the caller must own the `chat_sessions` row for that contextId). The server no
    /// longer acquires this key: it parks an agent by scaling its own workload, and only releases
    /// sandboxes left under it from before.
    AgentContext {
        /// The agent.
        agent_id: Uuid,
        /// The A2A contextId; arbitrary caller text, so it is hashed into the name.
        context_id: String,
    },
}

impl SandboxKey {
    /// Canonical name: `s-<uuid>`, `a-<uuid>` or `ac-<uuid>-<16 hex of FNV-1a(context_id)>`.
    ///
    /// At most 56 chars, `[a-z0-9-]`; fits a Kubernetes label VALUE only unprefixed — put
    /// prefixes on object names, never on label values (63-char cap). The context id is
    /// hashed because it is unbounded caller text.
    pub fn name(&self) -> String {
        match self {
            Self::Session { session_id } => format!("s-{session_id}"),
            Self::Agent { agent_id } => format!("a-{agent_id}"),
            Self::AgentContext {
                agent_id,
                context_id,
            } => format!("ac-{agent_id}-{:016x}", fnv1a_64(context_id.as_bytes())),
        }
    }

    /// Whether a sandbox under this key gets a scratch area at [`HARNESS_SCRATCH_DIR`]. A harness
    /// session's does: its package installs and builds are what the area is for. A framework
    /// agent's does not, so its sandbox reserves no local disk for one.
    pub fn has_scratch(&self) -> bool {
        matches!(self, Self::Session { .. })
    }
}

/// FNV-1a 64-bit. Hand-rolled because `DefaultHasher` is not stable across runs, and a name
/// derived from it would orphan sandboxes on restart.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(PRIME)
    })
}

/// Where a sandbox's persistent workspace is mounted, and the directory an ACP session starts
/// in. One definition for every provider and for the turn path that opens the session there.
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// Where a sandbox whose key [has scratch](SandboxKey::has_scratch) keeps what it can regenerate:
/// package caches and build output, which a harness session's env points here, one directory per
/// sandbox key (`<HARNESS_SCRATCH_DIR>/<key name>`) so sessions sharing a pod keep apart.
///
/// Node-local storage, never the durable workspace (which may be a network filesystem that
/// small-file churn would saturate), so its contents go when the sandbox's pod or container
/// does. Every provider mounts it, writable by the sandbox's user. One definition for the
/// providers and for the server that builds the env.
pub const HARNESS_SCRATCH_DIR: &str = "/scratch";

/// Everything a provider needs to create a sandbox.
///
/// `Debug` prints env as keys only: the values carry tokens.
#[derive(Clone)]
pub struct SandboxSpec {
    /// What the sandbox is keyed by.
    pub key: SandboxKey,
    /// Fully-qualified OCI image.
    pub image: String,
    /// argv to run; empty = image default entrypoint.
    pub command: Vec<String>,
    /// Environment variables for the sandbox process.
    pub env: HashMap<String, String>,
    /// None → the runtime's default (same semantics as `DeploymentSpec::resources`).
    pub resources: Option<ResourceLimits>,
    /// Persistent per-key directory; None = ephemeral filesystem.
    pub workspace: Option<WorkspaceRef>,
    /// Kubernetes runtimeClassName or equivalent (e.g. "gvisor"); None = backend default.
    pub runtime_class: Option<String>,
    /// Ports the sandbox listens on; must be non-empty and non-zero.
    pub ports: Vec<u16>,
    /// How callers talk to the sandbox. `attach` builds the URL for the FIRST entry of
    /// `ports` using this kind.
    pub transport: TransportKind,
    /// Name of the Kubernetes Secret that authenticates the image pull; `None` pulls anonymously.
    pub image_pull_secret: Option<String>,
}

impl std::fmt::Debug for SandboxSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env: std::collections::BTreeMap<&String, &str> =
            self.env.keys().map(|k| (k, "<redacted>")).collect();
        f.debug_struct("SandboxSpec")
            .field("key", &self.key)
            .field("image", &self.image)
            .field("command", &self.command)
            .field("env", &env)
            .field("resources", &self.resources)
            .field("workspace", &self.workspace)
            .field("runtime_class", &self.runtime_class)
            .field("ports", &self.ports)
            .field("transport", &self.transport)
            .finish()
    }
}

/// The protocol a sandbox's first port speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// Plain HTTP (A2A agents).
    Http,
    /// WebSocket carrying newline-delimited JSON-RPC (ACP harness shim).
    WebSocket,
}

/// Longest `AgentContext::context_id` accepted; backends store it verbatim (Docker label).
const MAX_CONTEXT_ID_BYTES: usize = 1024;

impl SandboxSpec {
    /// Rejects a spec no backend could run: empty image, invalid resources, invalid workspace
    /// container id, empty `ports`, port 0, or an env key that is empty or contains `=`.
    pub fn validate(&self) -> Result<()> {
        if self.image.is_empty() {
            return Err(invalid("image must not be empty"));
        }
        if let Some(resources) = &self.resources {
            resources.validate()?;
        }
        if let Some(workspace) = &self.workspace {
            workspace.container_id.validate()?;
        }
        if self.ports.is_empty() {
            return Err(invalid("ports must not be empty"));
        }
        if self.ports.contains(&0) {
            return Err(invalid("port 0 is not a valid container port"));
        }
        if let SandboxKey::AgentContext { context_id, .. } = &self.key
            && context_id.len() > MAX_CONTEXT_ID_BYTES
        {
            return Err(invalid(&format!(
                "context_id exceeds {MAX_CONTEXT_ID_BYTES} bytes"
            )));
        }
        if let Some(bad) = self.env.keys().find(|k| k.is_empty() || k.contains('=')) {
            return Err(invalid(&format!("invalid env key {bad:?}")));
        }
        Ok(())
    }
}

fn invalid(msg: &str) -> RuntimeError {
    RuntimeError::InvalidSpec(msg.to_owned())
}

/// Identifies one live or suspended sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxHandle {
    /// The key the sandbox was acquired for.
    pub key: SandboxKey,
    /// Which backend owns it.
    pub provider: SandboxProviderKind,
    /// The backend's own identifier (container id, pod name).
    pub backend_ref: String,
}

impl SandboxHandle {
    /// `"<provider>:<backend_ref>"`, the string persisted in the database.
    pub fn sandbox_ref(&self) -> String {
        format!("{}:{}", self.provider, self.backend_ref)
    }

    /// Parses a persisted `"<provider>:<backend_ref>"`. The key is not in the string, so the
    /// caller supplies it. Meant for refs read back from storage: a missing colon, unknown
    /// provider or empty backend ref means a corrupt row and is [`RuntimeError::Internal`].
    pub fn parse(sandbox_ref: &str, key: SandboxKey) -> Result<Self> {
        let (name, backend_ref) = sandbox_ref.split_once(':').ok_or_else(|| {
            RuntimeError::Internal(format!("malformed sandbox ref: {sandbox_ref:?}"))
        })?;
        if backend_ref.is_empty() {
            return Err(RuntimeError::Internal(format!(
                "sandbox ref has an empty backend ref: {sandbox_ref:?}"
            )));
        }
        Ok(Self {
            key,
            provider: name.parse()?,
            backend_ref: backend_ref.to_owned(),
        })
    }
}

/// TLS material for reaching a sandbox endpoint, all PEM text.
#[derive(Clone, PartialEq, Eq)]
pub struct TransportTls {
    /// The CA certificate(s) that sign the endpoint, trusted besides the public roots. Empty
    /// means the public roots alone (the endpoint wants only a client identity).
    pub ca_pem: String,
    /// The client certificate chain the endpoint requires, when it requires one.
    pub client_cert_pem: Option<String>,
    /// The private key of `client_cert_pem`.
    pub client_key_pem: Option<String>,
}

impl std::fmt::Debug for TransportTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportTls")
            .field("ca_pem", &self.ca_pem)
            .field("client_cert_pem", &self.client_cert_pem)
            .field(
                "client_key_pem",
                &self.client_key_pem.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// How to talk to a running sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxTransport {
    /// Plain HTTP (A2A agents): base URL incl. scheme, host, port.
    Http {
        /// Base URL.
        url: String,
    },
    /// WebSocket carrying newline-delimited JSON-RPC (ACP harness shim).
    WebSocket {
        /// Full `ws://` or `wss://` URL.
        url: String,
        /// Headers every request to this sandbox must carry (routing, never credentials):
        /// the attach upgrade and every control call derived from `url`. Empty for providers
        /// that address a sandbox by URL alone.
        headers: Vec<(String, String)>,
        /// TLS material for a `wss://` endpoint a public CA does not sign, or that demands a
        /// client identity; the attach and every control call must use it. `None` for plain
        /// `ws://` and for endpoints reachable with the public roots alone.
        tls: Option<TransportTls>,
    },
}

impl SandboxTransport {
    /// The URL, whatever the variant.
    pub fn url(&self) -> &str {
        match self {
            Self::Http { url } | Self::WebSocket { url, .. } => url,
        }
    }

    /// Routing headers (empty for `Http`).
    pub fn headers(&self) -> &[(String, String)] {
        match self {
            Self::WebSocket { headers, .. } => headers,
            Self::Http { .. } => &[],
        }
    }
}

/// Lifecycle state of a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxState {
    /// Created, not yet ready to attach.
    Starting,
    /// Ready to attach.
    Running,
    /// Frozen or stopped with state kept; `acquire` brings it back.
    Suspended,
    /// No such sandbox.
    Absent,
}

/// One resource-usage sample.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SandboxStats {
    /// CPU in use, in millicores.
    pub cpu_millicores: u32,
    /// Memory in use, in bytes.
    pub memory_bytes: u64,
    /// When the sample was taken (`Default`: the Unix epoch, meaning "no sample").
    pub sampled_at: DateTime<Utc>,
    /// Live processes in the sandbox, when the backend can count them.
    pub processes: Option<u32>,
    /// The backend asked the sandbox to go away (worker drain); suspend it as soon as no turn
    /// holds it.
    pub evicting: bool,
}

/// A provider's view of one sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxInfo {
    /// The sandbox.
    pub handle: SandboxHandle,
    /// Its current state.
    pub state: SandboxState,
    /// When it last started, if the backend reports it.
    pub started_at: Option<DateTime<Utc>>,
}

/// Provisions sandboxes that exist only while serving.
///
/// Every method is idempotent. Object-safe via `async_trait`, for use as
/// `Arc<dyn SandboxProvider>`. Every method is required on purpose. A defaulted method added
/// later MUST carry the RUN-1 forwarding warning as `ContainerRuntime` does (see
/// `InstrumentedRuntime`).
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    /// Which backend this is.
    fn kind(&self) -> SandboxProviderKind;

    /// Whether `acquire` can succeed for `key`. Equal to [`Self::is_available`] unless the
    /// provider routes keys to different backends (see `KeyRoutedSandboxProvider`), where one
    /// side can be down while the other serves.
    ///
    /// RUN-1: a decorator wrapping a provider MUST forward this method to its inner provider.
    /// The default is `self.is_available()`, so a wrapper that forgets silently reports the
    /// whole composite's availability for every key.
    fn is_available_for(&self, _key: &SandboxKey) -> bool {
        self.is_available()
    }

    /// Whether any key can be served. `false` only for the placeholder a server wires when the
    /// configured backend cannot be reached or is not built yet. Callers that act on one key use
    /// [`Self::is_available_for`]; an agent whose key is unavailable is treated as resident.
    fn is_available(&self) -> bool {
        true
    }

    /// Create-or-resume; idempotent for the same key; a resumed sandbox keeps the image,
    /// command, ports, transport, the runtime class the spec named, workspace and resources it
    /// was created with — a caller that needs a new image must `release` first; returns once
    /// Running, or `RuntimeError::Timeout` after the backend's readiness budget. Validates the
    /// spec first. A backend-level default runtime class (one the spec did not name) is applied
    /// at every creation, so a changed default reaches an existing sandbox on its next resume.
    ///
    /// Env is the one field a backend may take from the *resuming* spec: a backend that keeps
    /// the original object (Docker) keeps its env, one that recreates it from a snapshot that
    /// never stores values (bare pod) applies the new spec's env. Callers therefore pass the
    /// full, stable env on every `acquire`, and rely on a resume to change it only where
    /// [`Self::resume_takes_new_env`] says so.
    ///
    /// The handle, including `backend_ref`, is stable for the life of the key: suspend/resume
    /// and repeated acquire return an equal handle. A backend that recreates its object must
    /// use a deterministic name derived from `SandboxKey::name()`.
    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle>;

    /// How to reach a sandbox that is Running. Absent → `Err(RuntimeError::SandboxNotFound)`;
    /// Starting or Suspended → `Err(RuntimeError::SandboxNotRunning)`. Callers always
    /// `acquire` first; `attach` never resumes.
    ///
    /// The URL's scheme follows `transport`; its host and port are wherever the backend
    /// publishes `ports[0]`, which may be NAT'ed (Docker publishes to an ephemeral host port).
    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport>;

    /// Current state. A sandbox that does not exist is reported as [`SandboxState::Absent`],
    /// not as an error.
    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState>;

    /// Freeze or stop the sandbox, keeping its state so `acquire` can bring it back.
    /// Idempotent; Ok on Absent.
    async fn suspend(&self, handle: &SandboxHandle) -> Result<()>;

    /// Destroy the sandbox and (for ephemeral specs) its state. Idempotent; Ok on Absent.
    async fn release(&self, handle: &SandboxHandle) -> Result<()>;

    /// Whether the sandbox `handle` names, once `suspend`ed, comes back from the next `acquire`
    /// of its key running on the env of that acquire (and of the attach that follows) rather
    /// than on the env it was created with. True for a backend that starts a resumed sandbox
    /// afresh from the resuming spec (the bare pod) or whose sandbox takes its settings from the
    /// caller at attach (shared runtime pods, configured over the shim's handshake); false for
    /// one that keeps the object it created (Docker keeps the container and its env), and for a
    /// sandbox the next `acquire` of its key would not resume at all.
    ///
    /// A caller that must change a sandbox's env suspends it where this holds and releases it
    /// only where it does not, because `release` may take the sandbox's files with it (shared
    /// runtime pods purge the session's directory). An error means the provider could not tell
    /// (a routing lookup failed); the caller then neither suspends nor releases on a guess.
    ///
    /// RUN-1: a decorator wrapping a provider MUST forward this method to whichever provider
    /// holds `handle`, as it does `suspend` and `release`. It has no default on purpose: a
    /// wrapper answering for itself would turn a suspend into a release, and every env change
    /// into lost files.
    async fn resume_takes_new_env(&self, handle: &SandboxHandle) -> Result<bool>;

    /// Current resource usage, for the idle reaper's CPU-quiescence check. Absent →
    /// `SandboxNotFound`; Suspended or Starting → `SandboxNotRunning`; Running → a reading.
    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats>;

    /// Every sandbox this provider knows about, any state.
    async fn list(&self) -> Result<Vec<SandboxInfo>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails to compile when a kind is added, pointing at `ALL_KINDS`, which must list it too.
    fn kind_listing_is_exhaustive(k: SandboxProviderKind) {
        match k {
            SandboxProviderKind::Docker
            | SandboxProviderKind::Pod
            | SandboxProviderKind::SharedPod
            | SandboxProviderKind::HarnessAgent
            | SandboxProviderKind::Fake => (),
        }
    }

    #[test]
    fn every_listed_kind_is_matched_exhaustively() {
        for kind in ALL_KINDS {
            kind_listing_is_exhaustive(kind);
        }
    }

    #[test]
    fn shared_pod_kind_displays_and_parses_as_shared_pod() {
        assert_eq!(SandboxProviderKind::SharedPod.to_string(), "shared-pod");
        assert_eq!(
            "shared-pod".parse::<SandboxProviderKind>().unwrap(),
            SandboxProviderKind::SharedPod
        );
    }

    #[test]
    fn transport_tls_debug_redacts_the_key() {
        let tls = TransportTls {
            ca_pem: "ca".into(),
            client_cert_pem: Some("cert".into()),
            client_key_pem: Some("SECRET-KEY".into()),
        };
        let shown = format!("{tls:?}");
        assert!(!shown.contains("SECRET-KEY"), "{shown}");
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn websocket_transport_carries_headers_and_defaults_to_none() {
        let t = SandboxTransport::WebSocket {
            url: "ws://h/acp".into(),
            headers: vec![],
            tls: None,
        };
        assert_eq!(t.headers(), &[] as &[(String, String)]);
        let t = SandboxTransport::WebSocket {
            url: "ws://h/acp".into(),
            headers: vec![("ate-target-actor".into(), "ns/s-1".into())],
            tls: None,
        };
        assert_eq!(t.headers().len(), 1);
        assert_eq!(t.url(), "ws://h/acp");
        assert_eq!(
            SandboxTransport::Http {
                url: "http://h".into()
            }
            .headers()
            .len(),
            0
        );
    }

    #[test]
    fn stats_default_is_idle_and_not_evicting() {
        let s = SandboxStats::default();
        assert_eq!(
            (s.cpu_millicores, s.memory_bytes, s.processes, s.evicting),
            (0, 0, None, false)
        );
    }

    const ALL_KINDS: [SandboxProviderKind; 5] = [
        SandboxProviderKind::Docker,
        SandboxProviderKind::Pod,
        SandboxProviderKind::SharedPod,
        SandboxProviderKind::HarnessAgent,
        SandboxProviderKind::Fake,
    ];

    fn id() -> Uuid {
        Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap()
    }

    fn ctx_key(context: &str) -> SandboxKey {
        SandboxKey::AgentContext {
            agent_id: id(),
            context_id: context.to_owned(),
        }
    }

    fn spec() -> SandboxSpec {
        SandboxSpec {
            key: SandboxKey::Agent { agent_id: id() },
            image: "registry.example/agent:1".to_owned(),
            command: vec![],
            env: HashMap::new(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![8080],
            transport: TransportKind::Http,
            image_pull_secret: None,
        }
    }

    fn is_name_safe(s: &str) -> bool {
        s.len() <= 63
            && !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn only_a_session_key_has_scratch() {
        assert!(SandboxKey::Session { session_id: id() }.has_scratch());
        assert!(!SandboxKey::Agent { agent_id: id() }.has_scratch());
        assert!(!ctx_key("c1").has_scratch());
    }

    #[test]
    fn key_formats_session_and_agent() {
        let s = SandboxKey::Session { session_id: id() }.name();
        let a = SandboxKey::Agent { agent_id: id() }.name();
        assert_eq!(s, "s-123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(a, "a-123e4567-e89b-12d3-a456-426614174000");
    }

    #[test]
    fn key_context_has_fixed_width_hash_and_known_value() {
        // FNV-1a 64 of "" is the offset basis.
        assert_eq!(
            ctx_key("").name(),
            "ac-123e4567-e89b-12d3-a456-426614174000-cbf29ce484222325"
        );
        // FNV-1a 64 of "a" (published test vector).
        assert!(ctx_key("a").name().ends_with("-af63dc4c8601ec8c"));
    }

    #[test]
    fn key_context_stays_name_safe_for_long_and_odd_input() {
        let long = "X".repeat(500);
        for ctx in [long.as_str(), "ünï/cödé :: ctx", ""] {
            let name = ctx_key(ctx).name();
            assert!(is_name_safe(&name), "{name}");
            assert_eq!(name.len(), 56);
        }
        assert!(is_name_safe(
            &SandboxKey::Session { session_id: id() }.name()
        ));
        assert!(is_name_safe(&SandboxKey::Agent { agent_id: id() }.name()));
    }

    #[test]
    fn key_hash_is_deterministic_and_distinguishes_contexts() {
        assert_eq!(ctx_key("c1").name(), ctx_key("c1").name());
        assert_ne!(ctx_key("c1").name(), ctx_key("c2").name());
    }

    #[test]
    fn debug_redacts_env_values_but_keeps_keys() {
        let mut s = spec();
        s.env.insert("TOKEN".to_owned(), "s3cret-value".to_owned());
        let shown = format!("{s:?}");
        assert!(shown.contains("TOKEN") && shown.contains("<redacted>"));
        assert!(!shown.contains("s3cret-value"));
    }

    #[test]
    fn validate_accepts_minimal_spec() {
        assert!(spec().validate().is_ok());
    }

    #[test]
    fn validate_caps_context_id_length() {
        let mut s = spec();
        s.key = ctx_key(&"x".repeat(1024));
        assert!(s.validate().is_ok());
        s.key = ctx_key(&"x".repeat(1025));
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
    }

    #[test]
    fn validate_rejects_empty_image() {
        let mut s = spec();
        s.image.clear();
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
    }

    #[test]
    fn validate_rejects_bad_env_keys() {
        for key in ["", "A=B"] {
            let mut s = spec();
            s.env.insert(key.to_owned(), "v".to_owned());
            assert!(
                matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))),
                "{key:?}"
            );
        }
        let mut ok = spec();
        ok.env.insert("GOOD_KEY".to_owned(), "a=b".to_owned());
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn validate_rejects_port_zero_and_empty_ports() {
        let mut s = spec();
        s.ports = vec![8080, 0];
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
        s.ports = vec![];
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
        s.ports = (1..=40).collect();
        assert!(s.validate().is_ok(), "no port-count cap");
    }

    #[test]
    fn validate_checks_workspace_container_id() {
        let mut s = spec();
        s.workspace = Some(WorkspaceRef {
            owner_id: id(),
            container_id: crate::ContainerId::new("bad id/with spaces"),
        });
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
        s.workspace = Some(WorkspaceRef {
            owner_id: id(),
            container_id: crate::ContainerId::from_uuid(id()),
        });
        assert!(s.validate().is_ok());
    }

    #[test]
    fn provider_kind_display_and_from_str_round_trip() {
        for kind in ALL_KINDS {
            assert_eq!(
                kind.to_string().parse::<SandboxProviderKind>().unwrap(),
                kind
            );
        }
        assert_eq!(SandboxProviderKind::SharedPod.to_string(), "shared-pod");
        assert_eq!(
            SandboxProviderKind::HarnessAgent.to_string(),
            "harness-agent"
        );
        assert!(matches!(
            "vmware".parse::<SandboxProviderKind>(),
            Err(RuntimeError::Internal(_))
        ));
    }

    #[test]
    fn validate_rejects_invalid_resources() {
        let mut s = spec();
        s.resources = Some(ResourceLimits {
            memory: "512Mi".to_owned(),
            cpu_milli: 0,
            cpu_request_milli: None,
        });
        assert!(matches!(s.validate(), Err(RuntimeError::InvalidSpec(_))));
        s.resources = Some(ResourceLimits::default());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn handle_ref_round_trips_for_every_provider() {
        let key = SandboxKey::Session { session_id: id() };
        for provider in ALL_KINDS {
            let handle = SandboxHandle {
                key: key.clone(),
                provider,
                backend_ref: "abc:123".to_owned(),
            };
            let text = handle.sandbox_ref();
            assert_eq!(text, format!("{provider}:abc:123"));
            assert_eq!(SandboxHandle::parse(&text, key.clone()).unwrap(), handle);
        }
    }

    #[test]
    fn handle_parse_rejects_unknown_provider_missing_colon_and_empty_ref() {
        let key = || SandboxKey::Agent { agent_id: id() };
        for bad in ["vmware:abc", "dockerabc", "docker:", ""] {
            assert!(
                matches!(
                    SandboxHandle::parse(bad, key()),
                    Err(RuntimeError::Internal(_))
                ),
                "{bad:?}"
            );
        }
    }
}
