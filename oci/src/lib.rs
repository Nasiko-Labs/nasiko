pub mod authz;
pub mod error;
pub mod ops;
pub mod pull_credentials;
pub mod routes;
pub mod storage;

use std::sync::Arc;

use axum::Router;
use nasiko_runtime::BlobStore;
use sqlx::PgPool;

pub use authz::{
    BUILD_SERVICE_USERNAME, BuildServiceIdentity, Caller, CallerIdentity, PullOnlyIdentity, Writer,
};

/// Shared state for the OCI registry.
/// Construct this in the host binary. Pass to `axum_routes()` for the default
/// orchestrator, or use the `ops` module directly for custom route wiring.
///
/// Holds no per-process state: an upload session lives in `oci_uploads` and its
/// chunks in `storage`, so any replica can take any request of a session.
#[derive(Clone)]
pub struct OciState {
    pub pool: PgPool,
    pub storage: Arc<dyn BlobStore>,
    /// The host agent images are qualified with (`AGENT_IMAGE_REGISTRY`); empty when unset.
    /// Only images on this host or with no host count as platform repositories.
    pub platform_registry_host: String,
}

impl OciState {
    pub fn new(pool: PgPool, storage: Arc<dyn BlobStore>, platform_registry_host: String) -> Self {
        Self {
            pool,
            storage,
            platform_registry_host,
        }
    }
}

/// Returns an Axum orchestrator implementing the OCI Distribution Spec v2.
/// Routes are prefixed with `/v2/`. The returned orchestrator has its state
/// already applied (`Router<()>`), so it can be merged into any app.
pub fn axum_routes(state: OciState) -> Router {
    routes::router(state)
}
