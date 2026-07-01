pub mod a2a_handler;

pub use a2a_handler::{a2a_handler, a2a_upload_handler, router_stats_handler};
pub use nasiko_router::{AgentCardSummary, AgentSelector};

/// Mount all A2A routing routes.
/// The engine implementation is resolved from AppState.routing_engine,
/// set at startup by each binary (OssRoutingEngine for OSS, EeRoutingEngine for EE).
pub fn router_routes() -> axum::Router<crate::state::AppState> {
    axum::Router::new()
        .route("/a2a", axum::routing::post(a2a_handler))
        .route("/a2a/upload", axum::routing::post(a2a_upload_handler))
        .route("/router/stats", axum::routing::get(router_stats_handler))
}
