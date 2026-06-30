mod error;
mod types;

#[cfg(feature = "docker")]
mod docker;


pub use error::{Result, RuntimeError};
pub use types::{ContainerId, DeploymentSpec, DeploymentStatus, ResourceLimits, RuntimeState};
pub use types::validate_build_inputs;

// ─── Legacy type aliases (used by server during transition from old orchestrator) ─────
pub type ContainerSpec = DeploymentSpec;
pub type ContainerStatus = DeploymentStatus;
pub type ContainerState = RuntimeState;

/// Stub — pool scaling is EE-only now. Server code references this during transition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoolScalingPolicy {
    pub min_nodes: u32,
    pub max_nodes: u32,
}

/// Stub — scaling events are EE-only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScaleEvent {
    pub from: u32,
    pub to: u32,
    pub reason: String,
}

/// Stub — node info is EE-only.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub status: String,
}

#[cfg(feature = "docker")]
pub use docker::{DockerRuntime, DockerRuntimeConfig};


use async_trait::async_trait;

/// Core trait for managing the deployment lifecycle of Nasiko agents.
///
/// Every method is async and idempotent:
/// - [`deploy`](ContainerRuntime::deploy) called twice converges; it never duplicates resources.
/// - [`destroy`](ContainerRuntime::destroy) on a missing agent is not an error.
/// - [`scale`](ContainerRuntime::scale) with the current replica count is a no-op.
///
/// The trait is object-safe via `async_trait` and suitable for use as
/// `Arc<dyn ContainerRuntime>` across async task boundaries.
///
/// # Backend selection
///
/// The concrete implementation is chosen at startup from `RUNTIME_BACKEND`:
/// - `"docker"` → [`DockerRuntime`] (requires feature `docker`)
/// - `"kubernetes"` → [`KubeRuntime`] (requires feature `k8s`)
///
/// The caller never imports `bollard` or `kube` directly.
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Create or update an agent deployment from `spec`.
    ///
    /// If the agent already exists with the same image, this is a no-op.
    /// If the image has changed, the old deployment is replaced atomically.
    ///
    /// In K8s, the first port in `spec.ports` is mapped to service port 80 on the
    /// ClusterIP service; additional ports retain their original numbers.
    ///
    /// Returns the observed [`DeploymentStatus`] immediately after the deploy
    /// call completes. For Kubernetes, the pod may still be `Pending` — callers
    /// that need `Running` must poll [`status`](ContainerRuntime::status).
    async fn deploy(&self, spec: &DeploymentSpec) -> Result<DeploymentStatus>;

    /// Remove all resources (containers, Deployments, Services) for `container_id`.
    ///
    /// Idempotent: if the agent does not exist, returns `Ok(())`.
    async fn destroy(&self, container_id: &ContainerId) -> Result<()>;

    /// Set the replica count for `container_id`.
    ///
    /// `replicas == 0` stops the agent. `replicas >= 1` starts or scales it.
    ///
    /// On the Docker backend, replicas > 1 is clamped to 1 with a warning — Docker
    /// has no native multi-replica concept.
    ///
    /// Returns `RuntimeError::ContainerNotFound` if no deployment exists for this ID.
    async fn scale(&self, container_id: &ContainerId, replicas: u32) -> Result<()>;

    /// Return the current observed state of the agent deployment.
    ///
    /// If no resource exists for `container_id`, returns a status with
    /// [`RuntimeState::Unknown`] rather than an error, to support polling
    /// patterns where the resource may not yet exist.
    ///
    /// Returns `RuntimeError::ImageNotFound` (K8s only) when pods are stuck in
    /// `ImagePullBackOff` or `ErrImagePull`.
    async fn status(&self, container_id: &ContainerId) -> Result<DeploymentStatus>;

    /// Return status for every agent managed by this runtime instance.
    ///
    /// Used by the reconciler to build a complete picture of cluster state.
    ///
    /// **Note:** Returns `Pending` for agents with 0 ready replicas, including those
    /// in `CrashLoopBackOff`. Use `status()` for per-agent health detail.
    async fn list(&self) -> Result<Vec<DeploymentStatus>>;

    /// Return the reachable address of the agent after a successful deploy.
    ///
    /// - Docker: `localhost:{host_port}` (ephemeral port assigned by Docker)
    /// - Kubernetes: `{service_name}.{namespace}.svc.cluster.local`
    ///
    /// Returns `RuntimeError::ContainerNotFound` if no deployment exists for `container_id`.
    async fn endpoint(&self, container_id: &ContainerId) -> Result<String>;

    /// Return the last `tail` lines of stdout+stderr from the agent's container(s).
    ///
    /// `tail` is clamped to 10 000 to prevent OOM. For Kubernetes backends with
    /// multiple replicas, lines from each pod are prefixed with `[pod-name] ` so
    /// the caller can distinguish sources.
    ///
    /// Returns `RuntimeError::ContainerNotFound` if no container or pod exists for `container_id`.
    async fn logs(&self, container_id: &ContainerId, tail: u32) -> Result<Vec<String>>;

    /// Build a container image from a pre-assembled tar build context.
    ///
    /// `tar_context` is a standard Docker build context: a tar archive containing
    /// at least a `Dockerfile` at the root. The caller (control plane worker) is
    /// responsible for assembling this — including injecting any observability layer
    /// into the Dockerfile before calling this method.
    ///
    /// `image_tag` is a non-empty image reference
    /// (e.g. `harbor.nasiko.io/agents/my-agent:v1.0.0`).
    /// The image is built locally; **no push occurs**. The caller must push separately.
    ///
    /// # Important: this is a long-running operation
    ///
    /// Docker builds can take 2–30 minutes. **Never await this inline in an HTTP handler.**
    /// The control plane must spawn a detached task, persist a `build_id`, and return
    /// `202 Accepted` to the client immediately:
    ///
    /// ```rust,ignore
    /// let build_id = db.create_build_record(&image_tag, BuildStatus::Queued).await?;
    /// tokio::spawn(async move {
    ///     let result = runtime.build(&tar_bytes, &image_tag).await;
    ///     db.update_build_record(build_id, result).await;
    /// });
    /// return Response::new(StatusCode::ACCEPTED, json!({ "build_id": build_id }));
    /// ```
    ///
    /// Returns `image_tag` verbatim on success so the caller can pass it directly to `deploy()`.
    ///
    /// - Docker: streams tar to the Docker daemon via bollard, drains `BuildInfo` output.
    ///   Timeout is `DockerRuntimeConfig::build_timeout` (default 30 min), separate from
    ///   `operation_timeout`.
    /// - K8s: uploads the tar context to object storage and runs a BuildKit build
    ///   Job (`buildctl` against a shared `buildkitd`), polling the Job to completion
    ///   and pushing the image to the registry. Timeout is
    ///   `KubeRuntimeConfig::build_timeout` (default 30 min).
    async fn build(&self, tar_context: &[u8], image_tag: &str) -> Result<String>;

    /// Best-effort delete of any autoscaler resource (e.g. KEDA ScaledObject) for this agent.
    ///
    /// Default no-op — only Kubernetes backends with KEDA installed override this.
    /// Called by the crash-loop guardian before scaling to 0 so KEDA cannot
    /// immediately scale the deployment back up.
    async fn try_delete_autoscaler(&self, _id: &ContainerId) -> Result<()> {
        Ok(())
    }
}
