pub mod auth;
pub mod config;
pub mod orchestrator;
pub mod proxy;
pub mod state;
pub mod static_files;

use axum::{
    middleware,
    routing::{any, post},
    Router,
};
use tower_http::cors::CorsLayer;

use auth::auth_middleware;
use orchestrator::a2a_handler;
use proxy::{agent_proxy, server_proxy};
use state::GatewayState;
use static_files::static_handler;

pub fn build_app(state: GatewayState) -> Router {
    // Authenticated: orchestrator + agent proxy
    let authed = Router::new()
        .route("/api/a2a", post(a2a_handler))
        .route("/agents/{agent_id}/{*rest}", any(agent_proxy))
        .route("/agents/{agent_id}", any(agent_proxy))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    // Authenticated: management API → proxied to server
    let api_proxy = Router::new()
        .route("/api/{*rest}", any(server_proxy))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    // Public routes — proxied to server (no auth)
    let public = Router::new()
        .route("/health", any(server_proxy))
        .route("/.well-known/{*rest}", any(server_proxy))
        .route("/api/auth/{*rest}", any(server_proxy));

    Router::new()
        .merge(public)
        .merge(authed)
        .merge(api_proxy)
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
