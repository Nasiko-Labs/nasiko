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
use serde_json::{Value, json};
use sqlx::PgPool;

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod inbound;
pub mod inject;
pub mod ir;
pub mod providers;
pub mod resolver;
pub mod routing;
pub mod usage;

pub use config::GatewayConfig;
pub use error::GatewayError;
pub use inbound::InboundFormat;
pub use inject::{LlmInjectCtx, inject_llm_env};
pub use resolver::{ConfigCache, ResolvedConfig};
pub use routing::{
    DecisionCache, NoopCache, PgTierRegistry, RedisCache, StaticTierRegistry, TierRegistry,
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
    /// Tier→model registry for classified routing. [`PgTierRegistry`] (DB-backed, static
    /// seed fallback) in production; tests use [`StaticTierRegistry`].
    pub tier_registry: Arc<dyn TierRegistry>,
}

impl LlmRouterCtx {
    /// Build from resources the host already owns (the server's `PgPool` + HTTP
    /// client). Gateway-specific config is read from the environment.
    pub fn from_shared(db: PgPool, http: reqwest::Client) -> Self {
        let cfg = GatewayConfig::from_env();
        let cache = Arc::new(ConfigCache::new(Duration::from_secs(cfg.llm_config_cache_ttl_secs)));
        let tier_registry = Arc::new(PgTierRegistry::new(db.clone()));
        let router_cache = build_router_cache(&cfg);
        Self {
            db,
            http,
            cfg: Arc::new(cfg),
            cache,
            router_cache,
            tier_registry,
        }
    }
}

/// Choose the model-routing decision cache from config: a [`RedisCache`] when `REDIS_URL`
/// is set (and opens), otherwise the fail-open [`NoopCache`]. A bad URL logs a warning and
/// degrades to `NoopCache` rather than failing startup — the cache is never load-bearing.
fn build_router_cache(cfg: &GatewayConfig) -> Arc<dyn DecisionCache> {
    if cfg.redis_url.is_empty() {
        return Arc::new(NoopCache);
    }
    match redis::Client::open(cfg.redis_url.as_str()) {
        Ok(client) => Arc::new(RedisCache::new(client, cfg.router_decision_ttl_secs)),
        Err(e) => {
            tracing::warn!(error = %e, "invalid REDIS_URL; router decision cache disabled");
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
}

/// `GET /v1/health` → `{"status":"ok"}`.
async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
