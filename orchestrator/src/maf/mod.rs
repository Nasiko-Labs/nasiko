// ── MAF (Multi-Agent Flow) orchestrator ──────────────────────────────────────
pub mod executor;
pub mod llm;
pub mod planner;
pub mod types;
mod worker;

use std::sync::Arc;

use nasiko_observability::ObservabilityProvider;
use sqlx::PgPool;

use llm::LlmClient;

/// Configuration for the LLM used by the MAF executor.
pub struct LlmConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: String,
}

/// Spawn the MAF background worker as a detached tokio task.
/// Call once at server startup — the worker reads from the Redis stream
/// `nasiko:maf:execute` and drives execution of queued MAF jobs.
pub fn start_worker(
    db: PgPool,
    redis: redis::Client,
    http_client: reqwest::Client,
    observability: Arc<dyn ObservabilityProvider>,
    llm_config: LlmConfig,
) {
    let llm = LlmClient::new(http_client.clone(), llm_config.api_key, llm_config.base_url, llm_config.model);
    tokio::spawn(worker::run(db, redis, http_client, observability, llm));
}
