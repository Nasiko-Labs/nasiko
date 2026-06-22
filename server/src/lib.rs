pub mod acl;
pub mod admin;
pub mod agents;
pub mod auth;
pub mod build;
pub mod capabilities;
pub mod chat;
pub mod flow;
pub mod observability;
pub mod pool;
pub mod proxy;
pub mod catalog;
pub mod router;
pub mod runtime;
pub mod secrets;
pub mod seed;
pub mod settings;
pub mod state;
pub mod telemetry;
pub mod usage;
pub mod users;

use axum::{Json, Router, middleware, routing::get};
use axum::handler::Handler;
use serde::Serialize;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::auth::Claims;
use crate::state::AppState;

pub use state::Providers;

/// Generic paginated response wrapper.
#[derive(Debug, Serialize)]
pub struct Paginated<T: Serialize> {
    pub data: Vec<T>,
    pub total: usize,
}

impl<T: Serialize> Paginated<T> {
    pub fn new(data: Vec<T>) -> Self {
        let total = data.len();
        Self { data, total }
    }
}

/// Build the full control plane Axum application.
/// Called by both OSS and cloud binaries. The `fallback` handler serves
/// static UI assets — each binary provides its own with appropriate embeds.
pub fn build_app<F, T>(state: AppState, fallback: F) -> Router
where
    F: Handler<T, ()> + Clone + Send + 'static,
    T: 'static,
{
    // Container lifecycle routes: require deployer+ role
    // TODO: restore RBAC once auth middleware is re-enabled
    let container_routes = Router::new()
        .nest("/containers", admin::router());
        // .layer(middleware::from_fn(auth::rbac::require_deployer));

    // Pool/scaling routes: require admin+ role
    let pool_routes = Router::new()
        .nest("/pool", pool::router())
        .layer(middleware::from_fn(auth::rbac::require_admin));

    // User management: superuser only
    let user_routes = Router::new()
        .merge(users::router())
        .layer(middleware::from_fn(auth::rbac::require_superuser));

    let protected = Router::new()
        .route("/me", get(me))
        .route("/a2a", axum::routing::post(router::a2a_handler))
        .nest("/agents", agents::router())
        .route("/agents/{agent_id}", axum::routing::any(agent_proxy_fallback))
        .route("/agents/{agent_id}/{*rest}", axum::routing::any(agent_proxy_fallback))
        .merge(container_routes)
        .merge(pool_routes)
        .merge(user_routes)
        .nest("/catalog", catalog::router())
        .nest("/build", build::router())
        .merge(chat::router())
        .merge(secrets::router())
        .merge(settings::router())
        .merge(capabilities::router())
        .merge(proxy::router())
        .merge(usage::routes::router())
        .merge(flow::routes::router())
        // .layer(middleware::from_fn_with_state(
        //     state.clone(),
        //     proxy::agent_proxy_middleware,
        // ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let oci_state = nasiko_oci::OciState::new(state.db.clone(), state.oci_storage.clone());
    let oci_routes = nasiko_oci::axum_routes(oci_state);

    // A2A discovery endpoints (public — agents need to discover each other without auth)
    // TODO: add API key auth for production
    let a2a_public = proxy::discovery_router().with_state(state.clone());

    Router::new()
        .route("/health", get(health))
        .merge(observability::router())
        .merge(auth::login::router())  // public: no auth required
        .merge(a2a_public)
        .nest("/api", protected)
        .with_state(state)
        .merge(oci_routes)
        .fallback(fallback)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn health() -> &'static str {
    "ok"
}

/// Catch-all for /agents/{id}/* — the proxy middleware handles the actual forwarding.
/// This route exists solely to make axum route into the protected router so middleware fires.
async fn agent_proxy_fallback() -> axum::http::StatusCode {
    axum::http::StatusCode::BAD_GATEWAY
}

async fn me(claims: Claims) -> Json<Claims> {
    Json(claims)
}
