//! Management API for the Level 3 request classifier (`[classifier]` UI companion).
//!
//! - `GET  /api/llm-router/classifier` — effective backend, configuration (no secrets) and
//!   process-wide counters (any authenticated user).
//! - `POST /api/llm-router/classifier/preview` — classify one query/context through the
//!   router's own `ClassifierService` and the regex baseline, side-effect free (superuser,
//!   rate limited: a hosted preview is a paid call).
//!
//! The preview reuses the exact service instance the router routes with, so what it shows
//! is what a request at a routing boundary would get. It never touches the decision
//! cache, learned cells, routing config or agent state; it only increments the service's
//! own counters, which the status endpoint labels as shared with routing. The browser never
//! talks to the hosted backend: the endpoint and key are deployment configuration, and a
//! caller cannot supply either.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Extension, Json, Router,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use nasiko_llm_router::routing::ClassifierService;
use nasiko_llm_router::routing::classifier::{BackendDiagnostics, Classification, ClassifyInput};
use nasiko_llm_router::routing::classifier_service::{
    BackendStatus, ClassifierStats, ClassifyOutcome, Disposition,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::Claims;
use crate::auth::rbac::require_superuser;
use crate::mcp::ApiResponse;
use crate::rate_limit::RateLimiter;

/// Longest query a preview accepts (chars). The service caps the hosted input lower; this
/// bounds what the browser can post at all.
const MAX_PREVIEW_QUERY_CHARS: usize = 8_000;
const MAX_PREVIEW_CONTEXT_CHARS: usize = 4_000;

/// What the preview endpoints hold: the router's shared service and a regex-only baseline.
pub struct ClassifierPreview {
    /// The instance the router routes with (`LlmRouterCtx.classifier`).
    pub configured: Arc<ClassifierService>,
    /// Regex baseline, built once; answers the comparison and the "regex" preview backend.
    pub regex: ClassifierService,
}

impl ClassifierPreview {
    pub fn new(configured: Arc<ClassifierService>) -> Self {
        Self {
            configured,
            regex: ClassifierService::regex_only(),
        }
    }
}

/// Generic over the app state because nothing here reads it: the handlers only need the
/// request extensions (`Claims`, the shared [`ClassifierPreview`]), which also lets the unit
/// tests mount the routes on `()` without constructing an `AppState`.
pub fn router<S: Clone + Send + Sync + 'static>(
    preview: Arc<ClassifierPreview>,
    limiter: RateLimiter,
) -> Router<S> {
    let write = Router::new()
        .route(
            "/llm-router/classifier/preview",
            post(preview_classification),
        )
        .layer(middleware::from_fn_with_state(
            limiter,
            crate::rate_limit::limit_by_user,
        ))
        .layer(middleware::from_fn(require_superuser));
    Router::new()
        .route("/llm-router/classifier", get(classifier_status))
        .merge(write)
        .layer(Extension(preview))
}

/// Effective classifier configuration and counters. Never carries the API key or the
/// endpoint's credentials.
#[derive(Debug, Serialize, ToSchema)]
pub struct ClassifierStatusResponse {
    /// `regex` or `jev`: what deployment configuration asked for.
    pub configured_backend: String,
    /// The backend that answers when nothing fails (`regex` when the configured one could
    /// not initialize).
    pub effective_backend: String,
    /// Why the configured backend is unusable, if it is.
    pub init_error: Option<String>,
    /// Hosted model id requested (`null` for regex).
    pub model: Option<String>,
    /// Hosted endpoint host (no path, no credentials; `null` for regex).
    pub endpoint_host: Option<String>,
    /// Local model directory (Laya), whether or not it loaded; `null` otherwise.
    pub model_path: Option<String>,
    pub timeout_ms: u64,
    /// Below this request-type probability a hosted answer is an abstention; `0` = off.
    pub min_confidence: f32,
    pub routing_seed_set: bool,
    /// Whether the caller may run previews (superuser).
    pub preview_allowed: bool,
    /// Counters since process start, shared between routing and previews.
    pub stats: StatsView,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StatsView {
    pub calls: u64,
    pub primary_ok: u64,
    pub abstained: u64,
    pub fallback_total: u64,
    /// `{reason: count}` for `init`, `inference`, `invalid_output`, `network`, `timeout`.
    pub fallbacks: std::collections::BTreeMap<String, u64>,
}

fn stats_view(s: &ClassifierStats) -> StatsView {
    StatsView {
        calls: s.calls,
        primary_ok: s.primary_ok,
        abstained: s.abstained,
        fallback_total: s.fallback_total(),
        fallbacks: s
            .fallbacks
            .iter()
            .map(|(r, n)| (r.as_str().to_string(), *n))
            .collect(),
    }
}

fn status_response(
    status: &BackendStatus,
    stats: &ClassifierStats,
    preview_allowed: bool,
) -> ClassifierStatusResponse {
    ClassifierStatusResponse {
        configured_backend: status.configured.as_str().to_string(),
        effective_backend: status.effective.clone(),
        init_error: status.init_error.clone(),
        model: status.model.clone(),
        endpoint_host: status
            .endpoint
            .as_deref()
            .and_then(|e| reqwest::Url::parse(e).ok())
            .and_then(|u| u.host_str().map(str::to_string)),
        model_path: status.model_path.clone(),
        timeout_ms: status.timeout_ms,
        min_confidence: status.min_confidence,
        routing_seed_set: status.routing_seed.is_some(),
        preview_allowed,
        stats: stats_view(stats),
    }
}

/// `crate::mcp::ApiResponse` envelope around [`ClassifierStatusResponse`].
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct ClassifierStatusEnvelope {
    data: ClassifierStatusResponse,
    status_code: u16,
    message: String,
}

/// Effective classifier backend, configuration and counters.
#[utoipa::path(
    get,
    path = "/api/llm-router/classifier",
    tag = "llm-router",
    responses(
        (status = 200, description = "Classifier status", body = ClassifierStatusEnvelope),
        (status = 401, description = "Not authenticated"),
    )
)]
pub(crate) async fn classifier_status(
    Extension(preview): Extension<Arc<ClassifierPreview>>,
    claims: Claims,
) -> impl IntoResponse {
    let status = preview.configured.status();
    let stats = preview.configured.stats();
    ApiResponse::ok(
        json!(status_response(&status, &stats, claims.is_superuser)),
        "Classifier status",
    )
}

/// Which backend a preview runs through. Only the deployment-configured model backend is
/// loaded in a process, so the choice is "that one" or the regex baseline; a backend that is
/// not configured cannot be previewed and the UI shows it as unrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewBackend {
    /// The deployment-configured backend (with its regex fallback), as routing uses it.
    Configured,
    /// The regex baseline only.
    Regex,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PreviewRequest {
    /// The latest user request.
    pub query: String,
    /// Optional surrounding material (earlier turns, a snippet).
    #[serde(default)]
    pub context: Option<String>,
    /// Defaults to `configured`.
    #[serde(default = "default_backend")]
    pub backend: PreviewBackend,
}

fn default_backend() -> PreviewBackend {
    PreviewBackend::Configured
}

/// One classification as the preview reports it.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewResult {
    /// Backend whose label this is (`regex` or `jev`).
    pub answered_by: String,
    /// `regex`, `primary`, `abstained`, or `fallback`.
    pub disposition: String,
    /// Fallback reason when `disposition` is `fallback`.
    pub fallback_reason: Option<String>,
    /// Log-safe fallback detail.
    pub fallback_detail: Option<String>,
    pub request_type: String,
    /// 1..=5.
    pub complexity: u8,
    /// Probability of `request_type` for a hosted answer; a fixed uncalibrated placeholder
    /// for regex.
    pub confidence: f32,
    /// Measured decision latency, including any fallback.
    pub latency_us: u64,
    pub input_truncated: bool,
    /// Hosted diagnostics when the hosted backend answered.
    #[schema(value_type = Object, nullable)]
    pub diagnostics: Option<BackendDiagnostics>,
}

fn preview_result(outcome: &ClassifyOutcome) -> PreviewResult {
    let Classification {
        request_type,
        complexity,
        confidence,
    } = outcome.classification;
    let (disposition, fallback_reason, fallback_detail) = match &outcome.disposition {
        Disposition::Regex => ("regex", None, None),
        Disposition::Primary => ("primary", None, None),
        Disposition::Abstained => ("abstained", None, None),
        Disposition::Fallback { reason, detail } => (
            "fallback",
            Some(reason.as_str().to_string()),
            Some(detail.clone()),
        ),
    };
    PreviewResult {
        answered_by: outcome.answered_by.clone(),
        disposition: disposition.to_string(),
        fallback_reason,
        fallback_detail,
        request_type: request_type.as_str().to_string(),
        complexity,
        confidence,
        latency_us: outcome.latency.as_micros() as u64,
        input_truncated: outcome.input_truncated,
        diagnostics: outcome.diagnostics.clone(),
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewResponse {
    /// Which backend the caller asked for.
    pub backend: PreviewBackend,
    /// `regex` or `jev`: what the deployment is configured with.
    pub configured_backend: String,
    /// The requested backend's answer (for `regex`, identical to `baseline`).
    pub result: PreviewResult,
    /// The regex baseline on the same input.
    pub baseline: PreviewResult,
    /// Why the hosted backend is unusable, when it is; the result is then a counted fallback.
    pub init_error: Option<String>,
}

/// `crate::mcp::ApiResponse` envelope around [`PreviewResponse`].
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct PreviewEnvelope {
    data: PreviewResponse,
    status_code: u16,
    message: String,
}

/// Classify one query through the router's classifier and the regex baseline, without
/// touching routing state. Superuser only; rate limited per user.
#[utoipa::path(
    post,
    path = "/api/llm-router/classifier/preview",
    tag = "llm-router",
    request_body = PreviewRequest,
    responses(
        (status = 200, description = "Classification preview", body = PreviewEnvelope),
        (status = 400, description = "Empty or oversized input"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Requires superuser"),
        (status = 429, description = "Rate limit exceeded"),
    )
)]
pub(crate) async fn preview_classification(
    Extension(preview): Extension<Arc<ClassifierPreview>>,
    _claims: Claims,
    Json(body): Json<PreviewRequest>,
) -> axum::response::Response {
    let query = body.query.trim();
    if query.is_empty() {
        return (StatusCode::BAD_REQUEST, "query is required").into_response();
    }
    if query.chars().count() > MAX_PREVIEW_QUERY_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            format!("query must be at most {MAX_PREVIEW_QUERY_CHARS} characters"),
        )
            .into_response();
    }
    let context = body
        .context
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    if context.is_some_and(|c| c.chars().count() > MAX_PREVIEW_CONTEXT_CHARS) {
        return (
            StatusCode::BAD_REQUEST,
            format!("context must be at most {MAX_PREVIEW_CONTEXT_CHARS} characters"),
        )
            .into_response();
    }
    let input = ClassifyInput { query, context };
    let baseline = preview.regex.classify(&input).await;
    let result = match body.backend {
        PreviewBackend::Regex => baseline.clone(),
        PreviewBackend::Configured => {
            // Bounded by the service's own deadline; a stuck call is still answered by the
            // regex fallback inside it. The outer guard only protects the HTTP handler.
            match tokio::time::timeout(
                Duration::from_millis(preview.configured.status().timeout_ms + 2_000),
                preview.configured.classify(&input),
            )
            .await
            {
                Ok(o) => o,
                Err(_) => {
                    return (StatusCode::GATEWAY_TIMEOUT, "classifier preview timed out")
                        .into_response();
                }
            }
        }
    };
    let status = preview.configured.status();
    ApiResponse::ok(
        json!(PreviewResponse {
            backend: body.backend,
            configured_backend: status.configured.as_str().to_string(),
            result: preview_result(&result),
            baseline: preview_result(&baseline),
            init_error: status.init_error,
        }),
        "Classification preview",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use nasiko_llm_router::config::{ClassifierBackend, ClassifierConfig, Secret};
    use serde_json::Value;

    /// The classifier routes alone on a loopback listener, with `Claims` injected the way
    /// `require_auth` does, so the tests exercise authorization, validation and the
    /// shared-service contract without a database or session. Served over TCP because the
    /// server crate has no `tower` dev-dependency for `oneshot`, and adding one would touch
    /// the lockfile.
    async fn serve(preview: Arc<ClassifierPreview>, superuser: bool, limit: u32) -> String {
        let claims = Claims {
            sub: "11111111-1111-1111-1111-111111111111".into(),
            username: "tester".into(),
            is_superuser: superuser,
        };
        let limiter = RateLimiter::new(limit, Duration::from_secs(60));
        let app: Router =
            Router::new()
                .merge(router::<()>(preview, limiter))
                .layer(middleware::from_fn(
                    move |mut req: Request<Body>, next: middleware::Next| {
                        let claims = claims.clone();
                        async move {
                            req.extensions_mut().insert(claims);
                            next.run(req).await
                        }
                    },
                ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn regex_preview() -> Arc<ClassifierPreview> {
        Arc::new(ClassifierPreview::new(Arc::new(
            ClassifierService::regex_only(),
        )))
    }

    /// A deployment configured for Jev without a key: the honest "unconfigured" state.
    fn jev_without_key_preview() -> Arc<ClassifierPreview> {
        let cfg = ClassifierConfig {
            backend: ClassifierBackend::Jev,
            api_key: Secret::default(),
            ..Default::default()
        };
        Arc::new(ClassifierPreview::new(Arc::new(
            ClassifierService::from_config(&cfg),
        )))
    }

    async fn get_status(base: &str) -> (u16, Value, String) {
        let resp = reqwest::get(format!("{base}/llm-router/classifier"))
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::Null),
            text,
        )
    }

    async fn post(base: &str, body: Value) -> (u16, Value, String) {
        let resp = reqwest::Client::new()
            .post(format!("{base}/llm-router/classifier/preview"))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::Null),
            text,
        )
    }

    #[tokio::test]
    async fn status_reports_effective_backend_without_secrets() {
        let cfg = ClassifierConfig {
            backend: ClassifierBackend::Jev,
            api_key: Secret::new("sk-live-should-never-appear"),
            ..Default::default()
        };
        let preview = Arc::new(ClassifierPreview::new(Arc::new(
            ClassifierService::from_config(&cfg),
        )));
        let base = serve(preview, false, 10).await;
        let (status, json, text) = get_status(&base).await;
        assert_eq!(status, 200);
        let d = &json["data"];
        assert_eq!(d["configured_backend"], "jev");
        assert_eq!(d["effective_backend"], "jev");
        assert_eq!(d["model"], "jev-1.13.0");
        assert_eq!(d["endpoint_host"], "api.typesafe.ai");
        assert!(d["model_path"].is_null());
        assert_eq!(d["preview_allowed"], false);
        assert_eq!(d["stats"]["calls"], 0);
        assert!(!text.contains("sk-live"), "key leaked: {text}");
        assert!(!text.contains("api_key"));
    }

    #[tokio::test]
    async fn status_reports_a_laya_deployment_whose_bundle_is_missing() {
        let cfg = ClassifierConfig {
            backend: ClassifierBackend::Laya,
            model_path: "/nonexistent/laya".into(),
            ..Default::default()
        };
        let preview = Arc::new(ClassifierPreview::new(Arc::new(
            ClassifierService::from_config(&cfg),
        )));
        let base = serve(preview, true, 10).await;
        let (status, json, _) = get_status(&base).await;
        assert_eq!(status, 200);
        let d = &json["data"];
        assert_eq!(d["configured_backend"], "laya");
        assert_eq!(d["effective_backend"], "regex");
        assert_eq!(d["model_path"], "/nonexistent/laya");
        assert!(d["endpoint_host"].is_null());
        assert!(d["init_error"].as_str().unwrap().contains("laya.onnx"));
        let (status, json, _) =
            post(&base, json!({"query": "what is the capital of France?"})).await;
        assert_eq!(status, 200);
        assert_eq!(json["data"]["result"]["disposition"], "fallback");
        assert_eq!(json["data"]["result"]["fallback_reason"], "init");
    }

    #[tokio::test]
    async fn preview_requires_superuser() {
        let base = serve(regex_preview(), false, 10).await;
        let (status, _, text) = post(&base, json!({"query": "write a function"})).await;
        assert_eq!(status, 403);
        assert_eq!(text, "requires superuser");
    }

    #[tokio::test]
    async fn preview_validates_input() {
        let base = serve(regex_preview(), true, 10).await;
        let (status, _, text) = post(&base, json!({"query": "   "})).await;
        assert_eq!(status, 400);
        assert!(text.contains("query is required"));
        let (status, _, _) = post(
            &base,
            json!({"query": "x".repeat(MAX_PREVIEW_QUERY_CHARS + 1)}),
        )
        .await;
        assert_eq!(status, 400);
        let (status, _, _) = post(
            &base,
            json!({"query": "q", "context": "c".repeat(MAX_PREVIEW_CONTEXT_CHARS + 1)}),
        )
        .await;
        assert_eq!(status, 400);
        let (status, _, _) = post(&base, json!({"query": "q", "backend": "local-llama"})).await;
        assert_eq!(status, 422);
    }

    #[tokio::test]
    async fn preview_returns_result_and_baseline_through_the_shared_service() {
        let preview = regex_preview();
        let base = serve(preview.clone(), true, 10).await;
        let (status, json, _) = post(
            &base,
            json!({"query": "draft an email to my team about the outage", "context": "user: hi"}),
        )
        .await;
        assert_eq!(status, 200);
        let d = &json["data"];
        assert_eq!(d["backend"], "configured");
        assert_eq!(d["configured_backend"], "regex");
        assert_eq!(d["result"]["request_type"], "writing");
        assert_eq!(d["result"]["complexity"], 3);
        assert_eq!(d["result"]["disposition"], "regex");
        assert_eq!(d["result"]["answered_by"], "regex");
        assert_eq!(d["baseline"]["request_type"], "writing");
        assert!(d["result"]["latency_us"].is_u64());
        assert!(d["init_error"].is_null());
        // The shared instance counted exactly one call; the baseline instance one too.
        assert_eq!(preview.configured.stats().calls, 1);
        assert_eq!(preview.regex.stats().calls, 1);
    }

    #[tokio::test]
    async fn preview_on_an_unconfigured_hosted_backend_is_an_honest_fallback() {
        let base = serve(jev_without_key_preview(), true, 10).await;
        let (status, json, text) =
            post(&base, json!({"query": "what is the capital of France?"})).await;
        assert_eq!(status, 200);
        let d = &json["data"];
        assert_eq!(d["configured_backend"], "jev");
        assert_eq!(d["result"]["disposition"], "fallback");
        assert_eq!(d["result"]["fallback_reason"], "init");
        assert_eq!(d["result"]["answered_by"], "regex");
        assert!(
            d["init_error"]
                .as_str()
                .unwrap()
                .contains("TYPESAFE_API_KEY")
        );
        assert!(!text.contains("sk-"));
    }

    #[tokio::test]
    async fn preview_regex_backend_selector_returns_the_baseline_only() {
        let base = serve(jev_without_key_preview(), true, 10).await;
        let (status, json, _) = post(
            &base,
            json!({"query": "what is the capital of France?", "backend": "regex"}),
        )
        .await;
        assert_eq!(status, 200);
        let d = &json["data"];
        assert_eq!(d["backend"], "regex");
        assert_eq!(d["result"]["disposition"], "regex");
        assert_eq!(d["result"]["request_type"], d["baseline"]["request_type"]);
    }

    #[tokio::test]
    async fn preview_is_rate_limited_per_user() {
        let base = serve(regex_preview(), true, 3).await;
        for _ in 0..3 {
            let (status, _, _) = post(&base, json!({"query": "hello"})).await;
            assert_eq!(status, 200);
        }
        let (status, _, _) = post(&base, json!({"query": "hello"})).await;
        assert_eq!(status, 429);
    }
}
