use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};

use crate::state::AppState;

/// `GET /pool` — read-only, so it's mounted separately from any
/// `require_admin`-gated mutation routes (there are none in OSS today; this
/// module is a stub, EE's real pool/infra scaling lives at `/infra`) under
/// `require_auth` only. Always returns the same static message regardless
/// of role — nothing sensitive to protect — so there's no internal check
/// to add here, unlike the other converted GET handlers elsewhere.
pub fn degradable_router() -> Router<AppState> {
    Router::new()
        .route("/", get(pool_status))
}

async fn pool_status() -> impl IntoResponse {
    // Pool/node management is EE-only (K8s scaling).
    (StatusCode::OK, "{\"message\": \"pool management not available in OSS\"}")
}
