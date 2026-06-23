pub mod routes;

use axum::Router;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Mirrors the Postgres `build_status` enum (migration 010 §11). Deriving
/// `sqlx::Type` lets sqlx encode/decode it directly instead of treating the
/// column as TEXT; serde keeps the JSON wire shape identical to the old TEXT
/// column ("queued"/"building"/"success"/"failed") so the UI/CLI are unaffected.
/// Shared by `build::routes` and `agents::routes` so both bind the column the
/// same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "build_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Queued,
    Building,
    Success,
    Failed,
}

pub fn router() -> Router<AppState> {
    routes::router()
}

/// Create a tar archive from a directory (for passing to ContainerRuntime::build).
pub fn tar_directory(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    use tar::Builder;

    let buf = Vec::new();
    let mut archive = Builder::new(buf);
    archive
        .append_dir_all(".", dir)
        .map_err(|e| format!("tar append_dir_all: {e}"))?;
    archive
        .into_inner()
        .map_err(|e| format!("tar finalize: {e}"))
}
