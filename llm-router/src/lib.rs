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

use attachments::{
    ATTACHMENT_CACHE_TTL, AttachmentCache, AttachmentStore, MarkerSigner, NoAttachmentStore,
};
use axum::{
    Json, Router,
    http::{Method, StatusCode, Uri},
    routing::{any, get, post},
};
use nasiko_pricing::{NoUsageLedger, PricingEngine, UsageLedger};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower_http::decompression::RequestDecompressionLayer;

pub mod attachments;
pub mod auth;
mod brevity;
mod compact_tools;
mod compress;
pub mod config;
pub mod error;
pub mod handlers;
pub mod inbound;
pub mod inject;
pub mod ir;
pub mod openai_model;
pub mod payer;
pub mod platform;
pub mod providers;
mod range_view;
pub mod recovery;
pub mod resolver;
pub mod routing;
mod savings;
pub mod turns;
pub mod usage;

pub use config::GatewayConfig;
pub use error::GatewayError;
pub use inbound::InboundFormat;
pub use inject::{LlmInjectCtx, inject_llm_base_url, inject_llm_env};
pub use resolver::{ConfigCache, ResolvedConfig, ResolvedEmbeddingConfig};
pub use routing::{
    AllowAllGate, CellStore, ClassifierSalienceGate, DecisionCache, InMemoryCellStore, NoopCache,
    PgCellStore, PgTierRegistry, RedisCache, SalienceGate, TierRegistry,
};

/// Shared context for the LLM router.
///
/// Holds the resources the router needs. The host supplies the `PgPool`; the router
/// builds its own provider HTTP client (see [`provider_client`]). Cheap to clone — it is
/// the Axum router state.
#[derive(Clone)]
pub struct LlmRouterCtx {
    /// Postgres pool — reads `agents.llm_config` / `user_secrets`, writes `token_usage`.
    pub db: PgPool,
    /// Pooled outbound HTTP client for provider calls, built by [`provider_client`].
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
    /// Request-type classifier used at Level 3 of routing. [`routing::RegexClassifier`] by
    /// default (behaviour unchanged); `CLASSIFIER_BACKEND=local|hosted` swaps in a guarded model
    /// backend that falls back to regex on any failure. See [`build_classifier`].
    pub classifier: Arc<dyn routing::RequestClassifier>,
    /// The platform's single cost engine. Every `token_usage` row is priced
    /// through this — the DB trigger that used to do it returned NULL for any
    /// model missing from `model_pricing`, which booked 92.8% of calls at $0.
    pub pricing: Arc<PricingEngine>,
    pub body_permits: Arc<tokio::sync::Semaphore>,
    pub attachment_store: Arc<dyn AttachmentStore>,
    pub attachment_cache: Arc<AttachmentCache>,
    pub marker_signer: Option<Arc<MarkerSigner>>,

    /// Receives each priced call on the transaction of its `trace_usage` write.
    /// [`NoUsageLedger`] unless the host supplies its own.
    pub ledger: Arc<dyn UsageLedger>,
    /// Who pays for a call another user makes through an agent. [`payer::CallerPays`] unless the host
    /// supplies its own.
    pub payer: Arc<dyn payer::SharedCallPayer>,
    /// Where a discovered A2A agent's calls go as conversation turns. [`turns::NoTurnRecorder`]
    /// by default; the server swaps in one that keeps them as a chat session.
    pub turn_recorder: Arc<dyn turns::TurnRecorder>,
}

impl LlmRouterCtx {
    /// Build from the host's `PgPool`. Gateway-specific config is read from the
    /// environment.
    ///
    /// The provider client is the router's own, not the host's: a host client is
    /// sized for short calls, and its total timeout cut off slow model replies and
    /// long streams alike.
    pub fn from_shared(db: PgPool) -> Self {
        let cfg = GatewayConfig::from_env();
        let http = provider_client(Duration::from_secs(cfg.provider_timeout_secs));
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
            provider_timeout_secs = cfg.provider_timeout_secs,
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
        let classifier = build_classifier(&cfg, http.clone());
        let pricing = Arc::new(PricingEngine::new(db.clone()));
        let body_permits = handlers::auth_layer::body_permit_pool(cfg.max_inflight_body_bytes);
        let attachment_cache = Arc::new(AttachmentCache::new(
            cfg.attachment_cache_bytes,
            ATTACHMENT_CACHE_TTL,
        ));
        let marker_signer = MarkerSigner::from_secret(&cfg.agent_jwt_secret).map(Arc::new);
        Self {
            db,
            http,
            cfg,
            cache,
            router_cache,
            tier_registry,
            cell_store,
            salience_gate,
            classifier,
            pricing,

            body_permits,
            attachment_store: Arc::new(NoAttachmentStore),
            attachment_cache,
            marker_signer,
            ledger: Arc::new(NoUsageLedger),
            payer: Arc::new(payer::CallerPays),
            turn_recorder: Arc::new(turns::NoTurnRecorder),
        }
    }

    pub fn with_config_cache(mut self, cache: Arc<ConfigCache>) -> Self {
        self.cache = cache;
        self
    }

    pub fn with_attachment_store(mut self, store: Arc<dyn AttachmentStore>) -> Self {
        self.attachment_store = store;
        self
    }
}

/// The outbound client for provider calls.
///
/// `read_timeout`, not `timeout`: it restarts after every read, so a stream that keeps
/// sending chunks runs as long as it needs, a non-streaming call (silent until the reply
/// is ready) still fails after `read_timeout`, and so does a stalled connection.
pub fn provider_client(read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(20)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(read_timeout)
        .build()
        .expect("failed to build provider http client")
}

/// Build the request classifier from config. The only place backend selection happens, shared by
/// the router and the `classifier_eval` example so both exercise the same path.
///
/// `local` (the default) and `hosted` are wrapped in a [`routing::GuardedClassifier`] (see
/// [`build_guarded_classifier`]), which answers with the regex classifier when the model errors or
/// times out; `regex` or an unknown value is returned bare.
pub fn build_classifier(
    cfg: &GatewayConfig,
    http: reqwest::Client,
) -> Arc<dyn routing::RequestClassifier> {
    match build_guarded_classifier(cfg, http) {
        Some(guard) => guard,
        None => Arc::new(routing::RegexClassifier),
    }
}

/// The configured model backend behind its [`routing::GuardedClassifier`] (timeout, regex
/// fallback, low-confidence safe default), or `None` when routing uses the bare regex
/// classifier: `CLASSIFIER_BACKEND=regex` / unknown, a `local` model that fails to load, or a
/// `hosted` config with no endpoint (the last two log a warning — a degrade, never an outage).
///
/// Returned concretely so a caller that needs the guard's fallback counters (the
/// `classifier_eval` example) can read [`routing::GuardedClassifier::stats`].
pub fn build_guarded_classifier(
    cfg: &GatewayConfig,
    http: reqwest::Client,
) -> Option<Arc<routing::GuardedClassifier>> {
    use routing::{
        GuardedClassifier, classifier::linear::LinearClassifier,
        classifier_hosted::HostedClassifier,
    };
    let timeout = Duration::from_millis(cfg.classifier_timeout_ms.max(1));
    let min_conf = cfg.classifier_min_confidence;
    let inner: Arc<dyn routing::RequestClassifier> = match cfg.classifier_backend.as_str() {
        "local" => {
            let (source, model) = if cfg.classifier_model_path.is_empty() {
                ("embedded", LinearClassifier::embedded())
            } else {
                (
                    cfg.classifier_model_path.as_str(),
                    LinearClassifier::from_path(&cfg.classifier_model_path),
                )
            };
            match model {
                Ok(m) => {
                    tracing::info!(target: "nasiko::llm_router::startup", weights_source = source, provenance = %m.provenance(), "classifier: local model loaded");
                    Arc::new(m)
                }
                Err(e) => {
                    tracing::warn!(target: "nasiko::llm_router::startup", weights_source = source, error = %e, "classifier: local model load failed; using regex");
                    return None;
                }
            }
        }
        "hosted" if !cfg.classifier_endpoint.is_empty() => Arc::new(HostedClassifier::new(
            http,
            cfg.classifier_endpoint.clone(),
            cfg.classifier_model.clone(),
            cfg.classifier_api_key.clone(),
        )),
        "hosted" => {
            tracing::warn!(target: "nasiko::llm_router::startup", "classifier: hosted backend needs CLASSIFIER_ENDPOINT; using regex");
            return None;
        }
        _ => return None,
    };
    tracing::info!(target: "nasiko::llm_router::startup", backend = inner.name(), timeout_ms = cfg.classifier_timeout_ms, min_confidence = min_conf, "llm-router: request classifier enabled");
    Some(Arc::new(GuardedClassifier::new(inner, timeout, min_conf)))
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

/// Choose the model-routing decision cache from config: a [`RedisCache`] when `REDIS_URL`
/// is set (and opens), otherwise the fail-open [`NoopCache`]. A bad URL logs a warning and
/// degrades to `NoopCache` rather than failing startup — the cache is never load-bearing.
fn build_router_cache(cfg: &GatewayConfig) -> Arc<dyn DecisionCache> {
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
            Arc::new(RedisCache::new(
                nasiko_redis::SharedRedis::new(client),
                cfg.router_decision_ttl_secs,
            ))
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
    let llm_routes = Router::new()
        .route(
            "/v1/chat/completions",
            post(handlers::chat::chat_completions),
        )
        .route("/v1/responses", post(handlers::responses::responses))
        // Anthropic Messages surface — an Anthropic-SDK agent (`ANTHROPIC_BASE_URL`)
        // POSTs here; the inbound parser normalizes to the same IR (P2.3).
        .route("/v1/messages", post(handlers::chat::messages))
        .route(
            "/v1/messages/count_tokens",
            post(handlers::chat::count_tokens),
        )
        // Gemini `generateContent` surface — a Gemini-SDK agent (`GOOGLE_GEMINI_BASE_URL`)
        // POSTs to `…/v1beta/models/{model}:generateContent` (or `:streamGenerateContent`);
        // the `{model}:{method}` segment is captured and the method picks (non-)streaming (P2.4).
        .route(
            "/v1beta/models/{model_method}",
            post(handlers::chat::gemini_generate),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            handlers::auth_layer::authenticate_first,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(ctx.cfg.max_body_bytes));
    Router::new()
        // Liveness probe owned by this router. The host server keeps its own
        // top-level `/health`; a future standalone binary will also map `/health`.
        .route("/v1/health", get(health))
        .merge(llm_routes)
        .route("/v1/embeddings", post(handlers::embeddings::embeddings))
        .route("/v1/models", get(handlers::models::models))
        .route("/v1/{*rest}", any(unknown_endpoint))
        .route("/v1beta/{*rest}", any(unknown_endpoint))
        .with_state(ctx)
        .layer(RequestDecompressionLayer::new())
}

/// `GET /v1/health` → `{"status":"ok"}`.
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn unknown_endpoint(method: Method, uri: Uri) -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_FOUND,
        crate::error::error_body(
            StatusCode::NOT_FOUND,
            &format!("no such router endpoint: {method} {}", uri.path()),
        ),
    )
}

#[cfg(test)]
mod provider_client_tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::provider_client;

    const READ_TIMEOUT: Duration = Duration::from_millis(500);

    /// Accepts one connection, waits for the request head, then hands the socket to
    /// `respond`. Returns the URL to call.
    async fn serve_once<F, Fut>(respond: F) -> String
    where
        F: FnOnce(TcpStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                head.extend_from_slice(&buf[..n]);
            }
            respond(socket).await;
        });
        url
    }

    async fn reply_after(delay: Duration, mut socket: TcpStream) {
        tokio::time::sleep(delay).await;
        // The client may already have given up; a failed write is the expected outcome then.
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await;
    }

    async fn stream_chunks(count: usize, every: Duration, mut socket: TcpStream) {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for _ in 0..count {
            tokio::time::sleep(every).await;
            socket.write_all(b"1\r\nx\r\n").await.unwrap();
        }
        socket.write_all(b"0\r\n\r\n").await.unwrap();
    }

    #[tokio::test]
    async fn reply_slower_than_the_read_timeout_fails() {
        let url = serve_once(|s| reply_after(READ_TIMEOUT * 3, s)).await;
        let err = provider_client(READ_TIMEOUT)
            .get(url)
            .send()
            .await
            .unwrap_err();
        assert!(err.is_timeout(), "expected a timeout, got {err}");
    }

    #[tokio::test]
    async fn reply_inside_the_read_timeout_succeeds() {
        let url = serve_once(|s| reply_after(READ_TIMEOUT / 5, s)).await;
        let body = provider_client(READ_TIMEOUT)
            .get(url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn steady_stream_longer_than_the_read_timeout_arrives_complete() {
        // 8 chunks 200 ms apart: 1.6 s in all, more than 3× the read timeout, which a
        // total `timeout` of the same length would have cut.
        let url = serve_once(|s| stream_chunks(8, READ_TIMEOUT * 2 / 5, s)).await;
        let body = provider_client(READ_TIMEOUT)
            .get(url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "x".repeat(8));
    }
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

#[cfg(test)]
mod classifier_default_tests {
    use super::*;
    use routing::classifier::linear::LinearClassifier;

    #[test]
    fn the_default_classifier_is_the_local_model() {
        let classifier = build_classifier(&GatewayConfig::default(), reqwest::Client::new());
        assert_eq!(classifier.name(), LinearClassifier::NAME);
    }

    #[test]
    fn regex_stays_available_and_is_the_answer_for_an_unknown_backend() {
        for backend in ["regex", "no-such-backend"] {
            let cfg = GatewayConfig {
                classifier_backend: backend.into(),
                ..GatewayConfig::default()
            };
            assert_eq!(
                build_classifier(&cfg, reqwest::Client::new()).name(),
                "regex"
            );
        }
    }

    #[test]
    fn a_local_model_that_fails_to_load_falls_back_to_regex() {
        let cfg = GatewayConfig {
            classifier_model_path: "/no/such/weights.json".into(),
            ..GatewayConfig::default()
        };
        assert_eq!(
            build_classifier(&cfg, reqwest::Client::new()).name(),
            "regex"
        );
    }
}
