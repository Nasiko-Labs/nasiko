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
pub mod llm_wiring;
pub mod model_registry;
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
use axum::{Json, Router, middleware, routing::{any, get, post}};
use axum::handler::Handler;
use axum::http::Method;
use axum::response::IntoResponse;
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
        .merge(model_registry::router())
        .merge(capabilities::router())
        .merge(usage::routes::router())
        .merge(flows::router())
        .nest("/observability", observability::protected_router())
        .merge(github::router())
        .merge(auth::login::protected_router())
        .merge(transcribe::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let oci_state = nasiko_oci::OciState::new(state.db.clone(), state.oci_storage.clone());
    let oci_pull_limiter = RateLimiter::new(300, Duration::from_secs(60));
    let oci_auth_state = OciAuthState { app: state.clone(), bearer_limiter: oci_limiter, pull_limiter: oci_pull_limiter };
    let oci_routes = nasiko_oci::axum_routes(oci_state)
        .layer(middleware::from_fn_with_state(oci_auth_state, authenticate_oci_request));

    let cors = cors_layer(&state.config.cors_allowed_origins);

    // Agent proxy lives in its own router so it never conflicts with catalog routes.
    // - any() on {id}/{*rest}: no catalog route has a wildcard, so no conflict
    // - post() on bare {id}: catalog uses GET/PUT/DELETE, so POST is free for proxying
    let proxy_routes = Router::new()
        .route("/agents/{id}/{*rest}", any(agent_proxy::agent_proxy))
        .route("/agents/{id}", post(agent_proxy::agent_proxy))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    // LLM router: OpenAI-compatible egress proxy for deployed agents. Mounted at the
    // top level (outside `/api` and `auth::require_auth`) — it verifies the agent's
    // own identity JWT internally, not the user session. Deployed agents point their
    // SDK base URL (`LLM_GATEWAY_BASE_URL`) directly at these `/v1/...` routes.
    let llm_routes = nasiko_llm_router::router(nasiko_llm_router::LlmRouterCtx::from_shared(
        state.db.clone(),
        state.http_client.clone(),
    ));

    Router::new()
        .route("/health", get(health))
        .merge(observability::router())
        .merge(auth::login::public_router(login_limiter))
        .merge(github::public_router())
        .nest("/api", protected)
        .nest("/api", proxy_routes)
        .with_state(state)
        .merge(oci_routes)
        .merge(llm_routes)
        .fallback(fallback)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// State for [`authenticate_oci_request`] — bundles the two things it needs
/// beyond `AppState` (the normal bearer-JWT rate limiter, reused as-is, and a
/// second limiter for the pull-credential path, keyed by agent rather than
/// user).
#[derive(Clone)]
struct OciAuthState {
    app: AppState,
    bearer_limiter: RateLimiter,
    pull_limiter: RateLimiter,
}

/// Auth middleware for the `/v2/*` OCI registry mount. Accepts either of two
/// credential types, since kubelet/containerd's `imagePullSecrets` mechanism
/// can't carry a bearer JWT the way this app's normal session auth does:
///
/// - `Authorization: Basic <user:pass>` — a per-agent pull credential minted
///   by `nasiko_oci::pull_credentials` (see its module doc). On success,
///   inserts a `PullOnlyIdentity` extension; no `Claims`/`CallerIdentity` is
///   ever inserted for this path, so it's structurally impossible for a pull
///   credential to reach a push/delete handler (those extract `CallerIdentity`
///   directly, which is simply absent from the request).
/// - `Authorization: Bearer <jwt>` (or the `access_token` cookie) — the
///   normal session token, validated identically to `auth::require_auth`
///   (this replaces that middleware + the old `populate_oci_caller_identity`
///   adapter for this one mount, since `require_auth` alone can't fall
///   through to try Basic auth on failure).
async fn authenticate_oci_request(
    axum::extract::State(auth_state): axum::extract::State<OciAuthState>,
    mut req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    if let Some((username, password)) = extract_basic_auth(req.headers()) {
        return match nasiko_oci::pull_credentials::verify(&auth_state.app.db, &username, &password).await {
            Ok(Some(agent_id)) => {
                if !auth_state.pull_limiter.allow(&agent_id.to_string()) {
                    return rate_limit::too_many_requests();
                }
                req.extensions_mut().insert(nasiko_oci::PullOnlyIdentity { agent_id });
                next.run(req).await
            }
            Ok(None) => (axum::http::StatusCode::UNAUTHORIZED, "invalid pull credential").into_response(),
            Err(e) => {
                tracing::error!(%e, "oci pull credential verification failed");
                (axum::http::StatusCode::UNAUTHORIZED, "invalid pull credential").into_response()
            }
        };
    }

    let claims = match auth::middleware::validate_bearer(&auth_state.app, req.headers()).await {
        Ok(c) => c,
        Err((status, message)) => return (status, message).into_response(),
    };
    if !auth_state.bearer_limiter.allow(&claims.sub) {
        return rate_limit::too_many_requests();
    }
    req.extensions_mut().insert(nasiko_oci::CallerIdentity {
        user_id: claims.sub.clone(),
        is_superuser: claims.is_superuser,
    });
    req.extensions_mut().insert(claims);
    next.run(req).await
}

fn extract_basic_auth(headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    use base64::Engine;
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

async fn health() -> &'static str {
    "ok"
}

async fn me(claims: Claims) -> Json<Claims> {
    Json(claims)
}
