pub mod auth;
pub mod config;
pub mod proxy;
pub mod state;
#[cfg(test)]
mod tests;

use axum::{
    middleware,
    routing::any,
    Router,
};
use tower_http::cors::CorsLayer;

use auth::auth_middleware;
use proxy::{agent_proxy, server_proxy};
use state::GatewayState;

pub fn build_app(state: GatewayState) -> Router {
    // ── Fully public routes (no auth, proxied straight to server) ───────────
    // Only the bare minimum: token exchange and admin bootstrap.
    // token_validate is public because you pass the token in the request body —
    // there is no "caller" to authenticate.
    let public = Router::new()
        .route("/health", any(server_proxy))
        .route("/.well-known/{*rest}", any(server_proxy))
        .route("/api/auth/login", any(server_proxy))
        .route("/api/auth/initialize-admin", any(server_proxy))
        .route("/api/auth/tokens/validate", any(server_proxy));

    // ── Auth-required: A2A orchestrator + direct agent proxy ────────────────
    let authed = Router::new()
        .route("/agents/{agent_id}/{*rest}", any(agent_proxy))
        .route("/agents/{agent_id}", any(agent_proxy))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    // ── Auth-required catch-all: management API + frontend ──────────────────
    // This covers /api/auth/logout, /api/auth/system/*, and all other /api/*
    // so that logout and protected management endpoints require a valid token.
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
