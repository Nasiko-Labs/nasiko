// Axum handlers here deliberately return `Result<T, axum::response::Response>`
// so `?` can short-circuit with an already-built HTTP response — clippy's
// large-Err-variant lint doesn't fit that idiom, which is used pervasively
// across this crate's routes.
#![allow(clippy::result_large_err)]

pub mod acl;
pub mod admin;
pub mod admission;
pub mod agent_lifecycle;
pub mod agent_proxy;
pub mod agents;
pub mod auth;
pub mod build;
pub mod capabilities;
pub mod catalog;
pub mod chat;
pub mod coding_agent_otlp;
pub mod coding_agent_telemetry;
pub mod context_selection;
pub mod flows;
pub mod github;
pub mod hitl;
pub mod llm_configs;
pub mod llm_router;
pub mod maf;
pub mod mcp;
pub mod multipart_util;
pub mod observability;
pub mod onboarding;
pub mod openapi;
pub mod orchestrator_policy;
pub mod pool;
pub mod prompt_context;
pub mod rate_limit;
pub mod registry_a2a;
pub mod router;
pub mod runtime;
pub mod secrets;
pub mod seed;
pub mod settings;
pub mod spa;
pub mod state;
pub mod telemetry;
pub mod titling;
pub mod transcribe;
pub mod usage;
pub mod users;

use axum::handler::Handler;
use axum::http::Method;
use axum::response::IntoResponse;
use axum::{
    Json, Router, middleware,
    routing::{any, get, post},
};
use serde::Serialize;
use std::time::Duration;
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
pub fn cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let origins: Vec<_> = allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            "a2a-version".parse().unwrap(),
        ])
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
    let login_limiter = RateLimiter::new(30, Duration::from_secs(60));
    // OSS-tier agent grants/visibility API (list grants, public toggle, user
    // and agent-agent shares, ownership transfer). Mounted HERE and not in
    // `agents::router()` because EE replaces it wholesale: `build_ee_app`
    // nests its richer org-aware grants router at the same
    // `/api/agents/{id}/…` paths, and mounting both would panic on route
    // conflicts at startup. Handlers gate themselves on owner-or-superuser
    // (`acl::can_manage_agent`), so the mount needs only `require_auth`.
    let grant_routes = Router::new()
        .nest("/agents", agents::grants::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));
    build_app_with_user_router(state.clone(), fallback, users::router())
        .nest("/api", grant_routes.with_state(state.clone()))
        .merge(auth::login::public_router(login_limiter).with_state(state))
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
    // Spawn the MAF worker — requires an OpenAI API key for prompt generation,
    // extraction, and final output synthesis. Degrades gracefully rather than
    // panicking: MAF routes still work (create/list/run) without a key, jobs
    // just queue in Redis unprocessed until one is configured.
    if let Some(api_key) = state.config.openai_api_key.clone() {
        let llm_config = nasiko_orchestrator::maf::LlmConfig {
            api_key,
            base_url: state.config.openai_base_url.clone(),
            model: state.config.openai_model.clone(),
        };
        // The MAF worker's client makes nothing but agent A2A calls, so it
        // carries the agent-call budget at the client level rather than
        // repeating a per-request override at each of the executor's call
        // sites. Its own pool, deliberately: a background worker's traffic
        // profile has no business sharing the request path's.
        let maf_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                state.config.agent_call_timeout_secs,
            ))
            .build()
            .expect("failed to build MAF agent client");
        nasiko_orchestrator::maf::start_worker(
            state.db.clone(),
            state.redis.clone(),
            maf_client,
            // The same guard the A2A dispatch and agent-proxy paths use, so a
            // MAF step's agent call is bounded by exactly the cascade limits
            // every other inter-agent call already is.
            std::sync::Arc::new(state.flow_guard.clone()),
            llm_config,
            state.hitl_store.clone(),
        );
    } else {
        tracing::warn!(
            "OPENAI_API_KEY not set — MAF worker not started; MAF executions will queue but not run"
        );
    }

    // Container lifecycle mutations (deploy/destroy/stop/start/restart/scale):
    // deployer+ only. The read routes (list/status/logs) are split out into
    // `degradable_routes` below — same `can_deploy` check, but inline per
    // handler so a caller below deployer gets 200 {"available": false}
    // instead of a blanket 403.
    let container_routes =
        Router::new()
            .nest("/containers", admin::router())
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::rbac::require_deployer,
            ));

    // Pool/scaling: read-only stub in OSS (EE's real scaling lives at
    // /infra), no mutations to gate — mounted under require_auth only (via
    // `protected`'s outer layer), no per-route role check needed.
    let pool_routes = Router::new().nest("/pool", pool::degradable_router());

    // User management: admin role or superuser.
    let user_routes = user_router.layer(middleware::from_fn_with_state(
        state.clone(),
        auth::rbac::require_user_manager,
    ));

    // Agent deploy MUTATIONS (upload, restart-deployment, update/rollback):
    // deployer+ only. Reads are in `degradable_routes` below.
    let agent_deploy_routes =
        Router::new()
            .nest("/agents", agents::router())
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::rbac::require_deployer,
            ));

    // Build MUTATIONS (create): deployer+ only. Reads (list/get/logs/
    // progress) are in `degradable_routes` below.
    let build_routes = Router::new()
        .merge(build::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::rbac::require_deployer,
        ));

    // MCP-server-upload MUTATIONS (build a container from user-supplied
    // source): deployer+ only, same reasoning as `build_routes` above —
    // building a container is a privileged, resource-consuming operation.
    // Build-status/build-logs reads stay in `mcp::router()` below, at plain
    // `require_auth` (ownership-checked inside the handler).
    let mcp_upload_routes = mcp::upload_mutation_router(state.config.mcp_upload_max_bytes).layer(
        middleware::from_fn_with_state(state.clone(), auth::rbac::require_deployer),
    );

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
    let non_login_limiter = RateLimiter::new(30, Duration::from_secs(60));
    let registry_limiter = RateLimiter::new(60, Duration::from_secs(60));
    // Per-caller, not global: /auth/change-password is authenticated, and it
    // costs two bcrypt cost-12 hashes. 10/min is generous for a human changing
    // their own password and still bounds the CPU burn from a scripted loop.
    let change_password_limiter = RateLimiter::new(10, Duration::from_secs(60));
    // Starting a MAF run is the single most expensive authenticated action in
    // the product: the executor makes 4 LLM calls minimum (plan, per-step
    // placeholder fill, per-step extraction, final synthesis) plus one agent
    // HTTP call per step, and each of those agents makes its own LLM calls.
    // Nothing bounded it, so a client could enqueue runs in a loop and bill
    // the deployment for the lot. `/maf/generate` and
    // `/maf/workflow/from-instruction` share the budget: both are LLM-backed
    // and neither is something a human does at speed.
    let maf_run_limiter = RateLimiter::new(10, Duration::from_secs(60));
    // MAF's read/CRUD surface. Loose on purpose — the UI polls
    // `/maf/execution/{id}` and `/maf/execution/{id}/usage` every couple of
    // seconds while a workflow runs, so this has to allow steady polling and
    // only bounds the pathological case.
    let maf_read_limiter = RateLimiter::new(120, Duration::from_secs(60));

    // Public A2A registry (agent discovery) — see registry_a2a.rs for why it
    // is unauthenticated; the global fixed window bounds enumeration abuse.
    let registry_routes = Router::new()
        .route("/a2a/v1", post(registry_a2a::registry_a2a_handler))
        .layer(middleware::from_fn_with_state(
            registry_limiter,
            rate_limit::limit_globally,
        ));

    // LLM router context, built before the management routes so the classifier preview
    // shares the exact `ClassifierService` instance routing uses (one backend client, one
    // set of counters). Mounted further down, outside `/api`.
    let llm_ctx =
        nasiko_llm_router::LlmRouterCtx::from_shared(state.db.clone(), state.http_client.clone());
    let classifier_preview = std::sync::Arc::new(llm_router::classifier::ClassifierPreview::new(
        llm_ctx.classifier.clone(),
    ));
    // A hosted classifier preview is a paid network call; a human testing prompts by hand
    // does not need more than this.
    let classifier_preview_limiter = RateLimiter::new(20, Duration::from_secs(60));

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
        .merge(context_selection::router())
        .merge(onboarding::router())
        .merge(coding_agent_telemetry::router())
        .merge(maf::router(maf_run_limiter, maf_read_limiter))
        .merge(secrets::router())
        .merge(llm_configs::router())
        .merge(settings::router())
        .merge(llm_router::model_registry::router())
        .merge(llm_router::providers::router())
        .merge(llm_router::custom_providers::router())
        .merge(llm_router::classifier::router(
            classifier_preview,
            classifier_preview_limiter,
        ))
        .merge(capabilities::router())
        .merge(usage::routes::router())
        .merge(flows::router())
        .merge(router::hitl::router())
        .nest(
            "/observability",
            observability::protected_router(state.clone()),
        )
        .merge(agents::upload::status_router())
        .merge(github::router())
        .merge(auth::login::protected_router(change_password_limiter))
        .merge(transcribe::router())
        .merge(mcp::router())
        .merge(mcp_upload_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        // Unauthed MCP routes (OAuth callback, Composio webhook) — mounted
        // under /api but outside the require_auth layer above; they
        // authenticate via OAuth state / HMAC signature instead of a user JWT.
        .merge(mcp::public_api_router());

    // Agent-facing MCP gateway (`POST /api/mcp`) — deliberately mounted OUTSIDE
    // `require_auth`. Agents authenticate with their deploy-time gateway
    // credential (`Authorization: Bearer $MCP_GATEWAY_TOKEN`) and the user
    // identity is resolved from the request's `traceparent` via the flow
    // record — both validated inside the handler itself
    // (docs/MCP_GATEWAY_AGENT_AUTH.md).
    let mcp_agent_gateway = Router::new()
        .nest("/api", mcp::agent_gateway_router())
        .with_state(state.clone());

    let oci_state = nasiko_oci::OciState::new(state.db.clone(), state.oci_storage.clone());
    // Blob deletes commit a reclaim tombstone and then remove the bytes, so a
    // crash or storage outage between the two leaves work queued. Drain it in the
    // background at boot — inline would delay serving on a slow storage backend,
    // and nothing else ever revisits a stranded tombstone.
    {
        let sweep_state = oci_state.clone();
        tokio::spawn(async move {
            nasiko_oci::ops::blobs::sweep_pending_blob_gc(&sweep_state).await;
        });
    }
    let oci_pull_limiter = RateLimiter::new(300, Duration::from_secs(60));
    let build_push_token_hash = (!state.config.build_push_token.is_empty())
        .then(|| nasiko_oci::pull_credentials::hash_token(&state.config.build_push_token));
    let oci_auth_state = OciAuthState {
        app: state.clone(),
        bearer_limiter: oci_limiter,
        agent_credential_limiter: oci_pull_limiter,
        build_push_token_hash,
    };
    let oci_routes = nasiko_oci::axum_routes(oci_state).layer(middleware::from_fn_with_state(
        oci_auth_state,
        authenticate_oci_request,
    ));

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
    // Both sync loops below read the router's effective config, resolved once here
    // rather than re-read from env per loop.
    let llm_cfg = llm_ctx.cfg.clone();
    let llm_routes = nasiko_llm_router::router(llm_ctx);
    // Keep the provider model catalog (tier-routing candidates) fresh from each
    // provider's GET /models. Runs immediately, then every 24 h; fail-open.
    if state.config.model_catalog_sync_enabled {
        nasiko_llm_router::routing::catalog::spawn_sync(
            state.db.clone(),
            state.http_client.clone(),
            llm_cfg.clone(),
        );
    }
    // Keep model_pricing fresh from the Portkey price book (free, no-auth, MIT);
    // curated seed rows remain the offline baseline. Daily; fail-open.
    if state.config.model_pricing_sync_enabled {
        nasiko_llm_router::routing::pricing_sync::spawn_sync(
            state.db.clone(),
            state.http_client.clone(),
            llm_cfg,
        );
    }

    // UI pages: the static fallback is gated server-side — unauthenticated
    // page navigations get a redirect to /login.html instead of the document
    // (see `auth::require_page_auth`); non-page assets pass through.
    let ui_pages = Router::new()
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_page_auth,
        ))
        // An /api path that reached the UI fallback matched no API route, and
        // must not be answered with the SPA. Serving index.html here — status
        // 200, Content-Type text/html — is what made a missing route surface in
        // the browser as "Server returned a malformed JSON body": a real
        // failure wearing a label that sends you at your own JSON parsing
        // instead of at a route that is not there.
        //
        // Registered here rather than on the outer router because `nest("/api",
        // …)` already owns a catch-all at that position and a second wildcard
        // beside it panics at startup. Nothing else routes inside `ui_pages`,
        // so there is no conflict. After `.layer()` on purpose: the page-auth
        // redirect is for document navigations, and bouncing an API call to
        // login.html would put HTML back in the response we are removing it
        // from.
        .route("/api/{*rest}", any(api_not_found));

    Router::new()
        .route("/health", get(health))
        .merge(observability::router())
        .merge(openapi::router())
        .merge(auth::login::non_login_public_router(non_login_limiter))
        .merge(github::public_router())
        .merge(mcp::composio_callback_router())
        .merge(registry_routes)
        .nest("/api", protected)
        .nest("/api", proxy_routes)
        .with_state(state)
        .merge(oci_routes)
        .merge(llm_routes)
        .merge(mcp_agent_gateway)
        .fallback_service(ui_pages)
        .layer(cors)
        // Default span-making records the full request URI, which would publish
        // the agent credential carried by `/api/mcp/s/{token}` into every span
        // and log line. Redact that one route; everything else is unchanged.
        .layer(TraceLayer::new_for_http().make_span_with(
            |req: &axum::http::Request<axum::body::Body>| {
                let span = tracing::info_span!(
                    "request",
                    method = %req.method(),
                    uri = %mcp::redact_credential_uri(req.uri()),
                    version = ?req.version(),
                );
                // Adopt the caller's W3C trace context when it sends one, so this
                // server span joins the flow that triggered it rather than rooting
                // a trace of its own. Callers without a `traceparent` (a browser
                // hitting the UI or the API) are unaffected and still start a root.
                //
                // Agent→server hops depend on this. The LLM router's `gen_ai.chat`
                // span records the *resolved* provider and model, which is the only
                // place the truth appears when an agent's config re-routes it — the
                // agent labels its own span with the model it asked for. Rooted in a
                // separate trace, that span is unreachable from the session view and
                // from the span→`trace_usage` materializer, so traces and FinOps both
                // fall back to the requested model and price the wrong one.
                if let Some(cx) = req
                    .headers()
                    .get("traceparent")
                    .and_then(|v| v.to_str().ok())
                    .and_then(telemetry::remote_context_from_traceparent)
                {
                    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
                    span.set_parent(cx);
                }
                span
            },
        ))
}

/// The 404 for an unmatched `/api` path, in the envelope every other API error
/// uses (`{data, status_code, message}`) so the frontend's error handling reads
/// it the same way as any other failure rather than choking on HTML.
async fn api_not_found(uri: axum::http::Uri) -> impl IntoResponse {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "data": null,
            "status_code": 404,
            "message": format!("no API route matches {}", uri.path()),
        })),
    )
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
            if !auth_state
                .agent_credential_limiter
                .allow(nasiko_oci::BUILD_SERVICE_USERNAME)
            {
                return rate_limit::too_many_requests();
            }
            req.extensions_mut()
                .insert(nasiko_oci::BuildServiceIdentity);
            return next.run(req).await;
        }

        return match nasiko_oci::pull_credentials::verify(&auth_state.app.db, &username, &password)
            .await
        {
            Ok(Some(agent_id)) => {
                if !auth_state
                    .agent_credential_limiter
                    .allow(&agent_id.to_string())
                {
                    return rate_limit::too_many_requests();
                }
                req.extensions_mut()
                    .insert(nasiko_oci::PullOnlyIdentity { agent_id });
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
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
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
