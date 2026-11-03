//! How a queued MAF job is turned into a run.
//!
//! The worker owns the queue, retries and the HITL bookkeeping; what a run *does*
//! with its definition is a [`MafExecutor`]. The open-source executor is
//! [`LinearMafExecutor`], which runs the steps in array order. A distribution
//! that executes definitions differently replaces it on the server's `AppState`
//! at its composition root, the same way it replaces the routing engine.

use std::sync::Arc;

use async_trait::async_trait;
use nasiko_flow::FlowGuard;
use nasiko_types::agent_call::AgentCallMeter;
use sqlx::PgPool;
use uuid::Uuid;

use super::executor;
use super::llm::LlmClient;
use super::types::{MafDefinition, StepOutcome, StepResult};
use crate::outbound::OutboundPrefix;

/// Everything a run needs that does not come from the job itself.
pub struct MafRunContext<'a> {
    /// The worker's agent-call client, built with the platform's agent-call budget.
    pub client: &'a reqwest::Client,
    /// Counts each step's agent call as in flight to its agent.
    pub agent_calls: &'a Arc<dyn AgentCallMeter>,
    /// The organization policy prefix for each step's message and the final answer.
    pub outbound: &'a dyn OutboundPrefix,
    pub db: &'a PgPool,
    /// The guard every other inter-agent call is bounded by.
    pub flow_guard: &'a Arc<FlowGuard>,
    /// MAF's own LLM (planning, prompt filling, extraction, synthesis).
    pub llm: &'a LlmClient,
    pub execution_id: Uuid,
    /// The run's owner.
    pub user_id: Uuid,
}

/// A paused run's durable state plus the human's answer, as the HITL resume
/// dispatcher hands it back.
pub struct MafResume {
    pub step_results: Vec<StepResult>,
    pub tokens_used: i64,
    pub cost_used: f64,
    pub output_generation: String,
    /// The step that paused.
    pub step_index: usize,
    /// The task id the pause was recorded under.
    pub task_id: String,
    /// The A2A `contextId` the paused step ran under.
    pub context_id: String,
    pub answer: String,
}

/// Runs one MAF execution: a fresh run or a retry (`run`), or the continuation
/// of a run that paused for a human (`resume`).
#[async_trait]
pub trait MafExecutor: Send + Sync {
    async fn run(
        &self,
        ctx: &MafRunContext<'_>,
        maf_def: &MafDefinition,
        content: Option<&str>,
    ) -> Result<StepOutcome, String>;

    async fn resume(
        &self,
        ctx: &MafRunContext<'_>,
        maf_def: &MafDefinition,
        resume: MafResume,
    ) -> Result<StepOutcome, String>;
}

/// The open-source executor: every step, in array order.
pub struct LinearMafExecutor;

#[async_trait]
impl MafExecutor for LinearMafExecutor {
    async fn run(
        &self,
        ctx: &MafRunContext<'_>,
        maf_def: &MafDefinition,
        content: Option<&str>,
    ) -> Result<StepOutcome, String> {
        executor::run_maf(
            ctx.client,
            ctx.db,
            ctx.flow_guard,
            ctx.agent_calls,
            ctx.execution_id,
            ctx.user_id,
            maf_def,
            ctx.llm,
            content,
            ctx.outbound,
        )
        .await
    }

    async fn resume(
        &self,
        ctx: &MafRunContext<'_>,
        maf_def: &MafDefinition,
        resume: MafResume,
    ) -> Result<StepOutcome, String> {
        executor::run_maf_from(
            ctx.client,
            ctx.db,
            ctx.flow_guard,
            ctx.agent_calls,
            ctx.execution_id,
            ctx.user_id,
            maf_def,
            ctx.llm,
            resume.step_results,
            resume.tokens_used,
            resume.cost_used,
            resume.output_generation,
            resume.step_index,
            resume.task_id,
            resume.context_id,
            resume.answer,
            ctx.outbound,
        )
        .await
    }
}
