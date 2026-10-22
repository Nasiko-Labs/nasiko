// ── MAF (Multi-Agent Flow) orchestrator ──────────────────────────────────────
pub mod decomposer;
pub mod executor;
pub mod llm;
pub mod planner;
pub mod types;
mod worker;

/// The Redis stream key this crate's worker consumes from — re-exported so every producer
/// (`oss/server/src/maf.rs`, `oss/server/src/hitl/mod.rs`) binds to the same constant instead of
/// hardcoding the literal a second and third time (found in review — see `worker::STREAM_KEY`'s
/// own doc comment).
pub use worker::STREAM_KEY;

use std::sync::Arc;

use nasiko_flow::FlowGuard;
use nasiko_types::agent_call::AgentCallMeter;
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
///
/// How the worker reaches agents: the client for its calls to them and the meter that counts
/// each call as in flight. MAF calls agents directly, not through the server's proxy, so both
/// are received from the composition root.
pub struct AgentCalls {
    /// Serves the worker's calls to agents. They carry no per-request timeout, so this client
    /// carries the agent call budget.
    pub client: reqwest::Client,
    /// Counts each step's agent call as in flight to its agent.
    pub meter: Arc<dyn AgentCallMeter>,
    /// Prefixes each step's outbound message to the agent.
    pub outbound: Arc<dyn crate::outbound::OutboundPrefix>,
}

/// `http_client` serves the worker's LLM calls; `agent_calls` its calls to agents.
pub fn start_worker(
    db: PgPool,
    redis: redis::Client,
    http_client: reqwest::Client,
    agent_calls: AgentCalls,
    // Cascade-limit enforcement for each step's agent call. Received from the
    // composition root rather than constructed here, so MAF shares the very
    // same guard instance the A2A dispatch and proxy paths use.
    flow_guard: Arc<FlowGuard>,
    llm_config: LlmConfig,
    hitl_store: Arc<dyn nasiko_hitl::HitlStore>,
) {
    let llm = LlmClient::new(
        http_client,
        llm_config.api_key,
        llm_config.base_url,
        llm_config.model,
    );
    tokio::spawn(worker::run(
        db,
        redis,
        agent_calls,
        flow_guard,
        llm,
        hitl_store,
    ));
}
