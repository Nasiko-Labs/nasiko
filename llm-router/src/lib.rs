//! Nasiko LLM Router — a provider-agnostic, OpenAI-compatible egress proxy for
//! user-uploaded agents.
//!
//! Agents are deployed with `OPENAI_BASE_URL` pointed at this router and an
//! `OPENAI_API_KEY` that is a Nasiko identity JWT (not a real provider key). The
//! router verifies the JWT, resolves the agent's provider/model/key from the
//! database, translates the OpenAI-shaped request to the configured provider
//! (OpenAI / Anthropic / Gemini), and returns an OpenAI-shaped response — so the
//! agent never knows which provider answered. See `RUST_PLAN_V1.md`.
//!
//! ## Packaging
//! This is a **library** mounted in-process by `nasiko-server`. It deliberately
//! depends on no server crate: everything it needs is supplied via [`LlmRouterCtx`].
//! That keeps it decoupled and promotable to a standalone binary later (just add a
//! `src/bin` that builds the same context from the environment).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    routing::{get, post},
};
use nasiko_pricing::PricingEngine;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower_http::decompression::RequestDecompressionLayer;

pub mod auth;
mod brevity;
mod compress;
pub mod config;
pub mod error;
pub mod handlers;
pub mod inbound;
pub mod inject;
pub mod ir;
pub mod providers;
pub mod recovery;
pub mod resolver;
pub mod routing;
mod savings;
pub mod usage;

pub use config::{ClassifierConfig, GatewayConfig};
pub use error::GatewayError;
pub use inbound::InboundFormat;
pub use inject::{LlmInjectCtx, inject_llm_env};
pub use resolver::{ConfigCache, ResolvedConfig};
pub use routing::{
    AllowAllGate, CellStore, ClassifierSalienceGate, DecisionCache, GuardedClassifier,
    InMemoryCellStore, NoopCache, PgCellStore, PgTierRegistry, RedisCache, RequestClassifier,
    SalienceGate, TierRegistry,
};

/// Shared context for the LLM router.
///
/// Holds the resources the router needs, supplied by whatever host mounts it (the
/// server passes its own `PgPool` and HTTP client). Cheap to clone — it is the Axum
/// router state.
#[derive(Clone)]
pub struct LlmRouterCtx {
    /// Postgres pool — reads `agents.llm_config` / `user_secrets`, writes `token_usage`.
    pub db: PgPool,
    /// Pooled outbound HTTP client for provider calls.
    pub http: reqwest::Client,
    /// Gateway configuration (JWT secret, defaults, provider base URLs).
    pub cfg: Arc<GatewayConfig>,
    /// Process-wide TTL cache for per-agent `llm_config` lookups.
    pub cache: Arc<ConfigCache>,
    /// Model-routing decision cache, keyed on `(conv_id, agent_id)`. [`NoopCache`] by
    /// default (every read misses); S3 swaps in a Redis-backed impl when configured.
    pub router_cache: Arc<dyn DecisionCache>,
    /// Tier→model registry for classified routing. [`PgTierRegistry`] in production:
    /// operator `model_registry` overrides first, then a mapping derived from the
    /// live provider model catalog (synced from `GET /models`, price-ranked).
    pub tier_registry: Arc<dyn TierRegistry>,
    /// Learned per-provider quality cells behind Thompson-sampling tier selection.
    /// [`PgCellStore`] (durable, cross-instance) in production; tests use
    /// [`InMemoryCellStore`].
    pub cell_store: Arc<dyn CellStore>,
    /// Level 2.5 salience gate — decides whether a boundary turn is substantive enough to
    /// classify + pin. [`ClassifierSalienceGate`] when `SALIENCE_GATE_ENABLED`; else [`AllowAllGate`]
    /// (classify at every boundary, i.e. behaviour before the gate existed).
    pub salience_gate: Arc<dyn SalienceGate>,
    /// Level 3 request classifier (type, complexity, confidence), always wrapped in a
    /// [`GuardedClassifier`] so a failing or slow backend degrades to the regex. Built by
    /// [`build_request_classifier`]; the regex backend unless `CLASSIFIER_BACKEND` says
    /// otherwise.
    pub request_classifier: Arc<dyn RequestClassifier>,
    /// The platform's single cost engine. Every `token_usage` row is priced
    /// through this — the DB trigger that used to do it returned NULL for any
    /// model missing from `model_pricing`, which booked 92.8% of calls at $0.
    pub pricing: Arc<PricingEngine>,
}

impl LlmRouterCtx {
    /// Build from resources the host already owns (the server's `PgPool` + HTTP
    /// client). Gateway-specific config is read from the environment.
    pub fn from_shared(db: PgPool, http: reqwest::Client) -> Self {
        let cfg = GatewayConfig::from_env();
        tracing::info!(
            target: "nasiko::llm_router::startup",
            default_provider = %cfg.default_provider,
            default_model = %cfg.default_model,
            agent_jwt_secret_set = !cfg.agent_jwt_secret.is_empty(),
            agent_jwt_algorithm = %cfg.agent_jwt_algorithm,
            platform_openai_api_key_set = !cfg.platform_openai_api_key.is_empty(),
            platform_anthropic_api_key_set = !cfg.platform_anthropic_api_key.is_empty(),
            platform_gemini_api_key_set = !cfg.platform_gemini_api_key.is_empty(),
            llm_config_cache_ttl_secs = cfg.llm_config_cache_ttl_secs,
            redis_url_set = !cfg.redis_url.is_empty(),
            router_decision_ttl_secs = cfg.router_decision_ttl_secs,
            openai_api_base = %cfg.openai_api_base,
            anthropic_api_base = %cfg.anthropic_api_base,
            gemini_api_base = %cfg.gemini_api_base,
            llm_gateway_base_url = %cfg.llm_gateway_base_url,
            "llm-router: initializing with effective GatewayConfig"
        );
        let cache = Arc::new(ConfigCache::new(Duration::from_secs(
            cfg.llm_config_cache_ttl_secs,
        )));
        let tier_registry = Arc::new(PgTierRegistry::new(db.clone()));
        tracing::info!(
            target: "nasiko::llm_router::startup",
            "llm-router: tier registry = PgTierRegistry (operator model_registry overrides, then live provider catalog ranked by price)"
        );
        let cell_store = Arc::new(PgCellStore::new(db.clone()));
        tracing::info!(
            target: "nasiko::llm_router::startup",
            "llm-router: cell store = PgCellStore (DB router_quality_cells table; learns per-provider tier quality from feedback)"
        );
        let router_cache = build_router_cache(&cfg);
        let cfg = Arc::new(cfg);
        let salience_gate = build_salience_gate(&cfg);
        let request_classifier: Arc<dyn RequestClassifier> =
            build_request_classifier(&cfg.classifier, &http);
        let pricing = Arc::new(PricingEngine::new(db.clone()));
        Self {
            db,
            http,
            cfg,
            cache,
            router_cache,
            tier_registry,
            cell_store,
            salience_gate,
            request_classifier,
            pricing,
        }
    }
}

/// Build the Level 2.5 salience gate from config.
///
/// [`AllowAllGate`] when `SALIENCE_GATE_ENABLED=false`; otherwise
/// [`ClassifierSalienceGate`] over the model embedded in this binary, or over
/// `SALIENCE_WEIGHTS_PATH` when that override is set.
///
/// A model that cannot be loaded (a corrupt embedded asset, or a missing/malformed override
/// file) logs a warning and degrades to [`AllowAllGate`] — the router classifies at every
/// fireable boundary, exactly as it did before the gate existed. That is the same fail-safe
/// direction the gate itself takes: nothing defers a turn unless a model confidently says
/// it is small talk, so a gate that cannot run costs money, never availability.
fn build_salience_gate(cfg: &Arc<GatewayConfig>) -> Arc<dyn SalienceGate> {
    if !cfg.salience_gate_enabled {
        tracing::info!(
            target: "nasiko::llm_router::startup",
            "llm-router: salience gate = AllowAllGate (SALIENCE_GATE_ENABLED=false; classify at every fireable boundary)"
        );
        return Arc::new(AllowAllGate);
    }

    let (source, loaded) = if cfg.salience_weights_path.is_empty() {
        (
            "embedded",
            ClassifierSalienceGate::embedded(
                cfg.salience_low_threshold,
                cfg.salience_high_threshold,
            ),
        )
    } else {
        (
            cfg.salience_weights_path.as_str(),
            ClassifierSalienceGate::from_path(
                &cfg.salience_weights_path,
                cfg.salience_low_threshold,
                cfg.salience_high_threshold,
            ),
        )
    };

    match loaded {
        Ok((gate, trained_at)) => {
            tracing::info!(
                target: "nasiko::llm_router::startup",
                weights_source = source,
                model_trained_at = %trained_at,
                low_threshold = cfg.salience_low_threshold,
                high_threshold = cfg.salience_high_threshold,
                "llm-router: salience gate = ClassifierSalienceGate (Level 2.5 enabled; small talk answered cheaply without pinning, no LLM call in the hot path)"
            );
            Arc::new(gate)
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                weights_source = source,
                error = %e,
                "llm-router: salience model failed to load; falling back to AllowAllGate (classify at every fireable boundary)"
            );
            Arc::new(AllowAllGate)
        }
    }
}

/// Build the Level 3 request classifier from config.
///
/// The **one** construction path: [`LlmRouterCtx::from_shared`] and
/// `examples/classifier_eval.rs` both call it, so the eval exercises exactly what the router
/// runs. Reads no environment. Never fails: a backend that cannot be built (missing or invalid
/// weights, hosted endpoint/model unset, unknown name) logs a warning, wires
/// [`routing::RegexClassifier`] as the primary, and counts a `fallback_load`. The result is
/// always a [`GuardedClassifier`] applying `timeout_ms` and the regex fallback.
pub fn build_request_classifier(
    cfg: &ClassifierConfig,
    http: &reqwest::Client,
) -> Arc<GuardedClassifier> {
    use routing::{
        CascadeClassifier, ClassifierStats, HostedClassifier, LocalClassifier, RegexClassifier,
    };
    use std::sync::atomic::Ordering;

    let stats = Arc::new(ClassifierStats::default());
    let timeout = Duration::from_millis(cfg.timeout_ms.max(1));
    let degrade = |reason: &str| -> Arc<dyn RequestClassifier> {
        tracing::warn!(
            target: "nasiko::llm_router::startup",
            backend = %cfg.backend,
            reason,
            "llm-router: request classifier backend unavailable; using RegexClassifier"
        );
        stats.fallback_load.fetch_add(1, Ordering::Relaxed);
        Arc::new(RegexClassifier)
    };
    let load_local = || -> Result<Arc<LocalClassifier>, String> {
        let started = std::time::Instant::now();
        let loaded = if cfg.model_path.is_empty() {
            LocalClassifier::embedded()
        } else {
            LocalClassifier::from_path(&cfg.model_path)
        };
        let model = loaded.map_err(|e| e.to_string())?;
        tracing::info!(
            target: "nasiko::llm_router::startup",
            weights_source = if cfg.model_path.is_empty() { "embedded" } else { cfg.model_path.as_str() },
            load_ms = started.elapsed().as_secs_f64() * 1e3,
            provenance = %model.provenance(),
            "llm-router: local request classifier loaded"
        );
        Ok(Arc::new(model))
    };
    let hosted = || -> Option<Arc<HostedClassifier>> {
        if cfg.endpoint.is_empty() || cfg.model.is_empty() {
            return None;
        }
        let host = reqwest::Url::parse(&cfg.endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        tracing::info!(
            target: "nasiko::llm_router::startup",
            endpoint_host = %host,
            model = %cfg.model,
            timeout_ms = cfg.timeout_ms,
            api_key_set = !cfg.api_key.is_empty(),
            "llm-router: hosted request classifier configured"
        );
        Some(Arc::new(HostedClassifier::new(
            http.clone(),
            &cfg.endpoint,
            &cfg.model,
            &cfg.api_key,
            timeout,
            cfg.hosted_logprobs,
            cfg.hosted_default_confidence,
        )))
    };

    let primary: Arc<dyn RequestClassifier> = match cfg.backend.as_str() {
        "" | "regex" => Arc::new(RegexClassifier),
        "local" => match load_local() {
            Ok(m) => m,
            Err(e) => degrade(&e),
        },
        "hosted" => match hosted() {
            Some(h) => h,
            None => degrade("CLASSIFIER_ENDPOINT and CLASSIFIER_MODEL are required for hosted"),
        },
        "cascade" => match (load_local(), hosted()) {
            (Ok(local), Some(h)) => Arc::new(CascadeClassifier::new(
                local,
                h,
                cfg.escalate_below,
                timeout.mul_f64(0.8),
                stats.clone(),
            )),
            (Ok(local), None) => {
                tracing::warn!(
                    target: "nasiko::llm_router::startup",
                    "llm-router: cascade without CLASSIFIER_ENDPOINT/CLASSIFIER_MODEL; using the local classifier alone"
                );
                local
            }
            (Err(e), _) => degrade(&e),
        },
        other => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                backend = %other,
                "llm-router: unknown CLASSIFIER_BACKEND; using RegexClassifier"
            );
            Arc::new(RegexClassifier)
        }
    };
    tracing::info!(
        target: "nasiko::llm_router::startup",
        backend = primary.name(),
        timeout_ms = cfg.timeout_ms,
        min_confidence = cfg.min_confidence,
        complexity_routing = cfg.complexity_routing,
        "llm-router: request classifier = GuardedClassifier"
    );
    Arc::new(GuardedClassifier::new(primary, timeout, stats))
}

/// Choose the model-routing decision cache from config: [`build_shared_router_cache`], plus —
/// when `ROUTER_DECISION_L1_CAPACITY > 0` — an in-process L1 in front of it, so a single
/// instance keeps conversations sticky even without Redis. Capacity `0` (the default) returns
/// the shared cache unchanged.
fn build_router_cache(cfg: &GatewayConfig) -> Arc<dyn DecisionCache> {
    let shared = build_shared_router_cache(cfg);
    if cfg.router_decision_l1_capacity == 0 {
        return shared;
    }
    tracing::info!(
        target: "nasiko::llm_router::startup",
        capacity = cfg.router_decision_l1_capacity,
        ttl_secs = cfg.router_decision_ttl_secs,
        "llm-router: in-process L1 decision cache enabled in front of the shared cache"
    );
    Arc::new(routing::TieredDecisionCache::new(
        Arc::new(routing::InMemoryDecisionCache::new(
            cfg.router_decision_l1_capacity,
            Duration::from_secs(cfg.router_decision_ttl_secs),
        )),
        shared,
    ))
}

/// The shared decision cache: a [`RedisCache`] when `REDIS_URL` is set (and opens), otherwise
/// the fail-open [`NoopCache`]. A bad URL logs a warning and degrades to `NoopCache` rather
/// than failing startup — the cache is never load-bearing.
fn build_shared_router_cache(cfg: &GatewayConfig) -> Arc<dyn DecisionCache> {
    if cfg.redis_url.is_empty() {
        tracing::info!(
            target: "nasiko::llm_router::startup",
            "llm-router: REDIS_URL unset → decision cache = NoopCache (every request re-derives its model; fail-open)"
        );
        return Arc::new(NoopCache);
    }
    match redis::Client::open(cfg.redis_url.as_str()) {
        Ok(client) => {
            tracing::info!(
                target: "nasiko::llm_router::startup",
                ttl_secs = cfg.router_decision_ttl_secs,
                "llm-router: decision cache = RedisCache (conversation-sticky model decisions)"
            );
            Arc::new(RedisCache::new(client, cfg.router_decision_ttl_secs))
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                error = %e, "invalid REDIS_URL; router decision cache disabled (NoopCache)"
            );
            Arc::new(NoopCache)
        }
    }
}

/// Build the LLM router.
///
/// Mounted at the host's top level (outside user-session auth) — the agent-identity
/// JWT is verified inside these handlers. Agents reach these routes directly on the
/// server: the OpenAI-compatible surface lives under `/v1`, and Gemini under `/v1beta`
/// (each path mirrors what that provider's stock SDK appends to its base URL).
pub fn router(ctx: LlmRouterCtx) -> Router {
    Router::new()
        // Liveness probe owned by this router. The host server keeps its own
        // top-level `/health`; a future standalone binary will also map `/health`.
        .route("/v1/health", get(health))
        .route(
            "/v1/chat/completions",
            post(handlers::chat::chat_completions),
        )
        .route("/v1/responses", post(handlers::responses::responses))
        // Anthropic Messages surface — an Anthropic-SDK agent (`ANTHROPIC_BASE_URL`)
        // POSTs here; the inbound parser normalizes to the same IR (P2.3).
        .route("/v1/messages", post(handlers::chat::messages))
        // Gemini `generateContent` surface — a Gemini-SDK agent (`GOOGLE_GEMINI_BASE_URL`)
        // POSTs to `…/v1beta/models/{model}:generateContent` (or `:streamGenerateContent`);
        // the `{model}:{method}` segment is captured and the method picks (non-)streaming (P2.4).
        .route(
            "/v1beta/models/{model_method}",
            post(handlers::chat::gemini_generate),
        )
        .route("/v1/embeddings", post(handlers::embeddings::embeddings))
        .route("/v1/models", get(handlers::models::models))
        .with_state(ctx)
        .layer(RequestDecompressionLayer::new())
}

/// `GET /v1/health` → `{"status":"ok"}`.
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[cfg(test)]
mod transport_tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{Value, json};
    use std::future::poll_fn;
    use tower::Service;
    use tower_http::decompression::RequestDecompressionLayer;

    #[tokio::test]
    async fn request_decompression_accepts_codex_zstd_json() {
        async fn echo(Json(value): Json<Value>) -> Json<Value> {
            Json(value)
        }

        let mut app = Router::new()
            .route("/responses", post(echo))
            .layer(RequestDecompressionLayer::new());
        let expected =
            json!({"model":"gpt-5.4","stream":true,"input":[{"role":"user","content":"hello"}]});
        let compressed = zstd::stream::encode_all(expected.to_string().as_bytes(), 1).unwrap();
        let request = Request::post("/responses")
            .header("content-type", "application/json")
            .header("content-encoding", "zstd")
            .body(Body::from(compressed))
            .unwrap();
        poll_fn(|context| <Router as Service<Request<Body>>>::poll_ready(&mut app, context))
            .await
            .unwrap();
        let response = app.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
    }
}
