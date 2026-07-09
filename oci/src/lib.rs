pub mod authz;
pub mod error;
pub mod ops;
pub mod pull_credentials;
pub mod routes;
pub mod storage;

use std::sync::Arc;

use axum::Router;
use bytes::BytesMut;
use dashmap::DashMap;
use sqlx::PgPool;
use storage::S3Storage;
use uuid::Uuid;

pub use authz::{Caller, CallerIdentity, PullOnlyIdentity};

/// Shared state for the OCI registry.
/// Construct this in the host binary. Pass to `axum_routes()` for the default
/// orchestrator, or use the `ops` module directly for custom route wiring.
#[derive(Clone)]
pub struct OciState {
    pub pool: PgPool,
    pub storage: S3Storage,
    pub upload_buffers: Arc<DashMap<Uuid, BytesMut>>,
}

impl OciState {
    pub fn new(pool: PgPool, storage: S3Storage) -> Self {
        Self {
            pool,
            storage,
            upload_buffers: Arc::new(DashMap::new()),
        }
    }
}

/// Returns an Axum orchestrator implementing the OCI Distribution Spec v2.
/// Routes are prefixed with `/v2/`. The returned orchestrator has its state
/// already applied (`Router<()>`), so it can be merged into any app.
pub fn axum_routes(state: OciState) -> Router {
    routes::router(state)
}
