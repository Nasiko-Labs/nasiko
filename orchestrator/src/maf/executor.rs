use std::time::Instant;

use nasiko_observability::ObservabilityProvider;
use nasiko_types::a2a::{PauseReason, StreamDisposition, build_send_request_for_task};
use sqlx::PgPool;
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient};
use super::types::{ExecutionResult, MafDefinition, MafStep, PausedStep, StepOutcome, StepResult};

/// What one step's agent call produced, before extraction — a normal reply, or a pause that must
/// stop the step (and the whole run) right there instead of being treated as an oddly-worded
/// normal reply.
enum AgentCallOutcome {
    Completed(String),
    Paused { task_id: String, raw_data: String },
}

/// What one step's execution produced — shared between a fresh step (`execute_step`) and a
/// resumed one (`continue_paused_step`), so `run_maf`/`run_maf_from` branch on the same three
/// outcomes regardless of which path produced them.
enum StepLoopOutcome {
    Advance,
    AwaitingHuman(PausedStep),
    Failed(String),
}

pub async fn run_maf(
    client: &reqwest::Client,
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
) -> Result<StepOutcome, String> {
    // Seed one "pending" entry per step and persist immediately, so the full
    // step list is visible in the DB before the (possibly slow) planning LLM
    // call even starts.
    let mut step_results: Vec<StepResult> = maf_def.steps.iter().map(pending_result).collect();
    let mut total_cost = 0f64;
    persist_progress(db, execution_id, &step_results, 0, total_cost).await;

    // ── LLM call 1: plan all steps at runtime ────────────────────────────────
    // Generates prompt templates (with <placeholders>), to_extract goals, and
    // the output_generation guideline from the task descriptions.
    // Planning happens on every execution (Python MAF parity).
    let (step_plans, output_generation, plan_tokens) = plan_execution(&maf_def.steps, llm).await?;
    let mut total_tokens = plan_tokens;

    // Fill in the prompt template / extraction goal now that planning is
    // done — steps stay "pending" until their turn in the loop below. Every
    // step (not just completed ones) gets this filled in up front, which is
    // exactly what lets `run_maf_from` reconstruct the plan on resume without
    // a second planning call.
    for (result, plan) in step_results.iter_mut().zip(step_plans.iter()) {
        result.prompt_template = plan.prompt.clone();
        result.to_extract = plan.to_extract.clone();
    }
    persist_progress(db, execution_id, &step_results, total_tokens, total_cost).await;
    // Durable so a resumed run's final synthesis follows the same guidance the original run
    // planned — otherwise only held in this function's local `output_generation`, lost on resume.
    persist_output_generation(db, execution_id, &output_generation).await;

    for (i, (step, plan)) in maf_def.steps.iter().zip(step_plans.iter()).enumerate() {
        let context = build_context(&step_results[..i]);
        match execute_step(
            client,
            db,
            observability,
            execution_id,
            user_id,
            step,
            plan,
            &mut step_results,
            i,
            &context,
            &mut total_tokens,
            &mut total_cost,
            llm,
        )
        .await
        {
            StepLoopOutcome::Advance => {}
            StepLoopOutcome::AwaitingHuman(paused) => {
                return Ok(StepOutcome::AwaitingHuman(paused));
            }
            StepLoopOutcome::Failed(e) => return Err(e),
        }
    }

    finish_run(
        step_results,
        total_tokens,
        total_cost,
        &output_generation,
        llm,
    )
    .await
}

/// Resumes a MAF execution that paused mid-step, invoked by the HITL resume dispatcher
/// re-`XADD`ing a continuation job (`oss/server/src/hitl/mod.rs::deliver_maf`).
/// `step_results`/`tokens_used_so_far`/`output_generation` come from `maf_executions` (the durable
/// state every prior step already persisted, plus the two columns HITL support added); `maf_def`
/// is the SAME snapshot the original run
/// used (carried in the continuation job's `maf_json`, never re-fetched from the mutable `mafs`
/// table — §2.3 #6). Never re-plans: every step's `StepPlan` is reconstructed from the
/// `prompt_template`/`to_extract` `run_maf` already persisted for every step up front.
#[allow(clippy::too_many_arguments)]
pub async fn run_maf_from(
    client: &reqwest::Client,
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
    mut step_results: Vec<StepResult>,
    tokens_used_so_far: i64,
    cost_used_so_far: f64,
    output_generation: String,
    resume_index: usize,
    resume_task_id: String,
    resume_context_id: String,
    injected_answer: String,
) -> Result<StepOutcome, String> {
    // `resume_index` comes from `hitl_requests.maf_step_index`, off the same DB row
    // `worker.rs::create_hitl_request` already bounds-checks against `maf_def.steps` before
    // ever persisting it — but that check happens once, on a different (and possibly
    // since-corrupted) in-memory snapshot. Re-checked here against the actual `maf_def`/
    // `step_results` this call received: an out-of-range index would otherwise panic on the
    // slice below (or the `maf_def.steps[resume_index]` index just past it), taking down this
    // whole worker task with no `catch_unwind` around it.
    if resume_index >= maf_def.steps.len() || resume_index >= step_results.len() {
        return Err(format!(
            "resume step index {resume_index} out of range ({} steps, {} step_results)",
            maf_def.steps.len(),
            step_results.len()
        ));
    }
    // `plans` below is built 1:1 from `step_results`, but the loop after it walks `i` up to
    // `maf_def.steps.len()`, not `plans.len()` — the check above only bounds `resume_index`
    // itself, not the two collections against each other. `step_results`/`tokens_used_so_far`/
    // `output_generation` come from `maf_executions` while `maf_def` comes from the separate
    // `maf_json` snapshot column, and nothing cross-checks the two on the way in: a persisted
    // `step_results` shorter than the snapshot's own step list (found in review) would pass the
    // check above whenever `resume_index` still lands inside both, then panic on the direct
    // `plans[i]` index the first time the loop's `i` reaches `plans.len()` (== `step_results.len()`
    // here) — after `continue_paused_step` has already run and the human's just-delivered answer
    // has already been consumed, so the crash (caught by `process_job`'s `catch_unwind`, but still
    // a hard failure) discards it instead of failing before any of that work starts.
    if step_results.len() != maf_def.steps.len() {
        return Err(format!(
            "step_results length ({}) does not match the maf_json snapshot's step count ({}) — \
             refusing to resume rather than run past the shorter one",
            step_results.len(),
            maf_def.steps.len()
        ));
    }

    let mut total_tokens = tokens_used_so_far;
    let mut total_cost = cost_used_so_far;
    let plans: Vec<StepPlan> = step_results
        .iter()
        .map(|r| StepPlan {
            prompt: r.prompt_template.clone(),
            to_extract: r.to_extract.clone(),
        })
        .collect();

    let context = build_context(&step_results[..resume_index]);
    match continue_paused_step(
        client,
        db,
        observability,
        execution_id,
        user_id,
        &maf_def.steps[resume_index],
        &plans[resume_index],
        &mut step_results,
        resume_index,
        &context,
        &mut total_tokens,
        &mut total_cost,
        llm,
        &resume_task_id,
        &resume_context_id,
        &injected_answer,
    )
    .await
    {
        StepLoopOutcome::Advance => {}
        StepLoopOutcome::AwaitingHuman(paused) => return Ok(StepOutcome::AwaitingHuman(paused)),
        StepLoopOutcome::Failed(e) => return Err(e),
    }

    for i in (resume_index + 1)..maf_def.steps.len() {
        let context = build_context(&step_results[..i]);
        match execute_step(
            client,
            db,
            observability,
            execution_id,
            user_id,
            &maf_def.steps[i],
            &plans[i],
            &mut step_results,
            i,
            &context,
            &mut total_tokens,
            &mut total_cost,
            llm,
        )
        .await
        {
            StepLoopOutcome::Advance => {}
            StepLoopOutcome::AwaitingHuman(paused) => {
                return Ok(StepOutcome::AwaitingHuman(paused));
            }
            StepLoopOutcome::Failed(e) => return Err(e),
        }
    }

    finish_run(
        step_results,
        total_tokens,
        total_cost,
        &output_generation,
        llm,
    )
    .await
}

/// Runs one step fresh: prompt generation (LLM call 2) → agent call → extraction (LLM call 3),
/// persisting progress at every transition exactly as the original inline loop body did. Shared by
/// `run_maf` and, for every step after the resumed one, `run_maf_from`.
#[allow(clippy::too_many_arguments)]
async fn execute_step(
    client: &reqwest::Client,
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    user_id: Uuid,
    step: &MafStep,
    plan: &StepPlan,
    step_results: &mut [StepResult],
    i: usize,
    context: &str,
    total_tokens: &mut i64,
    total_cost: &mut f64,
    llm: &LlmClient,
) -> StepLoopOutcome {
    step_results[i].status = "running".to_string();
    persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;

    // ── LLM call 2: fill <placeholders> with context from previous steps ─
    let (actual_prompt, prompt_tokens) =
        match generate_step_prompt(&plan.prompt, &step.task_description, context, llm).await {
            Ok(v) => v,
            Err(e) => {
                let err = format!("step {}: prompt generation failed: {e}", step.step_index);
                step_results[i].status = "failed".to_string();
                step_results[i].error = Some(err.clone());
                persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
                return StepLoopOutcome::Failed(err);
            }
        };

    // ── Agent call ────────────────────────────────────────────────────────
    let start = Instant::now();
    let (traceparent, trace_id) = build_traceparent(execution_id, step.step_index);
    register_flow(
        db,
        execution_id,
        user_id,
        &trace_id,
        &step.agent_name,
        &step.task_description,
        step.agent_id,
    )
    .await;

    let call_result = call_agent(
        client,
        &step.agent_endpoint,
        &execution_id.to_string(),
        &user_id.to_string(),
        &actual_prompt,
        &traceparent,
    )
    .await;

    let raw_response = match call_result {
        Ok(AgentCallOutcome::Completed(text)) => text,
        Ok(AgentCallOutcome::Paused { task_id, raw_data }) => {
            step_results[i].status = nasiko_types::maf::AWAITING_HUMAN.to_string();
            step_results[i].prompt = actual_prompt;
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::AwaitingHuman(build_paused_step(
                step.step_index,
                task_id,
                execution_id.to_string(),
                &raw_data,
            ));
        }
        Err(e) => {
            let err = format!(
                "step {} (agent '{}') failed: {e}",
                step.step_index, step.agent_name
            );
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err);
        }
    };
    let latency_ms = start.elapsed().as_millis() as i64;

    finish_step(
        db,
        observability,
        execution_id,
        step,
        plan,
        step_results,
        i,
        context,
        total_tokens,
        total_cost,
        llm,
        actual_prompt,
        raw_response,
        latency_ms,
        &trace_id,
        prompt_tokens,
    )
    .await
}

/// Resumes exactly the paused step: sends the human's answer on the SAME `taskId`/`contextId`
/// (via `build_send_request_for_task` — the same builder the direct-chat HITL resume dispatcher
/// uses, `oss/server/src/hitl/mod.rs`) instead of generating a fresh prompt and starting a new
/// task, then runs the same extraction (LLM call 3) `execute_step` does. `actual_prompt` (the
/// prompt sent before the pause) is read back from `step_results[i]`, where `execute_step` already
/// persisted it on pausing.
#[allow(clippy::too_many_arguments)]
async fn continue_paused_step(
    client: &reqwest::Client,
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    user_id: Uuid,
    step: &MafStep,
    plan: &StepPlan,
    step_results: &mut [StepResult],
    i: usize,
    context: &str,
    total_tokens: &mut i64,
    total_cost: &mut f64,
    llm: &LlmClient,
    task_id: &str,
    context_id: &str,
    injected_answer: &str,
) -> StepLoopOutcome {
    let start = Instant::now();
    let (traceparent, trace_id) = build_traceparent(execution_id, step.step_index);

    // `register_flow` (execute_step's first-attempt insert) stamped `created_at` once, at the
    // original attempt — `ON CONFLICT DO NOTHING`, never touched again. The MCP gateway's
    // liveness check (`gateway.rs::flow_user`) requires `created_at` to be within
    // `NASIKO_FLOW_TIMEOUT_SECS` (600s default) of `now()`, so a human who takes longer than that to
    // approve a paused tool call permanently strands this trace id: every retried `tools/call`
    // 403s as "not a live flow", the agent re-asks, and re-approving can never fix it, since
    // nothing ever refreshes this row. Reopen it here, same as every other HITL resume dispatch
    // site (`a2a_dispatch.rs`'s `dispatch_to_agent`, `hitl/mod.rs`'s `deliver`) — except also
    // resetting `created_at`, which those two don't (they aren't the traceparent this specific
    // timeout-vs-human-latency bug was diagnosed against, but would have the same exposure).
    let _ = sqlx::query(
        "UPDATE flows SET status = 'running', completed_at = NULL, created_at = now() WHERE flow_id = $1",
    )
    .bind(&trace_id)
    .execute(db)
    .await;

    let call_result = call_agent_continuation(
        client,
        &step.agent_endpoint,
        context_id,
        task_id,
        &user_id.to_string(),
        injected_answer,
        &traceparent,
    )
    .await;

    let raw_response = match call_result {
        Ok(AgentCallOutcome::Completed(text)) => text,
        Ok(AgentCallOutcome::Paused { task_id, raw_data }) => {
            // Sequential HITL on the same step: still
            // awaiting a human, on a fresh question — the step's status was already
            // "awaiting_human" and stays that way.
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::AwaitingHuman(build_paused_step(
                step.step_index,
                task_id,
                context_id.to_string(),
                &raw_data,
            ));
        }
        Err(e) => {
            let err = format!(
                "step {} (agent '{}') resume failed: {e}",
                step.step_index, step.agent_name
            );
            step_results[i].status = "failed".to_string();
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err);
        }
    };
    let latency_ms = start.elapsed().as_millis() as i64;
    let actual_prompt = step_results[i].prompt.clone();

    finish_step(
        db,
        observability,
        execution_id,
        step,
        plan,
        step_results,
        i,
        context,
        total_tokens,
        total_cost,
        llm,
        actual_prompt,
        raw_response,
        latency_ms,
        &trace_id,
        // No LLM call 2 on resume — the prompt was already generated (and its tokens already
        // unaccounted-for, same as the pre-existing behavior for a step that errors before
        // reaching this point) before the pause.
        0,
    )
    .await
}

/// Shared tail of both `execute_step` and `continue_paused_step` once a real (non-paused) agent
/// reply is in hand: LLM call 3 (extraction), token/cost accounting, and the `"success"` persist.
#[allow(clippy::too_many_arguments)]
async fn finish_step(
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    execution_id: Uuid,
    step: &MafStep,
    plan: &StepPlan,
    step_results: &mut [StepResult],
    i: usize,
    context: &str,
    total_tokens: &mut i64,
    total_cost: &mut f64,
    llm: &LlmClient,
    actual_prompt: String,
    raw_response: String,
    latency_ms: i64,
    trace_id: &str,
    prompt_tokens: i64,
) -> StepLoopOutcome {
    // ── LLM call 3: extract relevant info from agent response ─────────────
    let (extracted, extract_tokens) = match extract_info(
        &plan.prompt,
        &actual_prompt,
        &raw_response,
        &plan.to_extract,
        context,
        llm,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            let err = format!("step {}: extraction failed: {e}", step.step_index);
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].latency_ms = latency_ms;
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err);
        }
    };

    // Wait for the agent's own token usage to land in Tempo (it batches
    // span export, so it's usually not there the instant the call
    // returns) so the persisted step total already reflects LLM + agent
    // cost together, not just MAF's own reasoning cost.
    let agent_usage = wait_for_agent_usage(observability, trace_id).await;
    let llm_tokens = prompt_tokens + extract_tokens;
    let step_tokens = llm_tokens + agent_usage.input as i64 + agent_usage.output as i64;
    *total_tokens += step_tokens;

    // Cost is agent-call spend only (not MAF's own planning/reasoning LLM calls) — keeps
    // Agent-view and Workflow-view FinOps rows apples-to-apples, since agent-view cost is also
    // agent-spend-only.
    let step_cost = observability
        .cost(nasiko_observability::CostRequest {
            // The agent's own spans carry the provider; this aggregate does not,
            // so the model name alone resolves the rate here.
            provider: None,
            model: agent_usage.model.as_deref(),
            at: chrono::Utc::now(),
            input_tokens: agent_usage.input,
            output_tokens: agent_usage.output,
            cache_read_tokens: agent_usage.cache_read,
            cache_creation_tokens: agent_usage.cache_creation,
            context: Default::default(),
        })
        .await
        .total_usd;
    *total_cost += step_cost;

    let new_context = if context.is_empty() {
        format!(
            "Step {} ({}): {}",
            step.step_index, step.agent_name, extracted
        )
    } else {
        format!(
            "{}\nStep {} ({}): {}",
            context, step.step_index, step.agent_name, extracted
        )
    };

    step_results[i].status = "success".to_string();
    step_results[i].prompt = actual_prompt;
    step_results[i].extracted_info = Some(extracted);
    step_results[i].tokens_used = step_tokens;
    step_results[i].input_tokens = agent_usage.input as i64;
    step_results[i].output_tokens = agent_usage.output as i64;
    step_results[i].cache_read_tokens = agent_usage.cache_read as i64;
    step_results[i].cache_creation_tokens = agent_usage.cache_creation as i64;
    step_results[i].model_used = agent_usage.model;
    step_results[i].cost_usd = step_cost;
    step_results[i].latency_ms = latency_ms;
    step_results[i].context = Some(new_context);
    persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;

    StepLoopOutcome::Advance
}

/// Builds the `PausedStep` a `hitl_requests` row is created from, out of a `Paused`-classified raw
/// A2A payload — shared parsing (`oss/types::a2a`) between `execute_step` and
/// `continue_paused_step`.
fn build_paused_step(
    step_index: i32,
    task_id: String,
    context_id: String,
    raw_data: &str,
) -> PausedStep {
    let kind = match nasiko_types::a2a::pause_reason(raw_data) {
        PauseReason::InputRequired => nasiko_hitl::HitlKind::InputRequired,
        PauseReason::AuthRequired => nasiko_hitl::HitlKind::AuthRequired,
    };
    PausedStep {
        step_index,
        task_id,
        context_id,
        kind,
        question: nasiko_types::a2a::build_pause_question(raw_data),
    }
}

/// LLM call 4: synthesise the final output once every step has succeeded — shared tail of
/// `run_maf`/`run_maf_from`.
async fn finish_run(
    step_results: Vec<StepResult>,
    mut total_tokens: i64,
    total_cost: f64,
    output_generation: &str,
    llm: &LlmClient,
) -> Result<StepOutcome, String> {
    let (output, output_tokens) = generate_final_output(&step_results, output_generation, llm)
        .await
        .map_err(|e| format!("final output generation failed: {e}"))?;
    total_tokens += output_tokens;

    Ok(StepOutcome::Completed(ExecutionResult {
        output,
        step_results,
        tokens_used: total_tokens,
        cost_usd: total_cost,
    }))
}

/// Registers this step as a flow so the LLM gateway sees it as IN-FLOW (not inert) and its tier
/// classifier can fire. The invariant the gateway relies on (see `derive_boundary_signals`): the
/// trace_id forwarded in `traceparent` IS the `flow_id` in this row — mirroring the orchestrator /
/// agent-proxy ingress. Best-effort — a failed insert only means this step falls back to the
/// default model.
async fn register_flow(
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    trace_id: &str,
    agent_name: &str,
    task_description: &str,
    agent_id: Uuid,
) {
    let flow_metadata = serde_json::json!({
        "context_id": execution_id.to_string(),
        "mode": "free_flowing",
    });
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, $3, $4, 'running', $5)
           ON CONFLICT (flow_id) DO NOTHING"#,
    )
    .bind(trace_id)
    .bind(user_id)
    .bind(agent_name)
    .bind(task_description)
    .bind(&flow_metadata)
    .execute(db)
    .await;
    // Participant record — the MCP gateway / LLM router only authorize this step's agent for
    // calls carrying this trace id if it is recorded here (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4).
    // Same synchronous pre-call write as the flows row; a failed insert denies (never escalates)
    // downstream.
    if let Err(e) = sqlx::query(
        "INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)
         ON CONFLICT (flow_id, agent_id) DO NOTHING",
    )
    .bind(trace_id)
    .bind(agent_id)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, flow_id = %trace_id, %agent_id,
            "flow participant record failed — the step agent's MCP/LLM calls will be denied"
        );
    }
}

/// Persists the planner's synthesis guideline (LLM call 1's output) so a resumed run's final
/// output generation (`finish_run`) can reuse it — see the migration comment in
/// `oss/migrations/0024_maf_hitl.sql` for why this can't just be re-derived on resume.
async fn persist_output_generation(db: &PgPool, execution_id: Uuid, output_generation: &str) {
    let _ = sqlx::query("UPDATE maf_executions SET output_generation = $1 WHERE id = $2")
        .bind(output_generation)
        .bind(execution_id)
        .execute(db)
        .await;
}

/// Builds a placeholder "pending" entry for a step before planning/execution
/// has produced any of its actual content.
fn pending_result(step: &MafStep) -> StepResult {
    StepResult {
        step_id: step.step_id,
        step_index: step.step_index,
        agent_id: step.agent_id,
        agent_name: step.agent_name.clone(),
        status: "pending".to_string(),
        error: None,
        prompt_template: String::new(),
        to_extract: String::new(),
        prompt: String::new(),
        extracted_info: None,
        tokens_used: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        model_used: None,
        cost_usd: 0.0,
        latency_ms: 0,
        context: None,
        obs_logs: serde_json::Value::Null,
    }
}

/// Writes the current step progress snapshot to `maf_executions.step_results`.
/// Best-effort: a transient write failure here shouldn't abort the run — the
/// next transition will just overwrite with fresher data, and the final
/// write in worker.rs remains the source of truth once the run completes.
async fn persist_progress(
    db: &PgPool,
    execution_id: Uuid,
    step_results: &[StepResult],
    tokens_used: i64,
    cost_usd: f64,
) {
    let step_json = serde_json::to_value(step_results).unwrap_or_default();
    let _ = sqlx::query(
        "UPDATE maf_executions SET step_results = $1::jsonb, tokens_used = $2, cost_usd = $3 WHERE id = $4",
    )
    .bind(step_json.to_string())
    .bind(tokens_used)
    .bind(cost_usd)
    .bind(execution_id)
    .execute(db)
    .await;
}

// ─── Step plan produced by plan_execution ────────────────────────────────────

struct StepPlan {
    prompt: String,
    to_extract: String,
}

// ─── LLM call 1: runtime planner ─────────────────────────────────────────────

async fn plan_execution(
    steps: &[MafStep],
    llm: &LlmClient,
) -> Result<(Vec<StepPlan>, String, i64), String> {
    let system = "You are a MAF (Multi-Agent Flow) step planner.\n\
                  Given a list of steps (each with a task description and the agent that will \
                  handle it), generate:\n\
                  1. For each step — a prompt template and a to_extract goal.\n\
                     - IMPORTANT: For the FIRST step (step 0), NEVER use placeholders. Use the exact \
                     values (amounts, currencies, names, etc.) from the task description verbatim.\n\
                     - For subsequent steps, use <variable_name> syntax (e.g. <jpy_amount>) ONLY to \
                     reference data that was extracted from a previous step — never for values that are \
                     already stated in the task description.\n\
                     - Include the placeholder name in to_extract only when a later step needs the value.\n\
                  2. An output_generation string describing how to present the final answer to the user.\n\n\
                  Return ONLY valid JSON:\n\
                  {\n\
                    \"output_generation\": \"...\",\n\
                    \"steps\": [{\"prompt\": \"...\", \"to_extract\": \"...\"}, ...]\n\
                  }\n\
                  The steps array must have exactly one entry per input step, in the same order.";

    let one_shot_human = "Steps:\n\
                          Step 0 (Fantasy Book Recommender): Recommend at least three fantasy books\n\
                          Step 1 (Online Book Shopping Agent): Find the best deals for the recommended books";

    let one_shot_assistant = r#"{"output_generation": "Present the top three fantasy book recommendations along with the best online deal for each, including store name and price.", "steps": [{"prompt": "Recommend at least three fantasy books.", "to_extract": "The top three fantasy book recommendations including title and author (<top_three_recommendations>)"}, {"prompt": "Here are the three books I want to buy: <top_three_recommendations>. Find out the best deals for these books online.", "to_extract": "Best online deals for each book including store name, price, and a direct purchase link if available"}]}"#;

    let step_list = steps
        .iter()
        .map(|s| {
            format!(
                "Step {} ({}): {}",
                s.step_index, s.agent_name, s.task_description
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user = format!("Steps:\n{step_list}");

    // Schema matches Python's MAFTemplate Pydantic model — strict enforcement via
    // OpenAI structured outputs, equivalent to `with_structured_output(MAFTemplate)`.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "output_generation": {"type": "string"},
            "steps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string"},
                        "to_extract": {"type": "string"}
                    },
                    "required": ["prompt", "to_extract"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["output_generation", "steps"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "execution_plan",
            schema,
        )
        .await?;

    let output_generation = json["output_generation"]
        .as_str()
        .unwrap_or("Summarise all extracted information into a clear, well-structured response.")
        .to_string();

    let plans_json = json["steps"]
        .as_array()
        .ok_or_else(|| "planner returned no 'steps' array".to_string())?;

    if plans_json.len() != steps.len() {
        return Err(format!(
            "planner returned {} step plans but MAF has {} steps",
            plans_json.len(),
            steps.len()
        ));
    }

    let plans = plans_json
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let prompt = p["prompt"]
                .as_str()
                .ok_or_else(|| format!("step {i}: planner returned no 'prompt'"))?
                .to_string();
            let to_extract = p["to_extract"]
                .as_str()
                .ok_or_else(|| format!("step {i}: planner returned no 'to_extract'"))?
                .to_string();
            Ok(StepPlan { prompt, to_extract })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok((plans, output_generation, tokens))
}

// ─── LLM call 2: prompt generator ────────────────────────────────────────────

async fn generate_step_prompt(
    template: &str,
    task_description: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // No placeholders — send the template verbatim, no LLM call needed.
    if !template.contains('<') {
        return Ok((template.to_string(), 0));
    }

    // When there are no prior step results, use the task description so the LLM can
    // fill placeholders from values stated explicitly in the task (e.g. "10 rupees").
    let effective_context = if context.is_empty() {
        format!("Task description for this step: {task_description}")
    } else {
        context.to_string()
    };

    // System prompt matches Python's MAFExecutor._create_user_prompt exactly.
    let system = "You are a Multi-Agent Flow (MAF) prompt generator.\n\
                  Your task is to generate a specific, actionable user prompt for an agent \
                  in a linear workflow.\n\n\
                  You will be provided with:\n\
                  1. The **prompt template** for the current step.\n\
                  2. **Context** from previous steps in the flow, including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts and the actual prompts generated.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Generate a user prompt for the current step by combining the current prompt \
                  template with the available context.\n\
                  - Replace any placeholders (like <variable_name>) in the prompt template with \
                  actual data from the context.\n\
                  - Ensure the resulting prompt is clear and directly tells the agent what to do, \
                  leveraging the history of the flow.\n\n\
                  Output the result in the specified structured format.";

    // One-shot uses the verbose context format that build_context produces.
    let one_shot_human = "Current Step Prompt Template: Here are the three books I want to buy: \
                          <top_three_recommendations>. Find out the best deals for these books online.\n\n\
                          Context from Previous Steps:\n\
                          --- Step 1 (Fantasy Book Recommender) ---\n\
                          Prompt Template: Recommend at least three fantasy books.\n\
                          User Prompt Sent: Recommend at least three fantasy books.\n\
                          Goal of Extraction: The top three fantasy book recommendations including \
                          title and author (<top_three_recommendations>).\n\
                          Actual Extracted Information: 1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch";

    let one_shot_assistant = r#"{"prompt": "Here are the three books I want to buy: 1. 'The Way of Kings' by Brandon Sanderson, 2. 'The Name of the Wind' by Patrick Rothfuss, 3. 'The Lies of Locke Lamora' by Scott Lynch. Find out the best deals for these books online."}"#;

    let user = if effective_context.is_empty() {
        format!("Current Step Prompt Template: {template}")
    } else {
        format!(
            "Current Step Prompt Template: {template}\n\nContext from Previous Steps:\n{effective_context}"
        )
    };

    // Schema matches Python's GeneratedPrompt Pydantic model.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "prompt": {"type": "string"}
        },
        "required": ["prompt"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "generated_prompt",
            schema,
        )
        .await?;

    let prompt = json["prompt"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM prompt generation returned no 'prompt' field".to_string())?;

    Ok((prompt, tokens))
}

// ─── LLM call 3: extractor ────────────────────────────────────────────────────

async fn extract_info(
    template: &str,
    actual_prompt: &str,
    response: &str,
    goal: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // System prompt matches Python's MAFExecutor._extract_info exactly.
    let system = "You are a Multi-Agent Flow (MAF) information extractor.\n\
                  Your task is to extract specific information from an agent's response based \
                  on the \"goal of extraction\" for the current step.\n\n\
                  You will be provided with:\n\
                  1. The **prompt template** for the current step.\n\
                  2. The **actual prompt** sent to the agent in the current step.\n\
                  3. The **goal of extraction** for the current step.\n\
                  4. The **agent's response** for the current step.\n\
                  5. **Context** from previous steps in the flow (if any), including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts.\n\
                     - The actual prompts sent to the agents.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Extract all information from the agent response that is required by the \
                  goal of extraction.\n\
                  - The goal of extraction may contain placeholders (like <variable_name>), but \
                  it might also mention other specific details to capture. Ensure EVERYTHING \
                  mentioned in the goal is extracted correctly.\n\
                  - The extracted information should be formatted as a clear, standalone piece of \
                  data that can be used as a direct replacement for its context in the workflow.\n\
                  - Use the provided context from previous steps if necessary to understand the \
                  full scope of what needs to be extracted (e.g., if the goal refers to something \
                  previously mentioned).\n\
                  - Ignore any conversational filler or irrelevant information in the agent's \
                  response.\n\n\
                  Output the result in the specified structured format.";

    // One-shot agent response matches Python's _extract_info example exactly.
    let one_shot_human = "Current Step Prompt Template: Recommend at least three fantasy books.\n\
                          Current Step User Prompt Sent: Recommend at least three fantasy books.\n\
                          Current Step Goal of Extraction: The top three fantasy book recommendations \
                          including title and author (<top_three_recommendations>).\n\
                          Current Step Agent Response: Hello! I'd be happy to help. Based on your \
                          interest in fantasy, here are some great reads. First, there's 'The Way of \
                          Kings' by Brandon Sanderson, which is the start of a massive epic. Then, \
                          'The Name of the Wind' by Patrick Rothfuss is a must-read for its beautiful \
                          prose. Finally, I highly recommend 'The Lies of Locke Lamora' by Scott Lynch \
                          for some high-stakes thievery. I've also heard 'Mistborn' is good, but these \
                          three are my top picks for you. Hope this helps!\n\n\
                          Context from Previous Steps:\n\
                          None";

    let one_shot_assistant = r#"{"extracted_info": "1. 'The Way of Kings' by Brandon Sanderson, 2. 'The Name of the Wind' by Patrick Rothfuss, 3. 'The Lies of Locke Lamora' by Scott Lynch"}"#;

    // Match Python: conditionally include context section; write "None" when empty.
    let context_section = if context.is_empty() {
        "Context from Previous Steps:\nNone".to_string()
    } else {
        format!("Context from Previous Steps:\n{context}")
    };

    let user = format!(
        "Current Step Prompt Template: {template}\n\n\
         Current Step User Prompt Sent: {actual_prompt}\n\n\
         Current Step Goal of Extraction: {goal}\n\n\
         Current Step Agent Response: {response}\n\n\
         {context_section}"
    );

    // Schema matches Python's ExtractedInfo Pydantic model — strict enforcement via
    // OpenAI structured outputs, equivalent to `with_structured_output(ExtractedInfo)`.
    // The API guarantees extracted_info is always present and always a string.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "extracted_info": {
                "type": "string",
                "description": "The information extracted from an agent's response."
            }
        },
        "required": ["extracted_info"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "extraction_result",
            schema,
        )
        .await?;

    let extracted = json["extracted_info"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM extraction returned no 'extracted_info' field".to_string())?;

    Ok((extracted, tokens))
}

// ─── LLM call 4: final output synthesiser ────────────────────────────────────

async fn generate_final_output(
    step_results: &[StepResult],
    guidelines: &str,
    llm: &LlmClient,
) -> Result<(String, i64), String> {
    // System prompt matches Python's MAFExecutor._create_final_output exactly.
    let system = "You are a Multi-Agent Flow (MAF) final output generator.\n\
                  Your task is to consolidate all information extracted from various agents \
                  into a single, cohesive, and helpful response for the user.\n\n\
                  You will be provided with:\n\
                  1. **Guidelines** on how to construct the final output.\n\
                  2. **Context** from previous steps in the flow, including:\n\
                     - The agents used.\n\
                     - The prompt templates used to generate prompts.\n\
                     - The actual prompts sent to the agents.\n\
                     - What information was intended to be extracted from the agent response \
                  (Goal of Extraction).\n\
                     - The actual information that was extracted from the agent response.\n\n\
                  Your goal is to:\n\
                  - Follow the provided guidelines to construct the final answer.\n\
                  - Ensure the response is well-structured, clear, and directly addresses \
                  the user's initial intent.\n\
                  - Leverage all the extracted data to provide a comprehensive result.\n\n\
                  Output the result in the specified structured format.";

    // One-shot context uses the same verbose format that build_context produces.
    let one_shot_human = "Guidelines for Output Generation: Present the top three fantasy book \
                          recommendations along with the best online deals for each, including \
                          store name, price, and a direct purchase link if available.\n\n\
                          Context from All Steps:\n\
                          --- Step 1 (Fantasy Book Recommender) ---\n\
                          Prompt Template: Recommend at least three fantasy books.\n\
                          User Prompt Sent: Recommend at least three fantasy books.\n\
                          Goal of Extraction: The top three fantasy book recommendations including \
                          title and author (<top_three_recommendations>).\n\
                          Actual Extracted Information: 1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch\n\n\
                          --- Step 2 (Online Book Shopping Agent) ---\n\
                          Prompt Template: Here are the three books I want to buy: \
                          <top_three_recommendations>. Find out the best deals for these books online.\n\
                          User Prompt Sent: Here are the three books I want to buy: \
                          1. 'The Way of Kings' by Brandon Sanderson, \
                          2. 'The Name of the Wind' by Patrick Rothfuss, \
                          3. 'The Lies of Locke Lamora' by Scott Lynch. \
                          Find out the best deals for these books online.\n\
                          Goal of Extraction: Best online deals for each book including store name, \
                          price, and a direct purchase link if available.\n\
                          Actual Extracted Information: \
                          1. 'The Way of Kings': Amazon $18.99 — best deal. \
                          2. 'The Name of the Wind': Powell's $15.99 — best deal. \
                          3. 'The Lies of Locke Lamora': Target $14.99 — best deal.";

    let one_shot_assistant = r#"{"final_output": "Here are the top three fantasy book recommendations with their best online deals:\n\n1. **The Way of Kings** by Brandon Sanderson\n   Best deal: Amazon — $18.99\n\n2. **The Name of the Wind** by Patrick Rothfuss\n   Best deal: Powell's Books — $15.99\n\n3. **The Lies of Locke Lamora** by Scott Lynch\n   Best deal: Target — $14.99"}"#;

    let context = build_context(step_results);
    let user = format!(
        "Guidelines for Output Generation: {guidelines}\n\nContext from All Steps:\n{context}"
    );

    // Schema matches Python's FinalOutput Pydantic model.
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "final_output": {
                "type": "string",
                "description": "The final consolidated response for the user."
            }
        },
        "required": ["final_output"],
        "additionalProperties": false
    });

    let (json, tokens) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(system),
                ChatMessage::user(one_shot_human),
                ChatMessage::assistant(one_shot_assistant),
                ChatMessage::user(user),
            ],
            "final_output",
            schema,
        )
        .await?;

    let output = json["final_output"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM final output returned no 'final_output' field".to_string())?;

    Ok((output, tokens))
}

// ─── A2A agent call ───────────────────────────────────────────────────────────

/// Builds a valid W3C `traceparent` scoped to this one step — not the whole
/// execution — so each step's agent call lands under its own Tempo trace_id.
/// That's what lets per-step (not just per-execution) agent token usage be
/// looked up later without ambiguity when the same agent is used in more
/// than one step. Deterministic (execution_id + step_index) via UUIDv5, so no
/// new dependency and no random-id bookkeeping is needed to reconstruct it
/// later when reading the execution back.
///
/// Returns `(traceparent_header, trace_id)`. The bare `trace_id` (32 hex, no
/// dashes) is the value both the flow-registration insert and the Tempo token
/// lookup key on, so it's returned rather than re-derived at each call site.
fn build_traceparent(execution_id: Uuid, step_index: i32) -> (String, String) {
    let trace_uuid = Uuid::new_v5(&execution_id, step_index.to_string().as_bytes());
    let span_uuid = Uuid::new_v5(&execution_id, format!("{step_index}-span").as_bytes());
    let trace_id = trace_uuid.simple().to_string();
    let span_id = &span_uuid.simple().to_string()[..16];
    // flags=01 (sampled) — otherwise a conforming exporter may decide not to
    // export the span at all, and this whole mechanism would silently no-op.
    let traceparent = format!("00-{trace_id}-{span_id}-01");
    (traceparent, trace_id)
}

/// Polls Tempo for this step's trace (keyed by the `trace_id` that
/// `build_traceparent` forwarded) and returns its total `gen_ai.usage` tokens,
/// or 0 if nothing shows up within the timeout — Tempo/the agent being
/// unreachable never fails the step, it just means this step's persisted total
/// is LLM-only.
struct AgentUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    model: Option<String>,
}

async fn wait_for_agent_usage(
    observability: &dyn ObservabilityProvider,
    trace_id: &str,
) -> AgentUsage {
    // Agents commonly batch-export spans every ~5s, so the trace usually
    // isn't queryable the instant the agent call returns — poll rather than
    // check once.
    for _ in 0..10 {
        if let Ok(trace) = observability.get_trace(trace_id).await {
            let (input, output, model) = trace.token_totals();
            if input + output > 0 {
                let (cache_read, cache_creation) = trace.cache_token_totals();
                return AgentUsage {
                    input,
                    output,
                    cache_read,
                    cache_creation,
                    model,
                };
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    AgentUsage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_creation: 0,
        model: None,
    }
}

async fn call_agent(
    client: &reqwest::Client,
    endpoint: &str,
    context_id: &str,
    user_id: &str,
    prompt: &str,
    traceparent: &str,
) -> Result<AgentCallOutcome, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        // Per the A2A spec (§5.3, §9.1): JSON-RPC method names are PascalCase,
        // matching gRPC conventions exactly — "SendMessage", not "message/send"
        // (that string is only the REST binding's URL path, a different
        // transport). Confirmed both against the spec's own example request
        // and empirically against a real deployed `oss/agents/translator`
        // build. Matches `oss/types::build_send_request`.
        "method": "SendMessage",
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": context_id,
                "role": "ROLE_USER",
                "parts": [{"text": prompt}]
            }
        }
    });

    let json = post_a2a_request(client, endpoint, user_id, traceparent, &body).await?;
    classify_agent_response(json, context_id)
}

/// Resumes a specific paused task with the human's answer, via
/// `nasiko_types::a2a::build_send_request_for_task` — the same request shape the direct-chat HITL
/// resume dispatcher sends (`oss/server/src/hitl/mod.rs`). Unlike `call_agent`, this always
/// targets an existing `taskId`, never starts a new one.
#[allow(clippy::too_many_arguments)]
async fn call_agent_continuation(
    client: &reqwest::Client,
    endpoint: &str,
    context_id: &str,
    task_id: &str,
    user_id: &str,
    answer: &str,
    traceparent: &str,
) -> Result<AgentCallOutcome, String> {
    let body = build_send_request_for_task(answer, context_id, task_id);
    let json = post_a2a_request(client, endpoint, user_id, traceparent, &body).await?;
    classify_agent_response(json, task_id)
}

/// Shared HTTP mechanics for both a fresh `call_agent` call and `call_agent_continuation`'s
/// resume: the `/jsonrpc`-then-`/` endpoint fallback and the top-level JSON-RPC `error` check —
/// everything both callers previously duplicated.
async fn post_a2a_request<T: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    endpoint: &str,
    user_id: &str,
    traceparent: &str,
    body: &T,
) -> Result<serde_json::Value, String> {
    // Some agents expose A2A at /jsonrpc, others at root /. Try /jsonrpc first
    // and fall back to / on 404 so both agent types work without DB changes.
    let base = endpoint.trim_end_matches('/');
    let url_jsonrpc = format!("{base}/jsonrpc");
    let url_root = format!("{base}/");

    // No per-request MCP credential: the agent authenticates to `/api/mcp`
    // with its own deploy-time MCP_GATEWAY_TOKEN, and the user binding rides
    // the forwarded traceparent + the flow_participants record written before
    // this call (docs/MCP_GATEWAY_AGENT_AUTH.md).
    // No per-request timeout: `client` is the MAF worker's own, built with the
    // platform's agent-call budget because agent A2A calls are all it makes.
    let resp = {
        let r = client
            .post(&url_jsonrpc)
            .header("X-User-Id", user_id)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("traceparent", traceparent)
            .json(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if r.status() == reqwest::StatusCode::NOT_FOUND {
            client
                .post(&url_root)
                .header("X-User-Id", user_id)
                .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
                .header("traceparent", traceparent)
                .json(body)
                .send()
                .await
                .map_err(|e| e.to_string())?
        } else {
            r
        }
    };

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = json["error"]["message"].as_str() {
        return Err(format!("A2A error: {err}"));
    }

    Ok(json)
}

/// Classifies an already-parsed A2A JSON-RPC response (top-level `error` already checked by
/// `post_a2a_request`) into a normal reply or a pause, via the same wire-shape parsing
/// `classify_stream_disposition` uses for the streaming direct-chat path.
/// `fallback_task_id` is used only if the payload carries no task id of its own (§`paused_task_id`).
fn classify_agent_response(
    json: serde_json::Value,
    fallback_task_id: &str,
) -> Result<AgentCallOutcome, String> {
    let data = json.to_string();
    match nasiko_types::a2a::classify_stream_disposition(&data) {
        StreamDisposition::Paused => {
            let task_id = nasiko_types::a2a::paused_task_id(&data, fallback_task_id);
            Ok(AgentCallOutcome::Paused {
                task_id,
                raw_data: data,
            })
        }
        StreamDisposition::Completed | StreamDisposition::Continue | StreamDisposition::Failed => {
            Ok(AgentCallOutcome::Completed(extract_text(&json)))
        }
    }
}

fn extract_text(json: &serde_json::Value) -> String {
    // Some agent SDKs nest the task under `result.task`; the ones actually
    // deployed here (confirmed live against `petra-assistant-demo`) return
    // `result` itself AS the task object — `result.artifacts`, no `task`
    // wrapper. Try both nestings for each shape so either SDK version works.
    for root in [&json["result"]["task"], &json["result"]] {
        if let Some(text) = root["artifacts"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a["parts"].as_array())
            .and_then(|p| p.first())
            .and_then(|p| p["text"].as_str())
        {
            return text.to_string();
        }
        if let Some(text) = root["status"]["message"]["parts"]
            .as_array()
            .and_then(|p| p.first())
            .and_then(|p| p["text"].as_str())
        {
            return text.to_string();
        }
    }
    if let Some(text) = json["result"]["parts"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["text"].as_str())
    {
        return text.to_string();
    }
    if let Some(text) = json["result"].as_str() {
        return text.to_string();
    }
    String::new()
}

// ─── Utilities ────────────────────────────────────────────────────────────────

/// Mirrors Python's `_get_context`: produces one verbose block per completed step,
/// joined by blank lines, using 1-based step numbering to match Python exactly.
///
/// Format per step:
/// ```text
/// --- Step N (AgentName) ---
/// Prompt Template: <template>
/// User Prompt Sent: <actual_prompt>
/// Goal of Extraction: <to_extract>
/// Actual Extracted Information: <extracted_info>
/// ```
fn build_context(step_results: &[StepResult]) -> String {
    step_results
        .iter()
        .filter_map(|s| {
            s.extracted_info.as_deref().map(|info| {
                format!(
                    "--- Step {} ({}) ---\n\
                     Prompt Template: {}\n\
                     User Prompt Sent: {}\n\
                     Goal of Extraction: {}\n\
                     Actual Extracted Information: {}",
                    s.step_index + 1,
                    s.agent_name,
                    s.prompt_template,
                    s.prompt,
                    s.to_extract,
                    info,
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use nasiko_observability::{
        AgentFinOps, AgentStats, CostBreakdown, ObservabilityError, Session, SessionDetails, Span,
        SpanDetails, TraceDetails,
    };
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A hand-rolled `ObservabilityProvider` test double. Only `get_trace` is
    /// exercised by `wait_for_agent_usage` — every other method is a
    /// never-called stub, required only because the trait has no default for
    /// them.
    struct MockProvider {
        /// `get_trace` returns `NotFound` for the first `fail_calls`
        /// invocations, then `trace` forever after.
        fail_calls: usize,
        calls: Arc<AtomicUsize>,
        trace: TraceDetails,
    }

    #[async_trait]
    impl ObservabilityProvider for MockProvider {
        async fn sessions_for_agent(
            &self,
            _: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<Vec<Session>, ObservabilityError> {
            Ok(vec![])
        }

        async fn get_session(
            &self,
            _: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<SessionDetails, ObservabilityError> {
            Err(ObservabilityError::NotFound("unused in these tests".into()))
        }

        async fn get_trace(&self, _trace_id: &str) -> Result<TraceDetails, ObservabilityError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_calls {
                Err(ObservabilityError::NotFound("not exported yet".into()))
            } else {
                Ok(self.trace.clone())
            }
        }

        async fn get_span(&self, _: &str, _: &str) -> Result<SpanDetails, ObservabilityError> {
            Err(ObservabilityError::NotFound("unused in these tests".into()))
        }

        async fn agent_stats(
            &self,
            agent_id: &str,
            start: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<AgentStats, ObservabilityError> {
            Ok(AgentStats {
                agent_id: agent_id.to_string(),
                trace_count: 0,
                is_capped: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                model_used: None,
                latency_ms_p50: None,
                latency_ms_p99: None,
                cost: CostBreakdown::default(),
                period_start: start,
            })
        }

        async fn agent_finops(
            &self,
            agent_id: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<AgentFinOps, ObservabilityError> {
            Ok(AgentFinOps {
                agent_id: agent_id.to_string(),
                operations: 0,
                is_capped: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                model_used: None,
                latency_ms_p50: None,
                cost: CostBreakdown::default(),
            })
        }

        async fn count_user_traces(
            &self,
            _: &str,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<usize, ObservabilityError> {
            Ok(0)
        }

        async fn query_logs(
            &self,
            _: &str,
            _: Option<DateTime<Utc>>,
            _: Option<DateTime<Utc>>,
            _: usize,
        ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError> {
            Ok(vec![])
        }

        async fn cost(&self, _: nasiko_observability::CostRequest<'_>) -> CostBreakdown {
            CostBreakdown::default()
        }
    }

    fn trace_with_tokens(input: u64, output: u64, model: Option<&str>) -> TraceDetails {
        let mut attrs: StdHashMap<String, serde_json::Value> = StdHashMap::new();
        attrs.insert(
            "gen_ai.usage.input_tokens".to_string(),
            serde_json::json!(input),
        );
        attrs.insert(
            "gen_ai.usage.output_tokens".to_string(),
            serde_json::json!(output),
        );
        if let Some(m) = model {
            attrs.insert("gen_ai.request.model".to_string(), serde_json::json!(m));
        }
        let span = Span {
            span_id: "span-1".into(),
            parent_span_id: None,
            name: "chat".into(),
            started_at: Utc::now(),
            ended_at: None,
            duration_ms: Some(100),
            service_name: "test-agent".into(),
            kind: 2,
            status_code: 1,
            status_message: String::new(),
            attributes: attrs,
            events: vec![],
        };
        TraceDetails {
            trace_id: "trace-1".into(),
            spans: vec![span],
            started_at: Some(Utc::now()),
            ended_at: None,
            duration_ms: Some(100),
        }
    }

    fn trace_with_no_tokens() -> TraceDetails {
        let span = Span {
            span_id: "span-1".into(),
            parent_span_id: None,
            name: "infra".into(),
            started_at: Utc::now(),
            ended_at: None,
            duration_ms: Some(5),
            service_name: "test-agent".into(),
            kind: 1,
            status_code: 1,
            status_message: String::new(),
            attributes: StdHashMap::new(),
            events: vec![],
        };
        TraceDetails {
            trace_id: "trace-1".into(),
            spans: vec![span],
            started_at: Some(Utc::now()),
            ended_at: None,
            duration_ms: Some(5),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_agent_usage_returns_immediately_when_the_trace_is_found_on_the_first_poll() {
        let provider = MockProvider {
            fail_calls: 0,
            calls: Arc::new(AtomicUsize::new(0)),
            trace: trace_with_tokens(120, 80, Some("gpt-4o-mini")),
        };
        let usage = wait_for_agent_usage(&provider, "trace-1").await;
        assert_eq!(usage.input, 120);
        assert_eq!(usage.output, 80);
        assert_eq!(usage.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            1,
            "must not poll again once found"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_agent_usage_polls_through_transient_not_found_then_succeeds() {
        let provider = MockProvider {
            fail_calls: 4,
            calls: Arc::new(AtomicUsize::new(0)),
            trace: trace_with_tokens(50, 25, Some("claude-3-5-sonnet")),
        };
        let usage = wait_for_agent_usage(&provider, "trace-1").await;
        assert_eq!(usage.input, 50);
        assert_eq!(usage.output, 25);
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            5,
            "4 failed attempts + the 1 that finally succeeded"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_agent_usage_gives_up_after_ten_attempts_and_returns_zeroed_usage() {
        let provider = MockProvider {
            fail_calls: 999, // never succeeds
            calls: Arc::new(AtomicUsize::new(0)),
            trace: trace_with_tokens(1, 1, None),
        };
        let usage = wait_for_agent_usage(&provider, "trace-1").await;
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert!(usage.model.is_none());
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            10,
            "must stop after exactly 10 poll attempts, not loop forever"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_agent_usage_keeps_polling_a_trace_that_exists_but_has_no_token_attributes() {
        // A real trace that exists (e.g. a non-LLM agent step) but never
        // reports tokens must NOT be mistaken for "found" — the loop's exit
        // condition is `input + output > 0`, not merely "the trace exists".
        let provider = MockProvider {
            fail_calls: 0,
            calls: Arc::new(AtomicUsize::new(0)),
            trace: trace_with_no_tokens(),
        };
        let usage = wait_for_agent_usage(&provider, "trace-1").await;
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            10,
            "a token-less trace must exhaust all 10 attempts, same as never-found"
        );
    }

    fn maf_step(step_index: i32) -> MafStep {
        MafStep {
            step_id: Uuid::new_v4(),
            step_index,
            agent_id: Uuid::new_v4(),
            agent_name: format!("agent-{step_index}"),
            agent_endpoint: "http://example.invalid".into(),
            task_description: "do the thing".into(),
        }
    }

    /// Security/robustness regression (found in review): `step_results` (from `maf_executions`)
    /// and `maf_def.steps` (from the separate `maf_json` snapshot column) are never cross-checked
    /// on the way in. The old guard only bounded `resume_index` itself against both collections,
    /// so a `step_results` shorter than `maf_def.steps` — entirely possible since the two come
    /// from different columns — would pass that check whenever `resume_index` still landed inside
    /// both, then panic on a direct index (`plans[i]`) once the post-resume loop's `i` walked past
    /// `step_results.len()`. This must return a clean `Err`, never panic, and must do so before
    /// touching `db`/`client`/`observability`/`llm` at all — none of those are wired to anything
    /// real below, so a panic or an actual I/O attempt both fail this test.
    #[tokio::test]
    async fn run_maf_from_rejects_a_step_results_length_mismatch_instead_of_panicking() {
        let steps: Vec<MafStep> = (0..3).map(maf_step).collect();
        let maf_def = MafDefinition {
            description: None,
            steps: steps.clone(),
            output_generation: None,
        };
        // Only 2 of the 3 steps' results were persisted — the exact shape a truncated/corrupted
        // `maf_executions.step_results` column would take relative to a 3-step `maf_json` snapshot.
        let step_results: Vec<StepResult> = steps[..2].iter().map(pending_result).collect();

        let client = reqwest::Client::new();
        let db = PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
            .expect("connect_lazy never actually connects");
        let observability = MockProvider {
            fail_calls: 0,
            calls: Arc::new(AtomicUsize::new(0)),
            trace: trace_with_no_tokens(),
        };
        let llm = LlmClient::new(reqwest::Client::new(), String::new(), None, String::new());

        let result = run_maf_from(
            &client,
            &db,
            &observability,
            Uuid::new_v4(),
            Uuid::new_v4(),
            &maf_def,
            &llm,
            step_results,
            0,
            0.0,
            String::new(),
            0, // resume_index: in range for both the 3 steps and the 2 step_results
            "task-1".into(),
            "ctx-1".into(),
            "the human's answer".into(),
        )
        .await;

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("a length mismatch must be rejected, not silently succeed"),
        };
        assert!(
            err.contains("does not match"),
            "error should name the actual mismatch: {err}"
        );
    }
}
