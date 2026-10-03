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

pub use config::GatewayConfig;
pub use error::GatewayError;
pub use inbound::InboundFormat;
pub use inject::{LlmInjectCtx, inject_llm_env};
pub use resolver::{ConfigCache, ResolvedConfig};
pub use routing::{
    AllowAllGate, CellStore, ClassifierSalienceGate, DecisionCache, InMemoryCellStore, NoopCache,
    PgCellStore, PgTierRegistry, RedisCache, SalienceGate, TierRegistry,
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

    /// Model-routing decision cache, keyed on `(conv_id, agent_id)`.
    /// [`NoopCache`] by default; Redis swaps in a Redis-backed implementation
    /// when configured.
    pub router_cache: Arc<dyn DecisionCache>,

    /// Tier→model registry for classified routing.
    ///
    /// [`PgTierRegistry`] in production: operator `model_registry` overrides first,
    /// then a mapping derived from the live provider model catalog.
    pub tier_registry: Arc<dyn TierRegistry>,

    /// Learned per-provider quality cells behind Thompson-sampling tier selection.
    ///
    /// [`PgCellStore`] is used in production; tests use [`InMemoryCellStore`].
    pub cell_store: Arc<dyn CellStore>,

    /// Level 2.5 salience gate.
    ///
    /// [`ClassifierSalienceGate`] is used when `SALIENCE_GATE_ENABLED` is enabled;
    /// otherwise [`AllowAllGate`] preserves the previous behavior.
    pub salience_gate: Arc<dyn SalienceGate>,

    /// Level 3 request classifier.
    ///
    /// The concrete backend is selected from [`GatewayConfig`]. The classifier is
    /// created once during router initialization and shared across requests.
    ///
    /// The default implementation is [`routing::RegexClassifier`]. Hosted/model
    /// classification automatically falls back to the regex implementation when
    /// initialization, network access, inference, timeout, parsing, or validation
    /// fails.
    pub request_classifier: Arc<dyn routing::RequestClassifier>,

    /// The platform's single cost engine.
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
            classifier_backend = %cfg.classifier_backend,
            classifier_model = %cfg.classifier_model,
            classifier_endpoint = %cfg.classifier_endpoint,
            classifier_timeout_secs = cfg.classifier_timeout_secs,
            classifier_api_key_set = !cfg.classifier_api_key.is_empty(),
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

        // IMPORTANT:
        // The classifier is constructed once at startup rather than for every
        // request. This keeps model/client initialization out of the hot path.
        let request_classifier = build_request_classifier(&cfg);

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

/// Build the Level 3 request classifier from configuration.
///
/// Regex is the default and remains the fail-safe implementation.
///
/// Hosted/model classification is initialized once at router startup and shared
/// through [`LlmRouterCtx`].
///
/// If the hosted classifier cannot initialize because of configuration or client
/// construction problems, the router immediately falls back to RegexClassifier.
///
/// Runtime failures are handled inside HostedClassifier itself and also fall back
/// to the regex classifier.
fn build_request_classifier(cfg: &Arc<GatewayConfig>) -> Arc<dyn routing::RequestClassifier> {
    let backend = cfg.classifier_backend.trim().to_ascii_lowercase();

    match backend.as_str() {
        "groq" | "hosted" | "openai-compatible" => {
            if cfg.classifier_api_key.trim().is_empty() {
                tracing::warn!(
                    target: "nasiko::llm_router::startup",
                    backend = %backend,
                    "llm-router: hosted request classifier selected but no classifier API key is configured; falling back to RegexClassifier"
                );

                return Arc::new(routing::RegexClassifier);
            }

            match routing::HostedClassifier::new(
                cfg.classifier_endpoint.clone(),
                cfg.classifier_model.clone(),
                cfg.classifier_api_key.clone(),
                Duration::from_secs(cfg.classifier_timeout_secs),
            ) {
                Ok(classifier) => {
                    tracing::info!(
                        target: "nasiko::llm_router::startup",
                        backend = %backend,
                        model = %cfg.classifier_model,
                        endpoint = %cfg.classifier_endpoint,
                        timeout_secs = cfg.classifier_timeout_secs,
                        "llm-router: request classifier = hosted"
                    );

                    Arc::new(classifier)
                }

                Err(error) => {
                    tracing::warn!(
                        target: "nasiko::llm_router::startup",
                        backend = %backend,
                        error = %error,
                        "llm-router: hosted request classifier failed to initialize; falling back to RegexClassifier"
                    );

                    Arc::new(routing::RegexClassifier)
                }
            }
        }

        "regex" | "" => {
            tracing::info!(
                target: "nasiko::llm_router::startup",
                "llm-router: request classifier = RegexClassifier"
            );

            Arc::new(routing::RegexClassifier)
        }

        other => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                backend = %other,
                "llm-router: unknown request classifier backend; falling back to RegexClassifier"
            );

            Arc::new(routing::RegexClassifier)
        }
    }
}

/// Build the Level 2.5 salience gate from config.
///
/// [`AllowAllGate`] when `SALIENCE_GATE_ENABLED=false`; otherwise
/// [`ClassifierSalienceGate`] is loaded from the embedded model or the configured
/// weights path.
///
/// A model that cannot be loaded degrades to [`AllowAllGate`].
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
                "llm-router: salience gate = ClassifierSalienceGate (Level 2.5 enabled)"
            );

            Arc::new(gate)
        }

        Err(error) => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                weights_source = source,
                error = %error,
                "llm-router: salience model failed to load; falling back to AllowAllGate"
            );

            Arc::new(AllowAllGate)
        }
    }
}

/// Choose the model-routing decision cache from config.
///
/// Redis is used when `REDIS_URL` is configured. Otherwise the router falls back
/// to [`NoopCache`].
fn build_router_cache(cfg: &GatewayConfig) -> Arc<dyn DecisionCache> {
    if cfg.redis_url.is_empty() {
        tracing::info!(
            target: "nasiko::llm_router::startup",
            "llm-router: REDIS_URL unset → decision cache = NoopCache (fail-open)"
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

        Err(error) => {
            tracing::warn!(
                target: "nasiko::llm_router::startup",
                error = %error,
                "invalid REDIS_URL; router decision cache disabled (NoopCache)"
            );

            Arc::new(NoopCache)
        }
    }
}

/// Build the LLM router.
///
/// Mounted at the host's top level (outside user-session auth) — the agent-identity
/// JWT is verified inside these handlers.
pub fn router(ctx: LlmRouterCtx) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route(
            "/v1/chat/completions",
            post(handlers::chat::chat_completions),
        )
        .route("/v1/responses", post(handlers::responses::responses))
        .route("/v1/messages", post(handlers::chat::messages))
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
