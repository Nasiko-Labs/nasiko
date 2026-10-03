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

use std::time::Duration;

use nasiko_llm_router::{LlmRouterCtx, router};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let bind = std::env::var("LLM_ROUTER_BIND")
        .or_else(|_| std::env::var("PORT").map(|p| format!("0.0.0.0:{p}")))
        .unwrap_or_else(|_| "0.0.0.0:8081".to_string());

    let app = match std::env::var("DATABASE_URL") {
        Ok(database_url) => {
            tracing::info!("connecting to Postgres at {database_url}");
            let db = sqlx::PgPool::connect(&database_url)
                .await
                .expect("failed to connect to postgres");
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("failed to build http client");

            let ctx = LlmRouterCtx::from_shared(db, http);
            router(ctx).route("/health", axum::routing::get(|| async { "ok" }))
        }
        Err(_) => {
            tracing::info!(
                "DATABASE_URL not set; running in standalone classifier mode with /classify and /health"
            );
            nasiko_llm_router::classifier_router()
        }
    };

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind}: {e}"));
    tracing::info!("nasiko-llm-router listening on {bind}");
    axum::serve(listener, app).await.expect("server error");
}
