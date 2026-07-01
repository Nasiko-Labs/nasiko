pub mod auth;
pub mod config;
pub mod orchestrator;
pub mod proxy;
pub mod state;

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

pub fn build_app(state: GatewayState) -> Router {
    // Routes that require authentication
    let authed = Router::new()
        .route("/api/a2a", post(a2a_handler))
        .route("/agents/{agent_id}/{*rest}", any(agent_proxy))
        .route("/agents/{agent_id}", any(agent_proxy))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    // Public routes (no auth) — proxied to server
    let public = Router::new()
        .route("/health", any(server_proxy))
        .route("/.well-known/{*rest}", any(server_proxy))
        .route("/api/auth/{*rest}", any(server_proxy));

    // Catch-all: authenticated, proxied to server (management API + frontend)
    let fallback = Router::new()
        .fallback(any(server_proxy))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .merge(public)
        .merge(authed)
        .merge(fallback)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
