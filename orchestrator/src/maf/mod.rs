// ── MAF (Multi-Agent Flow) orchestrator ──────────────────────────────────────
pub mod decomposer;
pub mod executor;
pub mod llm;
pub mod planner;
pub mod runner;
pub mod types;
mod worker;

pub use runner::{LinearMafExecutor, MafExecutor, MafResume, MafRunContext};

/// The Redis stream key this crate's worker consumes from — re-exported so every producer
/// (`oss/server/src/maf.rs`, `oss/server/src/hitl/mod.rs`) binds to the same constant instead of
/// hardcoding the literal a second and third time (found in review — see `worker::STREAM_KEY`'s
/// own doc comment).
pub use worker::STREAM_KEY;

use std::sync::Arc;

use nasiko_flow::FlowGuard;
use sqlx::PgPool;

use llm::LlmClient;

/// Configuration for the LLM used by the MAF executor.
pub struct LlmConfig {
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: String,
}

/// How the MAF worker runs: the LLM its executor uses, and how many executions
/// it keeps in flight at once (`Config::maf_worker_concurrency`).
pub struct WorkerConfig {
    pub llm: LlmConfig,
    pub concurrency: usize,
}

/// Spawn the MAF background worker as a detached tokio task.
/// Call once at server startup — the worker reads from the Redis stream
/// `nasiko:maf:execute` and drives up to `config.concurrency` queued MAF jobs at once.
pub fn start_worker(
    db: PgPool,
    redis: redis::Client,
    http_client: reqwest::Client,
    // Cascade-limit enforcement for each step's agent call. Received from the
    // composition root rather than constructed here, so MAF shares the very
    // same guard instance the A2A dispatch and proxy paths use.
    flow_guard: Arc<FlowGuard>,
    config: WorkerConfig,
    hitl_store: Arc<dyn nasiko_hitl::HitlStore>,
    // What each job runs through — received from the composition root, like the guard.
    maf_executor: Arc<dyn MafExecutor>,
) {
    let llm = LlmClient::new(
        http_client.clone(),
        config.llm.api_key,
        config.llm.base_url,
        config.llm.model,
    );
    let deps = worker::JobDeps {
        db,
        http_client,
        flow_guard,
        llm,
        hitl_store,
        maf_executor,
    };
    tokio::spawn(worker::run(deps, redis, config.concurrency));
}
