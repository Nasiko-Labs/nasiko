pub mod routes;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    routes::router()
}

pub fn degradable_router() -> Router<AppState> {
    routes::degradable_router()
}

pub fn admin_router() -> Router<AppState> {
    Router::new().route("/tiering/run", post(trigger_tiering_run))
}

/// Starts one tiering pass in the background and returns `202` at once; a pass can
/// run for minutes. The pass takes the cluster-wide tiering lock, so a run already
/// in progress makes this one a no-op (logged).
async fn trigger_tiering_run(State(state): State<AppState>) -> Response {
    let store = state.event_store();
    tokio::spawn(async move {
        match store.run_once(chrono::Utc::now()).await {
            Ok(Some(report)) => tracing::info!(?report, "admin tiering run finished"),
            Ok(None) => tracing::info!("admin tiering run skipped: lock held elsewhere"),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "admin tiering run failed"),
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "status": "started" }))).into_response()
}
