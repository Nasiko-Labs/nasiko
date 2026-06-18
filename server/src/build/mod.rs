pub mod routes;

use axum::Router;

use crate::state::AppState;

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
