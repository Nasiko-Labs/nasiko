pub mod acl;
pub mod admin;
pub mod agent_proxy;
pub mod agents;
pub mod auth;
pub mod build;
pub mod capabilities;
pub mod catalog;
pub mod chat;
pub mod flows;
pub mod github;
pub mod mcp;
pub mod observability;
pub mod pool;
pub mod rate_limit;
pub mod router;
pub mod runtime;
pub mod secrets;
pub mod seed;
pub mod settings;
pub mod state;
pub mod telemetry;
pub mod transcribe;
pub mod usage;
pub mod users;

use std::time::Duration;

use axum::{Json, Router, middleware, routing::{any, get}};
use axum::handler::Handler;
use axum::http::Method;
use serde::Serialize;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::auth::Claims;
use crate::rate_limit::RateLimiter;
use crate::state::AppState;

/// Explicit origin allowlist — never `CorsLayer::permissive()`. The UI is
/// served same-origin by this binary's own static handler in normal
/// deployments (see `main.rs`'s `static_handler`), so cross-origin access is
/// opt-in only, via `CORS_ALLOWED_ORIGINS`. An empty allowlist (the default)
/// allows no cross-origin browser requests at all.
fn cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let origins: Vec<_> = allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([axum::http::header::CONTENT_TYPE, axum::http::header::AUTHORIZATION])
        .allow_credentials(true)
}

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
    build_app_with_user_router(state.clone(), fallback, users::router())
}

/// Build the full control plane Axum application with a custom user orchestrator.
/// EE server passes its own org-aware user orchestrator (which merges management_router
/// and provides EE list/get handlers); OSS `build_app` passes `users::orchestrator()`.
pub fn build_app_with_user_router<F, T>(
    state: AppState,
    fallback: F,
    user_router: Router<AppState>,
) -> Router
where
    F: Handler<T, ()> + Clone + Send + 'static,
    T: 'static,
{
    // Container lifecycle routes: deployer+ only
    let container_routes = Router::new()
        .nest("/containers", admin::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // Pool/scaling routes: require admin+ role
    let pool_routes = Router::new()
        .nest("/pool", pool::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_admin));

    // User management: superuser only
    let user_routes = user_router
        .layer(middleware::from_fn(auth::rbac::require_superuser));

    // Agent deploy routes (upload, deploy-status, deployments, ACL): deployer+ only.
    let agent_deploy_routes = Router::new()
        .nest("/agents", agents::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // Build routes (trigger builds, view build history): deployer+ only
    let build_routes = Router::new()
        .merge(build::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // Fixed-window limiters — see rate_limit.rs for why this app has none of
    // its own otherwise (gateway removal took the last rate limiting with it).
    let a2a_limiter = RateLimiter::new(30, Duration::from_secs(60));
    let oci_limiter = RateLimiter::new(300, Duration::from_secs(60));
    let login_limiter = RateLimiter::new(30, Duration::from_secs(60));

    let protected = Router::new()
        .route("/agents/{id}/{*rest}", any(agent_proxy::agent_proxy))
        .route("/agents/{id}", any(agent_proxy::agent_proxy))
        .route("/me", get(me))
        .merge(router::router_routes(a2a_limiter))
        .merge(agent_deploy_routes)
        .nest("/agents", agents::user_routes())
        .merge(catalog::router())
        .merge(container_routes)
        .merge(pool_routes)
        .merge(user_routes)
        .merge(build_routes)
        .merge(chat::router())
        .merge(secrets::router())
        .merge(settings::router())
        .merge(capabilities::router())
        .merge(usage::routes::router())
        .merge(flows::router())
        .nest("/observability", observability::protected_router())
        .merge(observability::observe_router())
        .merge(github::router())
        .merge(auth::login::protected_router())
        .merge(transcribe::router())
        .merge(mcp::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        // Unauthed MCP routes (OAuth callback, Composio webhook) — mounted
        // under /api but outside the require_auth layer above; they
        // authenticate via OAuth state / HMAC signature instead of a user JWT.
        .merge(mcp::public_api_router());

    // Agent-facing MCP gateway (`POST /api/mcp`) — deliberately mounted OUTSIDE
    // `require_auth`. An agent's only credential is the short-lived delegation
    // JWT (`agent_proxy.rs` strips the caller's real `Authorization`/`Cookie`
    // before forwarding to a container), so this route validates that token
    // itself via `mcp::require_delegation` instead of a user session JWT.
    let mcp_agent_gateway = Router::new()
        .nest("/api", mcp::agent_gateway_router())
        .layer(middleware::from_fn(mcp::require_delegation))
        .with_state(state.clone());

    let oci_state = nasiko_oci::OciState::new(state.db.clone(), state.oci_storage.clone());
    let oci_routes = nasiko_oci::axum_routes(oci_state)
        // Adapt the resolved `Claims` (inserted by `require_auth` below, which
        // runs first) into the crate-agnostic `CallerIdentity` that nasiko-oci's
        // per-repository access checks read — `nasiko-oci` cannot depend on this
        // crate's `Claims` type directly (server depends on oci, not vice versa).
        .layer(middleware::from_fn(populate_oci_caller_identity))
        .layer(middleware::from_fn_with_state(oci_limiter, rate_limit::limit_by_user))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let cors = cors_layer(&state.config.cors_allowed_origins);

    Router::new()
        .route("/health", get(health))
        .merge(observability::router())
        .merge(auth::login::public_router(login_limiter))
        .merge(github::public_router())
        .merge(mcp::composio_callback_router())
        .nest("/api", protected)
        .with_state(state)
        .merge(oci_routes)
        .merge(mcp_agent_gateway)
        .fallback(fallback)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// Copies the already-authenticated `Claims` (set by `require_auth`, which
/// must run before this layer) into a `nasiko_oci::CallerIdentity` extension
/// so the OCI crate's route handlers can authorize per-repository access
/// without depending on this crate's auth types.
async fn populate_oci_caller_identity(
    claims: Claims,
    mut req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    req.extensions_mut().insert(nasiko_oci::CallerIdentity {
        user_id: claims.sub.clone(),
        is_superuser: claims.is_superuser,
    });
    next.run(req).await
}

async fn health() -> &'static str {
    "ok"
}

async fn me(claims: Claims) -> Json<Claims> {
    Json(claims)
}
