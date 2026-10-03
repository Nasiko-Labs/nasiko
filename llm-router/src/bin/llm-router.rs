//! Standalone `nasiko-llm-router` binary (Phase 2, P2.9) — **optional**.
//!
//! The router normally runs in-process inside `nasiko-server` (see `lib.rs`). This binary
//! serves the exact same `router(ctx)` as its own process, for independent scaling / fault
//! isolation. It shares the platform Postgres and **does not run migrations** — the server
//! owns the schema, so point this at an already-migrated database. There is no current
//! scaling driver for the split; this exists so the option is one `cargo build` away.
//!
//! Env: `DATABASE_URL` (required), `LLM_ROUTER_BIND` (default `0.0.0.0:8081`), plus the
//! gateway config read by `GatewayConfig::from_env` (`AGENT_JWT_SECRET`,
//! `SECRETS_ENCRYPTION_KEY`, `PLATFORM_OPENAI_API_KEY`, provider bases, …).
//!
//! Request classifier (opt-in, default `regex`): `CLASSIFIER_BACKEND`, `CLASSIFIER_MODEL`,
//! `CLASSIFIER_ENDPOINT`, `CLASSIFIER_API_KEY`, `CLASSIFIER_TIMEOUT_MS`,
//! `CLASSIFIER_MIN_CONFIDENCE` — read in `llm-router/config.rs`, see `llm-router/README.md`.

use std::time::Duration;

use nasiko_llm_router::{LlmRouterCtx, build_classifier, router};
use tracing_subscriber::EnvFilter;

// Classifier backend settings live in the binary, not the library.
#[path = "llm-router/config.rs"]
mod config;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for the standalone llm-router");
    let bind = std::env::var("LLM_ROUTER_BIND").unwrap_or_else(|_| "0.0.0.0:8081".to_string());

    // Connect to the shared, already-migrated Postgres (this binary never migrates).
    let db = sqlx::PgPool::connect(&database_url)
        .await
        .expect("failed to connect to postgres");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("failed to build http client");

    // Same context + routes as the in-server mount; gateway config from env.
    let mut ctx = LlmRouterCtx::from_shared(db, http);

    // Request classifier: regex unless the operator opts into another backend. A backend
    // that fails or times out falls back to regex per request, so this never blocks routing.
    let classifier_settings = config::classifier_settings_from(|k| std::env::var(k).ok());
    tracing::info!(
        backend = %classifier_settings.backend,
        model = %classifier_settings.model,
        endpoint = %classifier_settings.endpoint,
        timeout_ms = classifier_settings.timeout_ms,
        api_key_set = classifier_settings.api_key.is_some(),
        "llm-router: request classifier configured"
    );
    ctx.classifier = Some(build_classifier(&classifier_settings));
    let app = router(ctx).route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind}: {e}"));
    tracing::info!("nasiko-llm-router (standalone) listening on {bind}");
    axum::serve(listener, app).await.expect("server error");
}
