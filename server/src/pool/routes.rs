use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(pool_status))
}

async fn pool_status() -> impl IntoResponse {
    // Pool/node management is EE-only (K8s scaling).
    (
        StatusCode::OK,
        "{\"message\": \"pool management not available in OSS\"}",
    )
}
