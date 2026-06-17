// TODO: Move cp-lib routes and business logic here.
// For now, export a minimal build_app that compiles.

use axum::{Router, routing::get};

pub fn build_app() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}
