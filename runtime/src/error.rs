use crate::types::ContainerId;

/// All errors this runtime can surface. Variants are specific enough for the caller
/// to take distinct action rather than treating everything as a black-box failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// The backend (Docker daemon or Kubernetes API server) is unreachable.
    #[error("backend unreachable: {0}")]
    BackendUnreachable(String),

    /// Image pull failed: `ImagePullBackOff`, `ErrImagePull`, `InvalidImageName`.
    #[error("image not found or pull failed: {0}")]
    ImageNotFound(String),

    /// No resource (container / Deployment) exists for this agent ID.
    #[error("container not found: {0}")]
    ContainerNotFound(ContainerId),

    /// A Kubernetes resource already exists and cannot be created again (HTTP 409).
    #[error("resource conflict: {0}")]
    ResourceConflict(String),

    /// The operation did not complete within the allowed time.
    #[error("operation timed out: {0}")]
    Timeout(String),

    /// The caller provided a `DeploymentSpec` that is invalid (e.g. empty ports, empty image).
    #[error("invalid deployment spec: {0}")]
    InvalidSpec(String),

    /// An unexpected error from the backend that does not fit a more specific variant.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias so callers can write `use agent_runtime::Result`.
pub type Result<T> = std::result::Result<T, RuntimeError>;

#[cfg(test)]
mod tests;
