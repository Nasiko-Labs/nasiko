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

/// The shared "you don't have access to this" response for read-only (GET)
/// endpoints: 200, not 403/401 — a caller who's authenticated but lacks the
/// specific role/ownership this endpoint needs gets a body they can render
/// gracefully ("Not available") instead of an error state. Mutations
/// (POST/PUT/DELETE) do NOT use this — a rejected write must still return
/// a real error status, or a client could believe the write succeeded.
pub fn unavailable() -> axum::response::Response {
    use axum::response::IntoResponse;
    Json(serde_json::json!({"available": false})).into_response()
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
    // Container lifecycle mutations (deploy/destroy/stop/start/restart/scale):
    // deployer+ only. The read routes (list/status/logs) are split out into
    // `degradable_routes` below — same `can_deploy` check, but inline per
    // handler so a caller below deployer gets 200 {"available": false}
    // instead of a blanket 403.
    let container_routes = Router::new()
        .nest("/containers", admin::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // Pool/scaling: read-only stub in OSS (EE's real scaling lives at
    // /infra), no mutations to gate — mounted under require_auth only (via
    // `protected`'s outer layer), no per-route role check needed.
    let pool_routes = Router::new().nest("/pool", pool::degradable_router());

    // User management: superuser only
    let user_routes = user_router
        .layer(middleware::from_fn(auth::rbac::require_superuser));

    // Agent deploy MUTATIONS (upload, restart-deployment, update/rollback):
    // deployer+ only. Reads are in `degradable_routes` below.
    let agent_deploy_routes = Router::new()
        .nest("/agents", agents::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // Build MUTATIONS (create): deployer+ only. Reads (list/get/logs/
    // progress) are in `degradable_routes` below.
    let build_routes = Router::new()
        .merge(build::router())
        .layer(middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer));

    // GET routes pulled out from the require_deployer-gated groups above —
    // each handler checks `can_deploy` (and any resource-ownership check
    // that already existed) itself, returning `nasiko_server::unavailable()`
    // (200) instead of relying on a blanket 403 from middleware. Mounted
    // directly into `protected` below, so only `require_auth` applies.
    let degradable_routes = Router::new()
        .nest("/containers", admin::degradable_router())
        .nest("/agents", agents::degradable_router())
        .merge(build::degradable_router());

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
        .merge(degradable_routes)
        .merge(chat::router())
        .merge(secrets::router())
        .merge(settings::router())
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
    let build_push_token_hash = (!state.config.build_push_token.is_empty())
        .then(|| nasiko_oci::pull_credentials::hash_token(&state.config.build_push_token));
    let oci_auth_state = OciAuthState {
        app: state.clone(),
        bearer_limiter: oci_limiter,
        agent_credential_limiter: oci_pull_limiter,
        build_push_token_hash,
    };
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

    Router::new()
        .route("/health", get(health))
        .merge(observability::router())
        .merge(auth::login::public_router(login_limiter))
        .merge(github::public_router())
        .nest("/api", protected)
        .nest("/api", proxy_routes)
        .with_state(state)
        .merge(oci_routes)
        .fallback(fallback)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// State for [`authenticate_oci_request`] — bundles the two things it needs
/// beyond `AppState` (the normal bearer-JWT rate limiter, reused as-is, and a
/// second limiter for both agent-scoped Basic-auth paths below, keyed by
/// agent-or-service identity rather than user).
#[derive(Clone)]
struct OciAuthState {
    app: AppState,
    bearer_limiter: RateLimiter,
    agent_credential_limiter: RateLimiter,
    /// SHA-256 hex of `state.config.build_push_token`, precomputed once at
    /// router-build time (not per-request) — `None` when unconfigured
    /// (`AGENT_RUNTIME=local`, where no in-cluster build path exists), so the
    /// build-service check below is a guaranteed no-match rather than an
    /// accidental "empty string matches empty string" bypass.
    build_push_token_hash: Option<String>,
}

/// Auth middleware for the `/v2/*` OCI registry mount. Accepts any of three
/// credential types, since kubelet/containerd's `imagePullSecrets` and
/// BuildKit's `config.json` mechanisms can't carry a bearer JWT the way this
/// app's normal session auth does:
///
/// - `Authorization: Basic build-service:<token>` — the shared, cluster-wide
///   build-push credential (`GeneratedSecrets::build_push_token`), checked
///   first since it's a cheap in-memory hash comparison, no DB round trip.
///   On success, inserts a `BuildServiceIdentity` extension.
/// - `Authorization: Basic <user:pass>` — a per-agent pull credential minted
///   by `nasiko_oci::pull_credentials` (see its module doc). On success,
///   inserts a `PullOnlyIdentity` extension.
/// - `Authorization: Bearer <jwt>` (or the `access_token` cookie) — the
///   normal session token, validated identically to `auth::require_auth`
///   (this replaces that middleware + the old `populate_oci_caller_identity`
///   adapter for this one mount, since `require_auth` alone can't fall
///   through to try Basic auth on failure).
///
/// A request carrying `Authorization: Basic` never falls through to bearer-
/// JWT validation, regardless of which (if either) of the two Basic checks
/// matches — no `Claims`/`CallerIdentity` extension is ever inserted for
/// that path, so it's structurally impossible for either Basic-auth
/// identity to reach a route that requires a real session (see `Writer`'s
/// doc for how this is enforced for pull credentials specifically at the
/// write routes).
/// `WWW-Authenticate` realm advertised on every 401 this middleware returns.
///
/// Per the Docker Registry/OCI Distribution auth flow, a Basic-auth-capable
/// client (BuildKit, `docker push`, containerd's resolver) sends an
/// unauthenticated request first and only attaches `Authorization: Basic`
/// on a *retry*, triggered by seeing this header on the 401 — it does not
/// eagerly send stored credentials the way `curl -u` does. Omitting this
/// header (as this middleware did before) means such a client never learns
/// it should retry with credentials at all: it just reports the bare 401
/// and gives up. Found live — BuildKit's push failed with a plain "401
/// Unauthorized" on every attempt despite correct, matching credentials
/// being mounted, while `curl -u` (which sends Basic auth preemptively)
/// against the identical URL succeeded.
const OCI_AUTH_REALM: &str = "Basic realm=\"nasiko-registry\"";

fn unauthorized_with_challenge(message: &'static str) -> axum::response::Response {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, OCI_AUTH_REALM)],
        message,
    )
        .into_response()
}

async fn authenticate_oci_request(
    axum::extract::State(auth_state): axum::extract::State<OciAuthState>,
    mut req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    if let Some((username, password)) = extract_basic_auth(req.headers()) {
        if let Some(expected_hash) = &auth_state.build_push_token_hash
            && username == nasiko_oci::BUILD_SERVICE_USERNAME
            && nasiko_oci::pull_credentials::hash_token(&password) == *expected_hash
        {
            if !auth_state.agent_credential_limiter.allow(nasiko_oci::BUILD_SERVICE_USERNAME) {
                return rate_limit::too_many_requests();
            }
            req.extensions_mut().insert(nasiko_oci::BuildServiceIdentity);
            return next.run(req).await;
        }

        return match nasiko_oci::pull_credentials::verify(&auth_state.app.db, &username, &password).await {
            Ok(Some(agent_id)) => {
                if !auth_state.agent_credential_limiter.allow(&agent_id.to_string()) {
                    return rate_limit::too_many_requests();
                }
                req.extensions_mut().insert(nasiko_oci::PullOnlyIdentity { agent_id });
                next.run(req).await
            }
            Ok(None) => unauthorized_with_challenge("invalid credential"),
            Err(e) => {
                tracing::error!(%e, "oci pull credential verification failed");
                unauthorized_with_challenge("invalid credential")
            }
        };
    }

    let claims = match auth::middleware::validate_bearer(&auth_state.app, req.headers()).await {
        Ok(c) => c,
        Err((status, message)) => {
            return if status == axum::http::StatusCode::UNAUTHORIZED {
                unauthorized_with_challenge(message)
            } else {
                (status, message).into_response()
            };
        }
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
