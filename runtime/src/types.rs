use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Result, RuntimeError};

/// Opaque identifier for an agent.
///
/// Construct with [`ContainerId::try_new`] for user-supplied input (validated).
/// [`ContainerId::new`] is infallible for internal/test use where input is known-valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContainerId(String);

impl ContainerId {
    /// Validated constructor for user-supplied input.
    ///
    /// Rejects empty strings, characters outside `[A-Za-z0-9_-]`, IDs > 63 chars,
    /// and IDs that do not start and end with `[A-Za-z0-9]`.
    /// Returns `RuntimeError::InvalidSpec` on failure.
    pub fn try_new(id: impl Into<String>) -> Result<Self> {
        let s: String = id.into();
        Self::check(&s)?;
        Ok(ContainerId(s))
    }

    /// Infallible constructor for internal/test use. Prefer [`try_new`](ContainerId::try_new)
    /// for user-supplied input.
    pub fn new(id: impl Into<String>) -> Self {
        ContainerId(id.into())
    }

    /// Create a ContainerId from an agent UUID.
    ///
    /// UUID v4 always satisfies ContainerId constraints: 36 chars, lowercase hex + hyphens,
    /// starts and ends with a hex digit. This avoids the `try_new(...).expect(...)` pattern
    /// at every call site that converts an agent `Uuid` to a container ID.
    pub fn from_uuid(id: uuid::Uuid) -> Self {
        ContainerId(id.to_string())
    }

    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate this ID's format. Called by [`DeploymentSpec::validate`] and backend
    /// methods that accept a raw `ContainerId` to prevent label-selector injection.
    pub fn validate(&self) -> Result<()> {
        Self::check(&self.0)
    }

    fn check(s: &str) -> Result<()> {
        if s.is_empty() {
            return Err(RuntimeError::InvalidSpec(
                "container_id must be non-empty".to_owned(),
            ));
        }
        if s.len() > 63 {
            return Err(RuntimeError::InvalidSpec(
                "container_id exceeds 63 characters".to_owned(),
            ));
        }
        if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(RuntimeError::InvalidSpec(
                "container_id must contain only [A-Za-z0-9_-]".to_owned(),
            ));
        }
        // K8s label values must begin and end with [A-Za-z0-9]. Enforcing this here
        // also guarantees object_name() always produces a non-empty sanitized result
        // after sanitization, superseding the old "at least one alphanumeric" guard.
        if !s.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !s.ends_with(|c: char| c.is_ascii_alphanumeric())
        {
            return Err(RuntimeError::InvalidSpec(
                "container_id must start and end with [A-Za-z0-9]".to_owned(),
            ));
        }
        Ok(())
    }
}

impl fmt::Display for ContainerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ContainerId {
    /// Infallible. Prefer [`ContainerId::try_new`] for user-supplied input.
    fn from(s: String) -> Self {
        ContainerId(s)
    }
}

impl From<&str> for ContainerId {
    fn from(s: &str) -> Self {
        ContainerId(s.to_owned())
    }
}

/// Observed lifecycle state of a deployed agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    /// Agent is being scheduled or starting up.
    Pending,
    /// Agent is live and serving requests.
    Running,
    /// Agent process crashed or hit a restart-count threshold.
    Crashed,
    /// Infrastructure failure: image pull error, invalid image, container config error.
    Failed,
    /// Agent was intentionally stopped (scale to 0 or explicit stop).
    Stopped,
    /// Backend returned a state this runtime does not recognise.
    Unknown,
}

impl fmt::Display for RuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RuntimeState::Pending => "pending",
            RuntimeState::Running => "running",
            RuntimeState::Crashed => "crashed",
            RuntimeState::Failed  => "failed",
            RuntimeState::Stopped => "stopped",
            RuntimeState::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// CPU and memory limits applied to every agent container.
///
/// When `None` in [`DeploymentSpec::resources`], both backends apply [`Default`]
/// values (0.5 CPU / 512 MiB) to prevent runaway agents from starving colocated workloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Memory limit in Kubernetes notation (e.g. `"512Mi"`, `"1Gi"`).
    /// Docker parses `Mi`/`Gi` suffixes; bare integers are treated as bytes.
    pub memory: String,
    /// CPU limit in millicores (e.g. `500` = 0.5 CPU).
    /// K8s: emitted as `"<n>m"`. Docker: `nano_cpus = cpu_milli × 1_000_000`.
    pub cpu_milli: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        ResourceLimits {
            memory: "512Mi".to_owned(),
            cpu_milli: 500,
        }
    }
}

impl ResourceLimits {
    /// Validate that memory uses a recognized suffix and cpu_milli is non-zero.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.cpu_milli == 0 {
            return Err(RuntimeError::InvalidSpec(
                "cpu_milli must be > 0".to_owned(),
            ));
        }
        // suffix → multiplier (bytes) used to detect overflow before parse_memory_bytes runs
        let suffixes: &[(&str, i64)] = &[
            ("Gi", 1024 * 1024 * 1024),
            ("Mi", 1024 * 1024),
            ("G",  1_000_000_000),
            ("M",  1_000_000),
        ];
        let mut recognized = false;
        for (sfx, multiplier) in suffixes {
            if let Some(n) = self.memory.strip_suffix(sfx) {
                if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                let parsed: i64 = n.parse().map_err(|_| {
                    RuntimeError::InvalidSpec(format!(
                        "memory {:?} numeric part is too large", self.memory
                    ))
                })?;
                if parsed.checked_mul(*multiplier).is_none() {
                    return Err(RuntimeError::InvalidSpec(format!(
                        "memory {:?} overflows i64 — maximum is 8191Gi / 8191G",
                        self.memory
                    )));
                }
                recognized = true;
                break;
            }
        }
        if !recognized {
            // bare integer
            let is_bare = !self.memory.is_empty() && self.memory.chars().all(|c| c.is_ascii_digit());
            if !is_bare {
                return Err(RuntimeError::InvalidSpec(format!(
                    "memory {:?} is not a recognized quantity (e.g. \"512Mi\", \"1Gi\", \"536870912\")",
                    self.memory
                )));
            }
        }
        Ok(())
    }
}

/// Full specification for an agent deployment.
///
/// The caller constructs this from a build record and passes it to
/// [`ContainerRuntime::deploy`]. This crate never builds images — `image` must be
/// a fully-qualified OCI reference to an already-built image.
///
/// `min_replicas` is used as the initial replica count at deploy time.
/// `max_replicas` is stored and returned in status but not enforced here —
/// autoscaling policy (HPA/KEDA) is the orchestrator's responsibility.
///
/// # K8s port convention
///
/// The first port in `ports` is exposed as service port 80 on the ClusterIP service.
/// Additional ports are exposed on their own port numbers. Always call `validate()`
/// before passing this to a backend — backends call it internally in `deploy()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentSpec {
    /// Agent identifier (from the Nasiko registry).
    pub container_id: ContainerId,
    /// Human-readable name. Used as `app.kubernetes.io/name` label in K8s manifests
    /// and as a Docker container label.
    pub name: String,
    /// Fully-qualified OCI image URL (e.g. `harbor.nasiko.io/agents/my-agent:v1.0.0`).
    pub image: String,
    /// Initial (minimum) replica count. Used as starting replicas at deploy time.
    pub min_replicas: u32,
    /// Maximum replica count hint. Not enforced by this crate.
    pub max_replicas: u32,
    /// Environment variables injected into every container.
    pub env_vars: HashMap<String, String>,
    /// Container port(s). The first port is treated as the primary service port.
    /// Must not be empty — an empty list is rejected with `RuntimeError::InvalidSpec`.
    pub ports: Vec<u16>,
    /// CPU and memory limits. When `None`, defaults to 0.5 CPU / 512 MiB.
    pub resources: Option<ResourceLimits>,
}

impl DeploymentSpec {
    /// Validate the spec before handing it to a backend.
    ///
    /// Called automatically by both backend `deploy()` implementations; callers
    /// may also call this eagerly to surface errors before the async call.
    pub fn validate(&self) -> Result<()> {
        self.container_id.validate()?;
        if self.image.is_empty() {
            return Err(RuntimeError::InvalidSpec(
                "image must not be empty".to_owned(),
            ));
        }
        if self.ports.is_empty() {
            return Err(RuntimeError::InvalidSpec(
                "ports must not be empty".to_owned(),
            ));
        }
        for &port in &self.ports {
            if port == 0 {
                return Err(RuntimeError::InvalidSpec(
                    "port 0 is not a valid container port".to_owned(),
                ));
            }
        }
        if self.min_replicas == 0 {
            return Err(RuntimeError::InvalidSpec(
                "min_replicas must be at least 1 — use scale(0) to stop a running agent".to_owned(),
            ));
        }
        if self.min_replicas > self.max_replicas {
            return Err(RuntimeError::InvalidSpec(
                "min_replicas must not exceed max_replicas".to_owned(),
            ));
        }
        if self.name.is_empty() {
            return Err(RuntimeError::InvalidSpec(
                "name must not be empty".to_owned(),
            ));
        }
        if self.name.len() > 63 {
            return Err(RuntimeError::InvalidSpec(
                "name exceeds 63 characters".to_owned(),
            ));
        }
        let valid_label_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
        if !self.name.chars().all(valid_label_char)
            || !self.name.starts_with(|c: char| c.is_ascii_alphanumeric())
            || !self.name.ends_with(|c: char| c.is_ascii_alphanumeric())
        {
            return Err(RuntimeError::InvalidSpec(
                "name must start/end with [A-Za-z0-9] and contain only [-A-Za-z0-9_.]".to_owned(),
            ));
        }
        if let Some(ref r) = self.resources {
            r.validate()?;
        }
        for (key, value) in &self.env_vars {
            if key.is_empty() || !key.chars().all(|c| c != '=' && c.is_ascii_graphic()) {
                return Err(RuntimeError::InvalidSpec(format!(
                    "env var key {:?} contains invalid characters (= or non-printable)",
                    key
                )));
            }
            if value.chars().any(|c| c.is_ascii_control()) {
                return Err(RuntimeError::InvalidSpec(format!(
                    "env var value for key {:?} contains control characters",
                    key
                )));
            }
        }
        Ok(())
    }
}

/// Observed state of a running (or stopped) agent deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentStatus {
    /// Agent identifier.
    pub container_id: ContainerId,
    /// Current lifecycle state.
    pub state: RuntimeState,
    /// Number of replicas that are currently live and ready.
    pub replicas_live: u32,
    /// Reachable address, if the agent is running and addressable.
    pub endpoint: Option<String>,
    /// Human-readable message: crash reason, pull error, or general info.
    pub message: Option<String>,
    /// Cumulative container restart count (K8s only; 0 for Docker).
    pub restart_count: u32,
}

/// Validate inputs to [`ContainerRuntime::build`] before any backend call.
///
/// Called at the top of every backend's `build()` implementation, matching
/// the pattern of `spec.validate()` in `deploy()` and `container_id.validate()`
/// in all other methods.
pub fn validate_build_inputs(tar_context: &[u8], image_tag: &str) -> Result<()> {
    if tar_context.is_empty() {
        return Err(RuntimeError::InvalidSpec(
            "tar_context must not be empty".to_owned(),
        ));
    }
    const MAX_TAR_BYTES: usize = 500 * 1024 * 1024; // 500 MiB
    if tar_context.len() > MAX_TAR_BYTES {
        return Err(RuntimeError::InvalidSpec(format!(
            "tar_context size {} exceeds maximum {} bytes (500 MiB)",
            tar_context.len(),
            MAX_TAR_BYTES,
        )));
    }
    if image_tag.is_empty() {
        return Err(RuntimeError::InvalidSpec(
            "image_tag must not be empty".to_owned(),
        ));
    }
    // Reject characters that could inject key=value pairs into buildctl --output spec.
    if !image_tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@')) {
        return Err(RuntimeError::InvalidSpec(format!(
            "image_tag {:?} contains invalid characters — only [A-Za-z0-9._-/:@] are allowed",
            image_tag,
        )));
    }
    Ok(())
}
