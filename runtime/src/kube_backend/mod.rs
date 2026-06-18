use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::future;
use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::{DeleteParams, ListParams, LogParams, Patch, PatchParams, PostParams};
use kube::{Api, Client};
use tracing::{instrument, warn};

use crate::{
    error::{Result, RuntimeError},
    types::{validate_build_inputs, ContainerId, DeploymentSpec, DeploymentStatus, RuntimeState},
    ContainerRuntime,
};

// ── Config ──────────────────────────────────────────────────────────────────────

/// Configuration for the Kubernetes runtime backend.
#[derive(Clone)]
pub struct KubeRuntimeConfig {
    /// Kubernetes namespace where agent workloads are deployed.
    /// Corresponds to `nasiko-agents` in the Python reference implementation.
    pub namespace: String,
    /// Node selector applied to every agent Deployment pod spec.
    /// Restricts agents to the dedicated agent node pool.
    pub node_selector: HashMap<String, String>,
    /// Tolerations applied to every agent Deployment pod spec.
    /// Must include a matching toleration for any taint on the agent node pool.
    /// Stored as raw JSON values for flexibility (matches the manifest builder pattern).
    pub tolerations: Vec<serde_json::Value>,
    /// Per-operation timeout for Kubernetes API calls. Default: 30 seconds.
    pub operation_timeout: Duration,

    /// Timeout for the full image build (Job polling loop). Default: 30 minutes.
    pub build_timeout: Duration,

    /// MinIO/S3 endpoint used by the control-plane process to upload/delete build context tars.
    /// e.g. `"http://localhost:9000"` when running outside the cluster.
    pub minio_endpoint: String,

    /// MinIO endpoint embedded in the K8s build Job (used by the minio/mc init container).
    /// Defaults to `minio_endpoint` when empty — set this when the control plane runs outside
    /// the cluster but the Job needs an in-cluster or host-accessible address.
    /// e.g. `"http://host.docker.internal:9000"` for Docker Desktop local testing.
    pub minio_endpoint_in_cluster: String,

    /// MinIO bucket for build context staging. Default: `"nasiko-builds"`.
    pub minio_bucket: String,

    /// MinIO access key (supplied from a K8s Secret at runtime).
    pub minio_access_key: String,

    /// MinIO secret key (supplied from a K8s Secret at runtime).
    pub minio_secret_key: String,

    /// BuildKit daemon address in the same namespace as agents.
    /// Default: `"tcp://buildkitd.nasiko-agents.svc.cluster.local:1234"`
    pub buildkit_addr: String,

    /// K8s Secret holding `.dockerconfigjson` for registry push auth.
    /// Default: `"agent-registry-credentials"`
    pub registry_secret_name: String,

    /// PVC size for the `buildkitd` layer cache StatefulSet.
    /// Default: `"15Gi"`
    pub buildkit_cache_size: String,

    /// Image for the build job init container that downloads the build context.
    /// Default: `"alpine:3.21"`. Override for air-gapped or internal mirror setups.
    pub build_init_image: String,

    /// Allow HTTP (non-TLS) for MinIO connections. Default: `false`.
    /// Set to `true` only for local development. Never use in production —
    /// build contexts can contain secrets.
    pub minio_allow_http: bool,

    /// Registries to push to over HTTP (insecure). Checked by exact hostname prefix.
    /// Default: empty — all registries use TLS.
    pub insecure_registries: Vec<String>,
}

impl std::fmt::Debug for KubeRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeRuntimeConfig")
            .field("namespace", &self.namespace)
            .field("minio_endpoint", &self.minio_endpoint)
            .field("minio_endpoint_in_cluster", &self.minio_endpoint_in_cluster)
            .field("minio_access_key", &"[REDACTED]")
            .field("minio_secret_key", &"[REDACTED]")
            .field("buildkit_addr", &self.buildkit_addr)
            .finish_non_exhaustive()
    }
}

impl Default for KubeRuntimeConfig {
    fn default() -> Self {
        KubeRuntimeConfig {
            namespace: "nasiko-agents".to_owned(),
            // Enforce agent node pool — all agent pods land on nodes labelled
            // nasiko.com/pool=agents. Apply the matching toleration so the taint
            // nasiko.com/pool=agents:NoSchedule does not block scheduling.
            node_selector: [("nasiko.com/pool".to_owned(), "agents".to_owned())].into(),
            tolerations: vec![serde_json::json!({
                "key": "nasiko.com/pool",
                "operator": "Equal",
                "value": "agents",
                "effect": "NoSchedule"
            })],
            operation_timeout: Duration::from_secs(30),
            build_timeout: Duration::from_secs(30 * 60),
            minio_endpoint: String::new(),
            minio_endpoint_in_cluster: String::new(),
            minio_bucket: "nasiko-builds".to_owned(),
            minio_access_key: String::new(),
            minio_secret_key: String::new(),
            buildkit_addr: "tcp://buildkitd.nasiko-agents.svc.cluster.local:1234".to_owned(),
            registry_secret_name: "agent-registry-credentials".to_owned(),
            buildkit_cache_size: "15Gi".to_owned(),
            build_init_image: "alpine:3.21".to_owned(),
            minio_allow_http: false,
            insecure_registries: Vec::new(),
        }
    }
}

// ── Public struct ──────────────────────────────────────────────────────────────

/// Kubernetes-based agent runtime for production deployments.
///
/// Each agent maps to one `Deployment` + one `Service` (ClusterIP) in the
/// configured namespace. Object names are derived deterministically from
/// `container_id` and are DNS-label compliant (see [`object_name`](KubeRuntime::object_name)).
///
/// All mutating operations use server-side apply (`fieldManager=nasiko`), making
/// every `deploy` call idempotent regardless of concurrent callers.
///
/// # Endpoint convention
/// `endpoint()` returns `{service_name}.{namespace}.svc.cluster.local` — the
/// in-cluster DNS address. Route registration with the gateway is the caller's
/// responsibility.
pub struct KubeRuntime {
    client: Client,
    config: KubeRuntimeConfig,
}

impl std::fmt::Debug for KubeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeRuntime")
            .field("namespace", &self.config.namespace)
            .finish_non_exhaustive()
    }
}

// ── Constructor & helpers ──────────────────────────────────────────────────────

impl KubeRuntime {
    /// Connect to the Kubernetes cluster using the in-cluster config or kubeconfig file.
    /// Retries up to 3 times with 100ms / 500ms backoff on transient failures.
    pub async fn new(config: KubeRuntimeConfig) -> Result<Self> {
        let mut last_err = String::new();
        for attempt in 0u32..3 {
            match Client::try_default().await {
                Ok(client) => {
                    if let Err(e) = ensure_buildkitd(&client, &config).await {
                        warn!(error = %e, "failed to ensure buildkitd — build() will fail until resolved");
                    }
                    return Ok(KubeRuntime { client, config });
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < 2 {
                        let delay = if attempt == 0 { 100 } else { 500 };
                        warn!(attempt, "kube client init failed, retrying");
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        Err(RuntimeError::BackendUnreachable(last_err))
    }

    /// Derive a DNS-label-safe Kubernetes object name from an `ContainerId`.
    ///
    /// Transformation rules:
    /// 1. Lowercase every character.
    /// 2. Replace any character that is not `[a-z0-9]` with `-`.
    /// 3. Strip leading and trailing `-`.
    /// 4. Truncate to 56 chars (leaving room for the 7-char `nasiko-` prefix).
    /// 5. Prefix with `nasiko-`.
    ///
    /// The result is always `[a-z0-9][a-z0-9-]{0,61}[a-z0-9]` (max 63 chars).
    /// Agent IDs must contain at least one alphanumeric character.
    pub fn object_name(id: &ContainerId) -> String {
        let sanitized: String = id
            .as_str()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();

        let trimmed = sanitized.trim_matches('-');
        debug_assert!(
            !trimmed.is_empty(),
            "object_name called with an all-special-char ContainerId — validate first"
        );
        // Truncate to 56 chars before adding the 7-char "nasiko-" prefix
        let truncated = if trimmed.len() > 56 {
            &trimmed[..56]
        } else {
            trimmed
        };

        let result = format!("nasiko-{truncated}");
        // Ensure no trailing hyphens (can appear after truncation)
        result.trim_end_matches('-').to_owned()
    }
}

// ── Error mapping ──────────────────────────────────────────────────────────────

fn is_kube_not_found(err: &kube::Error) -> bool {
    if let kube::Error::Api(er) = err {
        er.code == 404
    } else {
        false
    }
}

fn map_kube_err(err: kube::Error) -> RuntimeError {
    warn!(error = %err, "kube API error");
    match &err {
        kube::Error::Api(er) => match er.code {
            404 => RuntimeError::Internal("unexpected 404 from Kubernetes API — check server logs".to_owned()),
            409 => RuntimeError::ResourceConflict(
                "resource was concurrently modified — retry the operation".to_owned(),
            ),
            _ => RuntimeError::Internal("unexpected Kubernetes API error — check server logs".to_owned()),
        },
        kube::Error::Service(_) | kube::Error::HyperError(_) => {
            RuntimeError::BackendUnreachable("Kubernetes API server unreachable".to_owned())
        }
        _ => RuntimeError::Internal("unexpected Kubernetes client error — check server logs".to_owned()),
    }
}

// ── Pod state mapping ──────────────────────────────────────────────────────────

/// Waiting reasons that indicate a permanent infrastructure failure.
/// Mirrors `FATAL_WAITING_REASONS` from the Python guardian worker.
const FATAL_WAITING_REASONS: &[&str] = &[
    "ImagePullBackOff",
    "ErrImagePull",
    "InvalidImageName",
    "CreateContainerConfigError",
    "CreateContainerError",
];

/// Waiting reasons that indicate an image pull failure specifically.
const IMAGE_PULL_REASONS: &[&str] = &["ImagePullBackOff", "ErrImagePull", "InvalidImageName"];

/// Inspect a pod's container statuses and return the most severe `RuntimeState`.
fn pod_runtime_state(pod: &Pod) -> RuntimeState {
    let pod_status = match pod.status.as_ref() {
        Some(s) => s,
        None => return RuntimeState::Unknown,
    };

    let phase = pod_status.phase.as_deref();

    match phase {
        Some("Succeeded") => return RuntimeState::Stopped,
        Some("Failed") => return RuntimeState::Crashed,
        _ => {}
    }

    // Inspect container statuses for fatal waiting reasons and crash signals
    let containers = pod_status
        .container_statuses
        .as_deref()
        .unwrap_or_default();
    let init_containers = pod_status
        .init_container_statuses
        .as_deref()
        .unwrap_or_default();

    for cs in init_containers.iter().chain(containers.iter()) {
        if let Some(state) = &cs.state {
            if let Some(waiting) = &state.waiting
                && let Some(reason) = &waiting.reason
            {
                if FATAL_WAITING_REASONS.contains(&reason.as_str()) {
                    return RuntimeState::Failed;
                }
                if reason == "CrashLoopBackOff" {
                    return RuntimeState::Crashed;
                }
            }
            if let Some(terminated) = &state.terminated
                && terminated.exit_code != 0
            {
                return RuntimeState::Crashed;
            }
        }
    }

    // All containers are ready → Running
    if !containers.is_empty() && containers.iter().all(|c| c.ready) {
        return RuntimeState::Running;
    }

    RuntimeState::Pending
}

/// Aggregate a list of pod states into a single `RuntimeState`.
/// The most "alarming" state wins: Failed > Crashed > Running > Pending > Stopped > Unknown.
fn aggregate_pod_states(pods: &[Pod]) -> RuntimeState {
    if pods.is_empty() {
        return RuntimeState::Pending; // Deployment exists but no pods yet
    }
    let mut worst = RuntimeState::Unknown;
    for pod in pods {
        let s = pod_runtime_state(pod);
        worst = match (worst, s) {
            (RuntimeState::Failed, _) | (_, RuntimeState::Failed) => RuntimeState::Failed,
            (RuntimeState::Crashed, _) | (_, RuntimeState::Crashed) => RuntimeState::Crashed,
            (RuntimeState::Running, _) | (_, RuntimeState::Running) => RuntimeState::Running,
            (RuntimeState::Pending, _) | (_, RuntimeState::Pending) => RuntimeState::Pending,
            (RuntimeState::Stopped, _) | (_, RuntimeState::Stopped) => RuntimeState::Stopped,
            _ => RuntimeState::Unknown,
        };
    }
    worst
}

/// Check whether any pod has an image pull failure waiting reason.
/// Returns the failure message if found.
fn image_pull_failure(pods: &[Pod]) -> Option<String> {
    for pod in pods {
        if let Some(ps) = &pod.status {
            for cs in ps.container_statuses.as_deref().unwrap_or_default()
                .iter()
                .chain(ps.init_container_statuses.as_deref().unwrap_or_default().iter())
            {
                if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref())
                    && IMAGE_PULL_REASONS.contains(&w.reason.as_deref().unwrap_or(""))
                {
                    let msg = w.message.clone().unwrap_or_else(|| {
                        w.reason.clone().unwrap_or_default()
                    });
                    return Some(msg);
                }
            }
        }
    }
    None
}

/// Extract a human-readable waiting message from pods for Failed/Crashed states.
fn pod_waiting_message(pods: &[Pod]) -> Option<String> {
    for pod in pods {
        if let Some(ps) = &pod.status {
            for cs in ps.container_statuses.as_deref().unwrap_or_default()
                .iter()
                .chain(ps.init_container_statuses.as_deref().unwrap_or_default().iter())
            {
                if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                    let reason = w.reason.as_deref().unwrap_or("");
                    let detail = w.message.as_deref().unwrap_or("");
                    if !reason.is_empty() {
                        return Some(if detail.is_empty() {
                            reason.to_owned()
                        } else {
                            format!("{reason}: {detail}")
                        });
                    }
                }
                if let Some(t) = cs.state.as_ref().and_then(|s| s.terminated.as_ref()) {
                    if t.exit_code != 0 {
                        let reason = t.reason.as_deref().unwrap_or("non-zero exit");
                        return Some(format!("exited with code {}: {}", t.exit_code, reason));
                    }
                }
            }
        }
    }
    None
}

// ── Manifest builders ──────────────────────────────────────────────────────────

fn deployment_manifest(
    spec: &DeploymentSpec,
    name: &str,
    namespace: &str,
    node_selector: &HashMap<String, String>,
    tolerations: &[serde_json::Value],
) -> serde_json::Value {
    let env_array: Vec<serde_json::Value> = spec
        .env_vars
        .iter()
        .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
        .collect();

    let ports_array: Vec<serde_json::Value> = spec
        .ports
        .iter()
        .map(|&p| serde_json::json!({ "containerPort": p, "protocol": "TCP" }))
        .collect();

    let lim = spec.resources.as_ref().cloned().unwrap_or_default();
    let cpu_str = format!("{}m", lim.cpu_milli);
    // Clamp halved request to at least 1m / 1Mi to avoid "0m" / "0Mi".
    let cpu_request = format!("{}m", (lim.cpu_milli / 2).max(1));
    let mem_request = {
        // Halve all accepted memory formats so K8s uses Burstable QoS (request < limit).
        let mem = &lim.memory;
        if let Some(n) = mem.strip_suffix("Mi").and_then(|s| s.parse::<u64>().ok()) {
            format!("{}Mi", (n / 2).max(1))
        } else if let Some(n) = mem.strip_suffix("Gi").and_then(|s| s.parse::<u64>().ok()) {
            format!("{}Mi", n * 512)
        } else if let Some(n) = mem.strip_suffix('G').and_then(|s| s.parse::<u64>().ok()) {
            format!("{}G", (n / 2).max(1))
        } else if let Some(n) = mem.strip_suffix('M').and_then(|s| s.parse::<u64>().ok()) {
            format!("{}M", (n / 2).max(1))
        } else if let Ok(n) = mem.parse::<u64>() {
            format!("{}", (n / 2).max(1))
        } else {
            mem.clone() // unreachable after validate()
        }
    };

    let mut pod_spec = serde_json::json!({
        // Agents have no legitimate reason to call the K8s API.
        // Withholding the token removes that credential from the pod entirely.
        "automountServiceAccountToken": false,
        "containers": [{
            "name": "agent",
            "image": spec.image,
            "ports": ports_array,
            "env": env_array,
            "securityContext": {
                "runAsNonRoot": true,
                "runAsUser": 65534,
                "allowPrivilegeEscalation": false,
                "readOnlyRootFilesystem": true,
                "capabilities": { "drop": ["ALL"] },
                "seccompProfile": { "type": "RuntimeDefault" }
            },
            "resources": {
                "limits":   { "memory": &lim.memory, "cpu": &cpu_str },
                "requests": { "memory": &mem_request, "cpu": &cpu_request }
            }
        }]
    });

    if !node_selector.is_empty() {
        pod_spec["nodeSelector"] = serde_json::to_value(node_selector)
            .unwrap_or(serde_json::Value::Object(Default::default()));
    }

    if !tolerations.is_empty() {
        pod_spec["tolerations"] = serde_json::Value::Array(tolerations.to_vec());
    }

    serde_json::json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "nasiko",
                "app.kubernetes.io/name": spec.name,
                // Use sanitized K8s name as label value — safe [a-z0-9-] chars only
                "nasiko-agent-id": name,
                // Raw original ID stored for list() round-trip correctness
                "nasiko-agent-id-raw": spec.container_id.as_str(),
            }
        },
        "spec": {
            "replicas": spec.min_replicas,
            "selector": {
                "matchLabels": {
                    "nasiko-agent-id": name,
                }
            },
            "template": {
                "metadata": {
                    "labels": {
                        "nasiko-agent-id": name,
                    }
                },
                "spec": pod_spec,
            }
        }
    })
}

/// Service manifest builder.
///
/// # Port convention
/// The first port in `spec.ports` is mapped to service port 80 (HTTP gateway convention).
/// All additional ports retain their original numbers. The accessible port therefore
/// differs from the container port for single-port agents — callers should use the
/// ClusterIP DNS name via `endpoint()` rather than guessing the service port.
fn service_manifest(
    spec: &DeploymentSpec,
    service_name: &str,
    namespace: &str,
) -> serde_json::Value {
    let service_ports: Vec<serde_json::Value> = spec
        .ports
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let svc_port = if i == 0 { 80u16 } else { p };
            serde_json::json!({
                "name": format!("port-{p}"),
                "port": svc_port,
                "targetPort": p,
                "protocol": "TCP",
            })
        })
        .collect();

    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": service_name,
            "namespace": namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "nasiko",
                "app.kubernetes.io/name": spec.name,
                "nasiko-agent-id": service_name,
            }
        },
        "spec": {
            "type": "ClusterIP",
            "selector": {
                "nasiko-agent-id": service_name,
            },
            "ports": service_ports,
        }
    })
}

// ── BuildKit manifest builders ────────────────────────────────────────────────────

fn buildkitd_statefulset_manifest(cfg: &KubeRuntimeConfig) -> serde_json::Value {
    let node_selector = serde_json::to_value(&cfg.node_selector)
        .unwrap_or(serde_json::Value::Object(Default::default()));
    serde_json::json!({
        "apiVersion": "apps/v1",
        "kind": "StatefulSet",
        "metadata": {
            "name": "buildkitd",
            "namespace": cfg.namespace,
            "labels": {
                "app": "buildkitd",
                "app.kubernetes.io/managed-by": "nasiko"
            }
        },
        "spec": {
            "replicas": 1,
            "serviceName": "buildkitd",
            "selector": { "matchLabels": { "app": "buildkitd" } },
            "template": {
                "metadata": { "labels": { "app": "buildkitd" } },
                "spec": {
                    "nodeSelector": node_selector,
                    "tolerations": cfg.tolerations,
                    "securityContext": {
                        "runAsUser": 1000,
                        "runAsGroup": 1000,
                        "fsGroup": 1000
                    },
                    "containers": [{
                        "name": "buildkitd",
                        "image": "moby/buildkit:v0.21.1-rootless",
                        "args": [
                            "--addr", "tcp://0.0.0.0:1234",
                            "--oci-worker-no-process-sandbox"
                        ],
                        "ports": [{ "containerPort": 1234, "protocol": "TCP" }],
                        "securityContext": {
                            "seccompProfile": { "type": "Unconfined" }
                        },
                        "volumeMounts": [{
                            "name": "buildkit-cache",
                            "mountPath": "/home/user/.local/share/buildkit"
                        }]
                    }]
                }
            },
            "volumeClaimTemplates": [{
                "metadata": { "name": "buildkit-cache" },
                "spec": {
                    "accessModes": ["ReadWriteOnce"],
                    "resources": { "requests": { "storage": &cfg.buildkit_cache_size } }
                }
            }]
        }
    })
}

fn buildkitd_service_manifest(namespace: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": "buildkitd",
            "namespace": namespace,
            "labels": {
                "app": "buildkitd",
                "app.kubernetes.io/managed-by": "nasiko"
            }
        },
        "spec": {
            "type": "ClusterIP",
            "selector": { "app": "buildkitd" },
            "ports": [{
                "name": "buildkitd",
                "port": 1234,
                "targetPort": 1234,
                "protocol": "TCP"
            }]
        }
    })
}

fn buildkitd_network_policy_manifest(namespace: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {
            "name": "buildkitd-ingress",
            "namespace": namespace,
            "labels": { "app.kubernetes.io/managed-by": "nasiko" }
        },
        "spec": {
            "podSelector": { "matchLabels": { "app": "buildkitd" } },
            "policyTypes": ["Ingress"],
            "ingress": [{
                "from": [{
                    "podSelector": {
                        "matchExpressions": [{
                            "key": "nasiko-build-id",
                            "operator": "Exists"
                        }]
                    }
                }],
                "ports": [{ "protocol": "TCP", "port": 1234 }]
            }]
        }
    })
}

// Ingress NetworkPolicy for a deployed agent: denies pod-to-pod lateral movement within the
// namespace. Egress is unrestricted — agents need to call external APIs and the registry.
// Callers that need stricter egress should layer additional policies via their own tooling.
fn agent_network_policy_manifest(name: &str, namespace: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": { "app.kubernetes.io/managed-by": "nasiko" }
        },
        "spec": {
            "podSelector": { "matchLabels": { "nasiko-agent-id": name } },
            "policyTypes": ["Ingress"],
            "ingress": [{
                "from": [{
                    "namespaceSelector": {
                        "matchLabels": { "kubernetes.io/metadata.name": "nasiko" }
                    }
                }]
            }]
        }
    })
}

fn build_job_manifest(
    job_name: &str,
    build_id: &str,
    image_tag: &str,
    cfg: &KubeRuntimeConfig,
    presigned_url: &str,
) -> serde_json::Value {
    let host = image_tag.split('/').next().unwrap_or(image_tag);
    let insecure = cfg.insecure_registries.iter().any(|r| host.starts_with(r.as_str()));
    let output_spec = if insecure {
        format!("type=image,name={image_tag},push=true,registry.insecure=true")
    } else {
        format!("type=image,name={image_tag},push=true")
    };

    // alpine busybox includes both wget and tar — no apk install needed.
    // CONTEXT_URL is a pre-signed MinIO URL valid for 2 hours; no credentials in the pod.
    let init_cmd =
        "wget -qO /workspace.tar \"$CONTEXT_URL\" && tar -xf /workspace.tar -C /workspace"
            .to_owned();

    let node_selector = serde_json::to_value(&cfg.node_selector)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name,
            "namespace": cfg.namespace,
            "labels": {
                "app.kubernetes.io/managed-by": "nasiko",
                "nasiko-build-id": build_id
            }
        },
        "spec": {
            "ttlSecondsAfterFinished": 300,
            "backoffLimit": 0,
            "template": {
                "metadata": {
                    "labels": { "nasiko-build-id": build_id }
                },
                "spec": {
                    "restartPolicy": "Never",
                    "nodeSelector": node_selector,
                    "tolerations": cfg.tolerations,
                    "initContainers": [{
                        "name": "minio-download",
                        "image": cfg.build_init_image,
                        "command": ["/bin/sh", "-c"],
                        "args": [init_cmd],
                        "env": [
                            { "name": "CONTEXT_URL", "value": presigned_url }
                        ],
                        "volumeMounts": [{ "name": "workspace", "mountPath": "/workspace" }]
                    }],
                    "containers": [{
                        "name": "buildkit-client",
                        "image": "moby/buildkit:v0.21.1-rootless",
                        "env": [{ "name": "BUILDKIT_HOST", "value": &cfg.buildkit_addr }],
                        "command": [
                            "buildctl", "build",
                            "--frontend", "dockerfile.v0",
                            "--local", "context=/workspace",
                            "--local", "dockerfile=/workspace",
                            "--output", output_spec
                        ],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/workspace" },
                            {
                                "name": "registry-auth",
                                "mountPath": "/home/user/.docker/config.json",
                                "subPath": "config.json"
                            }
                        ]
                    }],
                    "volumes": [
                        { "name": "workspace", "emptyDir": {} },
                        {
                            "name": "registry-auth",
                            "secret": {
                                "secretName": &cfg.registry_secret_name,
                                "items": [{
                                    "key": ".dockerconfigjson",
                                    "path": "config.json"
                                }]
                            }
                        }
                    ]
                }
            }
        }
    })
}

// ── BuildKit async helpers ────────────────────────────────────────────────────────

// Builds a MinIO object store client from config.
// `in_cluster`: when true and `minio_endpoint_in_cluster` is set, uses the in-cluster
// endpoint (needed for presigned URLs that must be reachable by K8s Job pods).
fn build_minio_store(
    cfg: &KubeRuntimeConfig,
    in_cluster: bool,
) -> Result<object_store::aws::AmazonS3> {
    use object_store::aws::AmazonS3Builder;
    let endpoint = if in_cluster && !cfg.minio_endpoint_in_cluster.is_empty() {
        &cfg.minio_endpoint_in_cluster
    } else {
        &cfg.minio_endpoint
    };
    AmazonS3Builder::new()
        .with_endpoint(endpoint)
        .with_bucket_name(&cfg.minio_bucket)
        .with_access_key_id(&cfg.minio_access_key)
        .with_secret_access_key(&cfg.minio_secret_key)
        .with_region("us-east-1")
        .with_allow_http(cfg.minio_allow_http)
        .with_virtual_hosted_style_request(false)
        .build()
        .map_err(|e| RuntimeError::Internal(format!("MinIO config: {e}")))
}

async fn upload_build_context(
    cfg: &KubeRuntimeConfig,
    build_id: &str,
    tar_bytes: &[u8],
) -> Result<String> {
    use bytes::Bytes;
    use object_store::{path::Path, ObjectStore};

    let store = build_minio_store(cfg, false)?;
    let key = format!("builds/{build_id}.tar");
    let path = Path::from(key.as_str());
    store
        .put(&path, Bytes::copy_from_slice(tar_bytes).into())
        .await
        .map_err(|e| RuntimeError::Internal(format!("MinIO upload: {e}")))?;
    Ok(key)
}

async fn delete_build_job(client: &Client, namespace: &str, job_name: &str) {
    let jobs: Api<Job> = Api::namespaced(client.clone(), namespace);
    let _ = jobs.delete(job_name, &DeleteParams::default()).await;
}

async fn delete_build_context(cfg: &KubeRuntimeConfig, key: &str) {
    use object_store::{path::Path, ObjectStore};
    let Ok(store) = build_minio_store(cfg, false) else { return };
    let _ = store.delete(&Path::from(key)).await;
}

async fn sign_build_context(cfg: &KubeRuntimeConfig, key: &str) -> Result<String> {
    use object_store::{path::Path, signer::Signer};
    // in_cluster=true: signed URL must be routable from inside the K8s Job pod.
    let store = build_minio_store(cfg, true)?;
    let url = store
        .signed_url(reqwest::Method::GET, &Path::from(key), Duration::from_secs(90))
        .await
        .map_err(|e| RuntimeError::Internal(format!("MinIO sign URL: {e}")))?;
    Ok(url.to_string())
}

async fn launch_build_job(
    client: &Client,
    cfg: &KubeRuntimeConfig,
    build_id: &str,
    image_tag: &str,
    presigned_url: &str,
) -> Result<String> {
    let short_id = build_id.replace('-', "");
    let job_name = format!("nasiko-build-{}", &short_id[..50.min(short_id.len())]);
    let manifest: Job =
        serde_json::from_value(build_job_manifest(&job_name, build_id, image_tag, cfg, presigned_url))
            .map_err(|e| RuntimeError::Internal(format!("build job manifest: {e}")))?;
    let jobs: Api<Job> = Api::namespaced(client.clone(), &cfg.namespace);
    jobs.create(&PostParams::default(), &manifest)
        .await
        .map_err(map_kube_err)?;
    Ok(job_name)
}

async fn wait_for_build(
    client: &Client,
    namespace: &str,
    job_name: &str,
    timeout: Duration,
) -> Result<()> {
    let jobs: Api<Job> = Api::namespaced(client.clone(), namespace);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(RuntimeError::Timeout("image build".to_owned()));
        }
        let job = jobs
            .get_opt(job_name)
            .await
            .map_err(map_kube_err)?
            .ok_or_else(|| RuntimeError::Internal(format!("build job {job_name} disappeared")))?;
        let status = job.status.as_ref();
        if status.and_then(|s| s.succeeded).unwrap_or(0) > 0 {
            return Ok(());
        }
        if status.and_then(|s| s.failed).unwrap_or(0) > 0 {
            return Err(RuntimeError::Internal(format!(
                "build job {job_name} failed — check pod logs"
            )));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn ensure_buildkitd(client: &Client, cfg: &KubeRuntimeConfig) -> Result<()> {
    let ns = &cfg.namespace;
    let timeout = cfg.operation_timeout;

    // StatefulSet: create-only (volumeClaimTemplates cannot change after creation)
    let sts_api: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
    let existing = tokio::time::timeout(timeout, sts_api.get_opt("buildkitd"))
        .await
        .map_err(|_| RuntimeError::Timeout("ensure buildkitd".to_owned()))?
        .map_err(map_kube_err)?;
    if existing.is_none() {
        let manifest: StatefulSet =
            serde_json::from_value(buildkitd_statefulset_manifest(cfg))
                .map_err(|e| RuntimeError::Internal(format!("buildkitd StatefulSet manifest: {e}")))?;
        tokio::time::timeout(timeout, sts_api.create(&PostParams::default(), &manifest))
            .await
            .map_err(|_| RuntimeError::Timeout("create buildkitd StatefulSet".to_owned()))?
            .map_err(map_kube_err)?;
    }

    // Service: server-side apply (idempotent)
    let svc_api: Api<Service> = Api::namespaced(client.clone(), ns);
    let pp = PatchParams::apply("nasiko");
    tokio::time::timeout(
        timeout,
        svc_api.patch("buildkitd", &pp, &Patch::Apply(buildkitd_service_manifest(ns))),
    )
    .await
    .map_err(|_| RuntimeError::Timeout("ensure buildkitd Service".to_owned()))?
    .map_err(map_kube_err)?;

    // NetworkPolicy: restrict buildkitd port 1234 to build Job pods only.
    // Relies on CNI plugin enforcing NetworkPolicy (all production CNIs do).
    use k8s_openapi::api::networking::v1::NetworkPolicy;
    let np_api: Api<NetworkPolicy> = Api::namespaced(client.clone(), ns);
    let np: NetworkPolicy = serde_json::from_value(buildkitd_network_policy_manifest(ns))
        .map_err(|e| RuntimeError::Internal(format!("buildkitd NetworkPolicy manifest: {e}")))?;
    tokio::time::timeout(timeout, np_api.patch("buildkitd-ingress", &pp, &Patch::Apply(np)))
        .await
        .map_err(|_| RuntimeError::Timeout("ensure buildkitd NetworkPolicy".to_owned()))?
        .map_err(map_kube_err)?;

    Ok(())
}

// ── ContainerRuntime impl ──────────────────────────────────────────────────────────

#[async_trait]
impl ContainerRuntime for KubeRuntime {
    #[instrument(skip(self, spec), fields(container_id = %spec.container_id))]
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<DeploymentStatus> {
        spec.validate()?;

        let name = KubeRuntime::object_name(&spec.container_id);
        let ns = &self.config.namespace;
        let pp = PatchParams::apply("nasiko");
        let timeout = self.config.operation_timeout;

        // Apply Deployment (server-side apply — idempotent, creates or updates)
        let deployment_api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);
        let deploy_manifest = deployment_manifest(spec, &name, ns, &self.config.node_selector, &self.config.tolerations);
        tokio::time::timeout(
            timeout,
            deployment_api.patch(&name, &pp, &Patch::Apply(deploy_manifest)),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("patch Deployment".to_owned()))?
        .map_err(map_kube_err)?;

        // Apply Service (server-side apply — idempotent)
        let service_api: Api<Service> = Api::namespaced(self.client.clone(), ns);
        let svc_manifest = service_manifest(spec, &name, ns);
        tokio::time::timeout(
            timeout,
            service_api.patch(&name, &pp, &Patch::Apply(svc_manifest)),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("patch Service".to_owned()))?
        .map_err(map_kube_err)?;

        // Apply per-agent ingress NetworkPolicy (restricts lateral movement from other pods)
        use k8s_openapi::api::networking::v1::NetworkPolicy;
        let np_api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), ns);
        let np_manifest = agent_network_policy_manifest(&name, ns);
        let np: NetworkPolicy = serde_json::from_value(np_manifest)
            .map_err(|e| RuntimeError::Internal(format!("agent NetworkPolicy manifest: {e}")))?;
        tokio::time::timeout(timeout, np_api.patch(&name, &pp, &Patch::Apply(np)))
            .await
            .map_err(|_| RuntimeError::Timeout("patch agent NetworkPolicy".to_owned()))?
            .map_err(map_kube_err)?;

        self.status(&spec.container_id).await
    }

    #[instrument(skip(self))]
    async fn destroy(&self, container_id: &ContainerId) -> Result<()> {
        container_id.validate()?;
        let name = KubeRuntime::object_name(container_id);
        let ns = &self.config.namespace;
        let dp = DeleteParams::default();
        let timeout = self.config.operation_timeout;

        let deployment_api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);
        let deploy_result = tokio::time::timeout(timeout, deployment_api.delete(&name, &dp))
            .await
            .map_err(|_| RuntimeError::Timeout("delete Deployment".to_owned()))?;
        match deploy_result {
            Ok(_) => {}
            Err(ref e) if is_kube_not_found(e) => {}
            Err(e) => return Err(map_kube_err(e)),
        }

        let service_api: Api<Service> = Api::namespaced(self.client.clone(), ns);
        let svc_result = tokio::time::timeout(timeout, service_api.delete(&name, &dp))
            .await
            .map_err(|_| RuntimeError::Timeout("delete Service".to_owned()))?;
        match svc_result {
            Ok(_) => {}
            Err(ref e) if is_kube_not_found(e) => {}
            Err(e) => return Err(map_kube_err(e)),
        }

        // Best-effort: delete the per-agent NetworkPolicy (404 is fine — idempotent)
        use k8s_openapi::api::networking::v1::NetworkPolicy;
        let np_api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), ns);
        let np_result = tokio::time::timeout(timeout, np_api.delete(&name, &dp))
            .await
            .map_err(|_| RuntimeError::Timeout("delete agent NetworkPolicy".to_owned()))?;
        match np_result {
            Ok(_) => {}
            Err(ref e) if is_kube_not_found(e) => {}
            Err(e) => return Err(map_kube_err(e)),
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn scale(&self, container_id: &ContainerId, replicas: u32) -> Result<()> {
        container_id.validate()?;

        let name = KubeRuntime::object_name(container_id);
        let ns = &self.config.namespace;
        let timeout = self.config.operation_timeout;

        let deployment_api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);

        // Verify the Deployment exists before patching
        tokio::time::timeout(timeout, deployment_api.get_opt(&name))
            .await
            .map_err(|_| RuntimeError::Timeout("get Deployment".to_owned()))?
            .map_err(map_kube_err)?
            .ok_or_else(|| RuntimeError::ContainerNotFound(container_id.clone()))?;

        // Merge patch — only updates spec.replicas, leaves selector/template untouched.
        // SSA (Patch::Apply) with a partial spec fails validation on K8s >= 1.34.
        let patch = serde_json::json!({ "spec": { "replicas": replicas } });
        let pp = PatchParams::default();
        tokio::time::timeout(timeout, deployment_api.patch(&name, &pp, &Patch::Merge(patch)))
            .await
            .map_err(|_| RuntimeError::Timeout("patch scale".to_owned()))?
            .map_err(map_kube_err)?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus> {
        container_id.validate()?;

        let name = KubeRuntime::object_name(container_id);
        let ns = &self.config.namespace;
        let timeout = self.config.operation_timeout;

        let deployment_api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);
        let deployment = match tokio::time::timeout(timeout, deployment_api.get_opt(&name))
            .await
            .map_err(|_| RuntimeError::Timeout("get Deployment".to_owned()))?
            .map_err(map_kube_err)?
        {
            None => {
                return Ok(DeploymentStatus {
                    container_id: container_id.clone(),
                    state: RuntimeState::Unknown,
                    replicas_live: 0,
                    endpoint: None,
                    message: None,
                });
            }
            Some(d) => d,
        };

        let spec_replicas = deployment
            .spec
            .as_ref()
            .and_then(|s| s.replicas)
            .unwrap_or(0);
        let ready_replicas = deployment
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0) as u32;

        let state = if spec_replicas == 0 {
            // Deployment is intentionally scaled to zero
            RuntimeState::Stopped
        } else if ready_replicas > 0 {
            RuntimeState::Running
        } else {
            // No ready replicas yet — inspect pods for detailed state
            let pod_api: Api<Pod> = Api::namespaced(self.client.clone(), ns);
            let label_selector = format!("nasiko-agent-id={}", name);
            let lp = ListParams::default().labels(&label_selector);
            let pods = tokio::time::timeout(timeout, pod_api.list(&lp))
                .await
                .map_err(|_| RuntimeError::Timeout("list Pods".to_owned()))?
                .map_err(map_kube_err)?
                .items;

            let agg = aggregate_pod_states(&pods);

            if agg == RuntimeState::Failed {
                // Distinguish image pull failure from other infrastructure failures
                if let Some(msg) = image_pull_failure(&pods) {
                    return Err(RuntimeError::ImageNotFound(msg));
                }
            }

            let msg = if matches!(agg, RuntimeState::Failed | RuntimeState::Crashed) {
                pod_waiting_message(&pods)
            } else {
                None
            };

            return Ok(DeploymentStatus {
                container_id: container_id.clone(),
                state: agg,
                replicas_live: ready_replicas,
                endpoint: None,
                message: msg,
            });
        };

        let endpoint = if state == RuntimeState::Running {
            Some(format!("{name}.{ns}.svc.cluster.local"))
        } else {
            None
        };

        Ok(DeploymentStatus {
            container_id: container_id.clone(),
            state,
            replicas_live: ready_replicas,
            endpoint,
            message: None,
        })
    }

    #[instrument(skip(self))]
    async fn list(&self) -> Result<Vec<DeploymentStatus>> {
        let ns = &self.config.namespace;
        let timeout = self.config.operation_timeout;
        let deployment_api: Api<Deployment> = Api::namespaced(self.client.clone(), ns);

        let lp = ListParams::default().labels("app.kubernetes.io/managed-by=nasiko");
        let deployments = tokio::time::timeout(timeout, deployment_api.list(&lp))
            .await
            .map_err(|_| RuntimeError::Timeout("list Deployments".to_owned()))?
            .map_err(map_kube_err)?
            .items;

        let mut statuses = Vec::with_capacity(deployments.len());

        for deployment in deployments {
            let labels = deployment
                .metadata
                .labels
                .as_ref()
                .cloned()
                .unwrap_or_default();

            // Prefer raw label (original validated ID); fall back to sanitized name for
            // old deployments. The sanitized-name fallback is only for pre-raw-label clusters:
            // a caller using the fallback ID to call destroy/scale/endpoint will double-prefix
            // it ("nasiko-nasiko-myagent") and silently no-op — migration scenario only.
            let Some(container_id_str) = labels.get("nasiko-agent-id-raw")
                .or_else(|| labels.get("nasiko-agent-id")) else {
                continue;
            };

            let container_id = ContainerId::new(container_id_str.as_str());
            let spec_replicas = deployment
                .spec
                .as_ref()
                .and_then(|s| s.replicas)
                .unwrap_or(0);
            let ready_replicas = deployment
                .status
                .as_ref()
                .and_then(|s| s.ready_replicas)
                .unwrap_or(0) as u32;

            // Lightweight state inference from Deployment-level info only (no pod list).
            // For precise state (e.g. Crashed vs Pending), use status().
            // Note: returns Pending for agents with 0 ready replicas, including CrashLoopBackOff.
            let state = if spec_replicas == 0 {
                RuntimeState::Stopped
            } else if ready_replicas > 0 {
                RuntimeState::Running
            } else {
                RuntimeState::Pending
            };

            statuses.push(DeploymentStatus {
                container_id,
                state,
                replicas_live: ready_replicas,
                endpoint: None, // Use status() or endpoint() for per-agent address
                message: None,
            });
        }

        Ok(statuses)
    }

    #[instrument(skip(self))]
    async fn endpoint(&self, container_id: &ContainerId) -> Result<String> {
        container_id.validate()?;

        let name = KubeRuntime::object_name(container_id);
        let ns = &self.config.namespace;
        let timeout = self.config.operation_timeout;

        let service_api: Api<Service> = Api::namespaced(self.client.clone(), ns);
        tokio::time::timeout(timeout, service_api.get_opt(&name))
            .await
            .map_err(|_| RuntimeError::Timeout("get Service".to_owned()))?
            .map_err(map_kube_err)?
            .ok_or_else(|| RuntimeError::ContainerNotFound(container_id.clone()))?;

        Ok(format!("{name}.{ns}.svc.cluster.local"))
    }

    #[instrument(skip(self))]
    async fn logs(&self, container_id: &ContainerId, tail: u32) -> Result<Vec<String>> {
        container_id.validate()?;

        let tail = tail.min(10_000);
        let ns = &self.config.namespace;
        let timeout = self.config.operation_timeout;
        let pod_api: Api<Pod> = Api::namespaced(self.client.clone(), ns);
        let name = KubeRuntime::object_name(container_id);
        let label = format!("nasiko-agent-id={}", name);

        let pods = tokio::time::timeout(
            timeout,
            pod_api.list(&ListParams::default().labels(&label)),
        )
        .await
        .map_err(|_| RuntimeError::Timeout("list Pods".to_owned()))?
        .map_err(map_kube_err)?
        .items;

        if pods.is_empty() {
            return Err(RuntimeError::ContainerNotFound(container_id.clone()));
        }

        // Divide the global cap evenly across pods so the total never exceeds 10,000 lines.
        let per_pod = if pods.len() <= 1 { tail } else { (tail / pods.len() as u32).max(1) };
        let multi = pods.len() > 1;
        let lp = LogParams {
            tail_lines: Some(per_pod as i64),
            ..Default::default()
        };

        // Fetch logs from all pods in parallel
        let futs: Vec<_> = pods
            .iter()
            .map(|pod| {
                let api = pod_api.clone();
                let pod_name = pod.metadata.name.clone().unwrap_or_default();
                let lp = lp.clone();
                async move {
                    let result = api.logs(&pod_name, &lp).await;
                    (pod_name, result)
                }
            })
            .collect();

        let results = tokio::time::timeout(timeout, future::join_all(futs))
            .await
            .map_err(|_| RuntimeError::Timeout("fetch pod logs".to_owned()))?;

        let mut all_lines = Vec::new();
        for (pod_name, result) in results {
            let text = result.map_err(map_kube_err)?;
            for line in text.lines() {
                if multi {
                    all_lines.push(format!("[{pod_name}] {line}"));
                } else {
                    all_lines.push(line.to_owned());
                }
            }
        }

        Ok(all_lines)
    }

    #[instrument(skip(self, tar_context), fields(image_tag))]
    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<String> {
        validate_build_inputs(tar_context, image_tag)?;

        let build_id = uuid::Uuid::new_v4().to_string();

        let s3_key = upload_build_context(&self.config, &build_id, tar_context).await?;

        let presigned_url = match sign_build_context(&self.config, &s3_key).await {
            Ok(url) => url,
            Err(e) => {
                delete_build_context(&self.config, &s3_key).await;
                return Err(e);
            }
        };

        let job_name =
            match launch_build_job(&self.client, &self.config, &build_id, image_tag, &presigned_url).await {
                Ok(name) => name,
                Err(e) => {
                    delete_build_context(&self.config, &s3_key).await;
                    return Err(e);
                }
            };

        let result = wait_for_build(
            &self.client,
            &self.config.namespace,
            &job_name,
            self.config.build_timeout,
        )
        .await;

        // Best-effort cleanup regardless of outcome.
        // ttlSecondsAfterFinished=300 is a safety net if either delete fails.
        delete_build_context(&self.config, &s3_key).await;
        delete_build_job(&self.client, &self.config.namespace, &job_name).await;

        result.map(|_| image_tag.to_owned())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
