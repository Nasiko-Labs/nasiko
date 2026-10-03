use std::sync::Arc;
use std::time::Instant;

use nasiko_flow::{FlowContext, FlowGuard};
use nasiko_types::a2a::{PauseReason, StreamDisposition, build_send_request_for_task};
use sqlx::PgPool;
use tracing::Instrument as _;
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient, LlmUsage};
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

/// Times an awaited step and emits it as an `info` event — visible under the
/// default `RUST_LOG=info` with no special filter — giving a per-call timing
/// breakdown of a MAF run (planning, per-step LLM calls, the agent HTTP round
/// trip) without needing an OTel exporter configured at all.
///
/// This is deliberately kept alongside the spans added by `run_maf`: spans go
/// to Tempo and are the only way to see the trace tree, but they produce
/// nothing in a deployment with no collector, which is the common local case.
async fn timed<T>(
    label: &'static str,
    execution_id: Uuid,
    step_index: Option<i32>,
    fut: impl std::future::Future<Output = T>,
) -> T {
    let start = Instant::now();
    // `otel.name` renames the span for OTel export, which is how a single
    // helper can emit distinctly-named spans despite `info_span!` requiring a
    // constant name.
    let span = tracing::info_span!(
        "maf.phase",
        otel.name = label,
        execution_id = %execution_id,
        step_index = step_index,
    );
    let result = fut.instrument(span).await;
    tracing::info!(
        target: "nasiko_orchestrator::maf",
        execution_id = %execution_id,
        step_index = step_index,
        label,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "maf step timing"
    );
    result
}

/// Runs one of MAF's own LLM calls: times it, records the GenAI usage
/// attributes on its span, and reports the call to the platform.
///
/// MAF used to total these tokens up itself into a bare scalar on
/// `maf_executions.tokens_used`. That number was never priced and never
/// reached any platform surface, so MAF's own orchestration spend — four-plus
/// LLM calls per run, on the platform's key — was both unbilled and invisible.
/// Metering is the platform's job, so each call is now handed to **both** of
/// the platform's ingestion paths and MAF stops keeping its own books:
///
/// * **`token_usage`** — the billing table behind `/api/usage/*`. `cost_usd`
///   is deliberately left NULL: the `calculate_usage_cost_trigger` prices the
///   row from `model_pricing`, exactly as it does for the LLM router's rows.
///   This is the same direct-insert pattern `engine.rs` already uses to meter
///   the routing engine from this crate.
/// * **`gen_ai.usage.*` span attributes** — read by the trace-usage
///   materializer into `trace_usage`, which is what the TokenOps/FinOps
///   dashboard queries.
///
/// The two are separate tables read by separate screens, so recording both is
/// not double counting.
///
/// Metering must never break a run: a failed usage write is logged and
/// swallowed, matching `write_selector_token_usage` and the router's
/// `spawn_log`.
async fn metered_llm<T>(
    phase: &'static str,
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    step_index: Option<i32>,
    llm: &LlmClient,
    fut: impl std::future::Future<Output = Result<(T, LlmUsage), String>>,
) -> Result<(T, LlmUsage), String> {
    let start = Instant::now();
    // `otel.name` renames the span for OTel export, which is how one helper
    // emits distinctly-named spans despite `info_span!` needing a constant
    // name. The `gen_ai.*` fields start Empty and are recorded once the call
    // returns — the attribute names are the ones `extract_token_attrs` and
    // `extract_cache_token_attrs` look for, so the materializer picks them up.
    let span = tracing::info_span!(
        "maf.phase",
        otel.name = phase,
        otel.kind = "client",
        gen_ai.operation.name = "chat",
        gen_ai.system = llm.provider(),
        execution_id = %execution_id,
        step_index = step_index,
        gen_ai.request.model = tracing::field::Empty,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.usage.cached_tokens = tracing::field::Empty,
    );

    let result = fut.instrument(span.clone()).await;

    // An empty model means no request was actually made — `generate_step_prompt`
    // short-circuits a placeholder-free template without calling the LLM. Such
    // a phase has nothing to meter, and writing a zeroed row for it would add
    // a junk entry to the billing table for every literal step.
    if let Ok((_, usage)) = &result
        && !usage.model.is_empty()
    {
        span.record("gen_ai.request.model", usage.model.as_str());
        span.record("gen_ai.usage.input_tokens", usage.input_tokens);
        span.record("gen_ai.usage.output_tokens", usage.output_tokens);
        span.record("gen_ai.usage.cached_tokens", usage.cached_tokens);
        write_token_usage(db, execution_id, user_id, phase, step_index, llm, usage).await;
    }

    tracing::info!(
        target: "nasiko_orchestrator::maf",
        execution_id = %execution_id,
        step_index = step_index,
        label = phase,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "maf step timing"
    );
    result
}

/// Insert one `token_usage` row for a MAF orchestration LLM call.
///
/// `session_id` is the execution id, which is also the A2A `contextId` MAF
/// sends its step agents — so MAF's own spend and its agents' spend aggregate
/// under one session, the same convention the LLM router follows by writing
/// the flow id there.
///
/// `cost_usd` is not bound: leaving it NULL is what lets the DB trigger price
/// the row. Writing a zero here would defeat it.
async fn write_token_usage(
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    phase: &'static str,
    step_index: Option<i32>,
    llm: &LlmClient,
    usage: &LlmUsage,
) {
    let metadata = serde_json::json!({
        "key_source": "platform",
        "component": "maf",
        "phase": phase,
        "execution_id": execution_id.to_string(),
        "step_index": step_index,
    });

    let result = sqlx::query(
        r#"INSERT INTO token_usage
               (user_id, operation_type, session_id, provider, model,
                input_tokens, output_tokens, total_tokens,
                cached_tokens, cache_read_input_tokens,
                latency_ms, streaming, metadata)
           VALUES ($1, 'maf_orchestration', $2, $3, $4, $5, $6, $7, $8, $8, $9, false, $10)"#,
    )
    .bind(user_id)
    .bind(execution_id.to_string())
    .bind(llm.provider())
    .bind(&usage.model)
    .bind(usage.input_tokens as i32)
    .bind(usage.output_tokens as i32)
    .bind(usage.total_tokens as i32)
    .bind(usage.cached_tokens as i32)
    .bind(usage.latency_ms as i32)
    .bind(metadata)
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::warn!(
            execution_id = %execution_id,
            phase,
            error = %e,
            "maf: token_usage write failed (non-fatal)"
        );
    }
}

/// Runs one MAF execution under a `maf.execution` span.
///
/// The span matters for more than tidiness. MAF previously emitted no spans at
/// all, and forwarded each step a `traceparent` whose ids it derived locally
/// from `(execution_id, step_index)` — see `build_traceparent`. Those ids were
/// well-formed but named a span no exporter ever produced, so in Tempo each
/// step's agent appeared as an orphan tree with no MAF node above it, and the
/// run itself was invisible. Wrapping the run and each step in real spans
/// makes a MAF execution one connected trace: the run, its planning and
/// synthesis phases, each step, and each step agent's own spans beneath it.
// 7 params is at clippy's default threshold; grouping them into a context
// struct isn't worth it for this one call site (worker.rs).
#[allow(clippy::too_many_arguments)]
pub async fn run_maf(
    client: &reqwest::Client,
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
    content: Option<&str>,
) -> Result<StepOutcome, String> {
    // A MAF run is driven by a background worker, not a request, so there is
    // no inbound traceparent to continue — this span is legitimately a root.
    let span = tracing::info_span!(
        "maf.execution",
        otel.kind = "internal",
        // `session.id` is how the platform finds a trace at all: the
        // trace-usage materializer discovers work with the TraceQL
        // `{span.session.id != ""}`, and `agent_session_query` expects it on
        // the server's own dispatch spans. Without it this trace exists in
        // Tempo but is invisible to every reader, so MAF's spend would never
        // reach `trace_usage`.
        //
        // The value is the execution id, which is also the A2A `contextId`
        // MAF sends its step agents — so the run and its agents group under
        // one session rather than appearing as unrelated traces.
        session.id = %execution_id,
        execution_id = %execution_id,
        user_id = %user_id,
        step_count = maf_def.steps.len(),
    );
    run_maf_inner(
        client,
        db,
        flow_guard,
        execution_id,
        user_id,
        maf_def,
        llm,
        content,
    )
    .instrument(span)
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_maf_inner(
    client: &reqwest::Client,
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
    content: Option<&str>,
) -> Result<StepOutcome, String> {
    // Seed one entry per step and persist immediately, so the full step list
    // is visible in the DB before the (possibly slow) planning LLM call even
    // starts.
    //
    // On a retry this carries the previous attempt's completed steps forward
    // instead of starting clean — see `resume_from`. A retry re-enqueues the
    // same `execution_id`, so the earlier attempt's snapshot is still in the
    // row and its finished work can be reused rather than repeated.
    let prior = load_prior_results(db, execution_id).await;
    let resumed = prior.iter().filter(|s| s.status == "success").count();
    let mut step_results: Vec<StepResult> = resume_from(&maf_def.steps, &prior);
    if resumed > 0 {
        tracing::info!(
            execution_id = %execution_id,
            resumed_steps = resumed,
            total_steps = maf_def.steps.len(),
            "maf: resuming from a previous attempt"
        );
    }
    // Progress writes carry 0 cost: the real figure is read back from the
    // platform's priced `token_usage` rows once the run finishes (see
    // `platform_spend`), because a call's cost isn't known until it has been
    // made and metered.
    let mut total_cost = 0f64;
    // Carry the resumed steps' token figures into the running tally so the
    // live number in the UI doesn't visibly drop at the start of a retry. The
    // final value is re-read from `token_usage` regardless.
    let resumed_tokens: i64 = step_results
        .iter()
        .filter(|s| s.status == "success")
        .map(|s| s.tokens_used)
        .sum();
    persist_progress(db, execution_id, &step_results, resumed_tokens, total_cost).await;

    // Register the execution as a platform session.
    //
    // `chat_sessions.session_id` is the A2A `contextId` — per its own schema
    // comment, "the platform-wide session key shared by chat history, agent
    // tasks, and observability". MAF already sends `execution_id` as the
    // contextId to every step agent, so the run IS a session; it just never
    // said so. `agent_proxy` upserts this row before forwarding any message,
    // and MAF has to as well, because `session_traces.session_id` is a
    // foreign key onto this table — without the row, the per-step trace index
    // below cannot be written and the run's agent spend stays undiscoverable.
    //
    // `agent_id` is left NULL deliberately: a MAF run spans several agents
    // and no single one owns the session.
    let session_title = maf_def
        .description
        .clone()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| format!("MAF execution {execution_id}"));
    if let Err(e) = sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, title, session_type)
         VALUES ($1, $2, $3, 'maf_execution')
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(execution_id.to_string())
    .bind(user_id)
    .bind(&session_title)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, execution_id = %execution_id,
            "maf: could not register the execution as a session — its agent spend will not be discoverable"
        );
    }

    // Run-time data for this execution only, folded into step 0's task
    // description before planning — lets one saved workflow shape (e.g.
    // "summarise this, then translate to japanese") be re-run against
    // different content each time instead of baking it in at creation.
    // Every step past 0 still gets its input purely from prior steps'
    // extracted context (see `build_context`), unchanged.
    let planning_steps: Vec<MafStep> = maf_def
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| match (i, content) {
            (0, Some(content)) => MafStep {
                task_description: format!("{} \"{content}\"", step.task_description),
                ..step.clone()
            },
            _ => step.clone(),
        })
        .collect();

    // ── LLM call 1: plan all steps at runtime ────────────────────────────────
    // Generates prompt templates (with <placeholders>), to_extract goals, and
    // the output_generation guideline from the task descriptions.
    // Planning happens on every execution (Python MAF parity).
    let ((step_plans, output_generation), plan_usage) = metered_llm(
        "plan_execution",
        db,
        execution_id,
        user_id,
        None,
        llm,
        plan_execution(&planning_steps, llm),
    )
    .await?;
    let mut total_tokens = plan_usage.total_tokens + resumed_tokens;

    // Fill in the prompt template / extraction goal now that planning is
    // done — steps stay "pending" until their turn in the loop below.
    //
    // Steps carried over from an earlier attempt keep the plan they actually
    // ran under. Planning is re-run every attempt (Python MAF parity, and the
    // guideline it returns is not persisted), but its output for an
    // already-finished step describes a call that never happened — storing it
    // would make the recorded template disagree with the recorded prompt.
    for (result, plan) in step_results.iter_mut().zip(step_plans.iter()) {
        if result.status == "success" {
            continue;
        }
        result.prompt_template = plan.prompt.clone();
        result.to_extract = plan.to_extract.clone();
    }
    persist_progress(db, execution_id, &step_results, total_tokens, total_cost).await;

    // Durable so a resumed run's final synthesis follows the same guidance the original run
    // planned — otherwise only held in this function's local `output_generation`, lost on resume.
    persist_output_generation(db, execution_id, &output_generation).await;

    for (i, (step, plan)) in maf_def.steps.iter().zip(step_plans.iter()).enumerate() {
        // Carried over from an earlier attempt. Skipping is not just an
        // optimisation: agent calls are not idempotent, so re-running a step
        // that already succeeded can repeat its side effects (a sent mail, a
        // filed ticket) once per attempt. Its `extracted_info` is already in
        // `step_results`, so `build_context` below still feeds it to the
        // steps that follow.
        if step_results[i].status == "success" {
            continue;
        }

        let context = build_context(&step_results[..i]);
        match execute_step(
            client,
            db,
            flow_guard,
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
        db,
        execution_id,
        user_id,
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
    flow_guard: &Arc<FlowGuard>,
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
        flow_guard,
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
            flow_guard,
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
        db,
        execution_id,
        user_id,
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
    flow_guard: &Arc<FlowGuard>,
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
    let (actual_prompt, prompt_usage) = match metered_llm(
        "generate_step_prompt",
        db,
        execution_id,
        user_id,
        Some(step.step_index),
        llm,
        generate_step_prompt(&plan.prompt, &step.task_description, context, llm),
    )
    .await
    {
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

    // The forwarded traceparent keeps the locally-derived, per-step trace
    // id — deliberately, and NOT the id of the `maf.step` span below.
    //
    // It is tempting to forward the real span's ids so the agent's spans
    // parent to an exported node. But every `maf.step` span is a child of
    // the one `maf.execution` root, so they all share a single trace id,
    // and two things depend on each step having its OWN:
    //
    //   1. Usage attribution. `trace_usage` is keyed by
    //      `(trace_id, agent_name)`, so a workflow that uses one agent for
    //      several steps would collapse into a single row — each step
    //      would report the whole run's tokens, and the totals would
    //      multiply them by the step count.
    //   2. Authorization. This id is the `flows.flow_id` the MCP gateway
    //      and LLM router check this step's agent against, and the
    //      participant record below is written per (flow_id, agent_id).
    //
    // The trade-off is that an agent's spans form their own trace rather
    // than nesting under the MAF span. `maf.trace_id` on the step span
    // below records the link so a Tempo reader can hop across.
    let (traceparent, trace_id) = build_traceparent(execution_id, step.step_index);

    // One real span per step. MAF previously emitted none at all, so a run
    // was entirely invisible in Tempo; these give the run, its phases and
    // its steps real exported nodes.
    let step_span = tracing::info_span!(
        "maf.step",
        otel.kind = "client",
        gen_ai.operation.name = "invoke_agent",
        execution_id = %execution_id,
        step_index = step.step_index,
        agent.id = %step.agent_id,
        agent.name = %step.agent_name,
        // The trace the agent's own spans will land under.
        maf.trace_id = %trace_id,
    );

    // Record the trace id before the call, not after it succeeds: an agent
    // that burns tokens and then fails (or times out) still produced spans,
    // and this is the only handle the usage API has to find them.
    step_results[i].trace_id = Some(trace_id.clone());

    // Register this step as a flow so the LLM gateway sees it as IN-FLOW (not
    // inert) and its tier classifier can fire. The invariant the gateway relies
    // on (see `derive_boundary_signals`): the trace_id we forward in
    // `traceparent` IS the `flow_id` in this row — mirroring the orchestrator /
    // agent-proxy ingress. `context_id = execution_id` is stable across every
    // step, so the gateway keys its decision cache on the whole MAF run: the
    // first step writes the tier decision, later steps reuse it. Best-effort —
    // a failed insert only means this step falls back to the default model.
    let flow_metadata = serde_json::json!({
        "context_id": execution_id.to_string(),
        "mode": "free_flowing",
    });
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata)
           VALUES ($1, $2, $3, $4, 'running', $5)
           ON CONFLICT (flow_id) DO NOTHING"#,
    )
    .bind(&trace_id)
    .bind(user_id)
    .bind(&step.agent_name)
    .bind(&step.task_description)
    .bind(&flow_metadata)
    .execute(db)
    .await;
    // Participant record — the MCP gateway / LLM router only authorize this
    // step's agent for calls carrying this trace id if it is recorded here
    // (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4). Same synchronous pre-call write
    // as the flows row; a failed insert denies (never escalates) downstream.
    if let Err(e) = sqlx::query(
        "INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)
         ON CONFLICT (flow_id, agent_id) DO NOTHING",
    )
    .bind(&trace_id)
    .bind(step.agent_id)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, flow_id = %trace_id, agent_id = %step.agent_id,
            "flow participant record failed — the step agent's MCP/LLM calls will be denied"
        );
    }

    // Index this step's trace against the run so the platform can FIND it.
    //
    // The trace-usage materializer discovers work two ways: a TraceQL
    // search for `{span.session.id != ""}`, unioned with this table. A
    // step's trace contains only the agent's own spans — MAF's `maf.step`
    // span lives on the execution trace, not this one — and agents that
    // don't run the instrumentation patch never set `session.id`, which is
    // exactly the gap this index exists to cover.
    //
    // Without this row a step's agent spend reaches Tempo and is never
    // found, so `trace_usage` stays empty and the usage API can never
    // resolve the step. `agent_proxy.rs` and `a2a_dispatch.rs` both write
    // it; MAF did not, which is why it kept missing pieces of what the
    // proxy does. Best-effort: a failed index only costs visibility.
    if let Err(e) = sqlx::query(
        "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (session_id, trace_id) DO NOTHING",
    )
    .bind(execution_id.to_string())
    .bind(&trace_id)
    .bind(step.agent_id)
    .bind(&step.agent_name)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, execution_id = %execution_id, %trace_id,
            "maf: session trace index failed — this step's agent spend will not be discoverable"
        );
    }

    // Resolve the agent's endpoint NOW rather than trusting the one frozen
    // into `maf_json` when the workflow was saved.
    //
    // `step.agent_endpoint` is a snapshot from creation time, and Docker
    // hands out a fresh random host port every time a container is
    // recreated. So a saved workflow kept pointing at a port that had
    // since moved, and every run failed with a connection error to a dead
    // address — permanently, for the life of the workflow. This is the
    // same staleness `agent_proxy.rs` documents and resolves for its own
    // path; MAF simply never did the lookup.
    //
    // A missing snapshot on the agent row (`endpoint: None`, e.g. a k8s
    // deploy that returned before the pod was Ready) falls back to the
    // stored value rather than failing outright — matching the proxy's
    // handling, and the only route by which an externally-registered
    // agent still works.
    let endpoint = match nasiko_agent_proxy::resolve(db, step.agent_id).await {
        Ok(resolved) => resolved
            .endpoint
            .map(|e| format!("http://{}:{}", e.host, e.port))
            .unwrap_or_else(|| step.agent_endpoint.clone()),
        Err(e) => {
            let err = format!(
                "step {} (agent '{}'): {e}",
                step.step_index, step.agent_name
            );
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err);
        }
    };

    // Cascade limits, enforced before the call goes out.
    //
    // MAF calls its step agents directly rather than through the server
    // proxy, so it never passed the chokepoint where the platform applies
    // FlowGuard — depth, fan-out, per-flow token budget, wall clock and
    // cycle detection. Every other dispatch path is bounded by these;
    // MAF was the one that wasn't, so a workflow could fan out or loop
    // without limit. This applies the same guard instance the proxy and
    // A2A dispatch use, received from the composition root.
    //
    // The context is scoped to the STEP, not the run. A run-level context
    // would put the whole workflow under one `flow_timeout_secs` window
    // (120s by default) and one fan-out budget, which would reject
    // ordinary long or many-step workflows. Per-step also matches the
    // `flows` row written just above and the window the MCP gateway
    // authorises against, so all three agree on what "this step" means.
    let flow_ctx = FlowContext {
        flow_id: trace_id.clone(),
        // The span id from the traceparent this step forwards, so the
        // guard's view of the call and the agent's view agree.
        parent_span_id: traceparent
            .split('-')
            .nth(2)
            .unwrap_or_default()
            .to_string(),
    };
    // The root is the CALLER, not the callee: `init_flow` seeds the call
    // chain with this name, so naming the target here would make cycle
    // detection reject the very first call to it. Mirrors the A2A
    // dispatch path, which seeds "orchestrator".
    flow_guard.init_flow(&flow_ctx, "maf").await;
    if let Err(rejection) = flow_guard.check(&flow_ctx, &step.agent_name).await {
        let err = format!(
            "step {} (agent '{}') blocked by flow limits: {rejection}",
            step.step_index, step.agent_name
        );
        step_results[i].status = "failed".to_string();
        step_results[i].prompt = actual_prompt;
        step_results[i].error = Some(err.clone());
        persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
        return StepLoopOutcome::Failed(err);
    }
    if let Err(rejection) = flow_guard
        .record_invocation(&flow_ctx, &step.agent_name)
        .await
    {
        let err = format!(
            "step {} (agent '{}') blocked by flow limits: {rejection}",
            step.step_index, step.agent_name
        );
        step_results[i].status = "failed".to_string();
        step_results[i].prompt = actual_prompt;
        step_results[i].error = Some(err.clone());
        persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
        return StepLoopOutcome::Failed(err);
    }

    let raw_response = timed(
        "call_agent",
        execution_id,
        Some(step.step_index),
        call_agent(
            client,
            &endpoint,
            &execution_id.to_string(),
            &user_id.to_string(),
            &actual_prompt,
            &traceparent,
        ),
    )
    .instrument(step_span)
    .await;

    // Unwind the guard's call stack whether or not the call succeeded —
    // `record_invocation` pushed onto it, so an early return here would
    // leave the frame in place and make cycle detection reject a later,
    // legitimate call to the same agent.
    flow_guard.record_return(&flow_ctx).await;

    // Close the flow now the agent call has returned.
    //
    // MAF was the only dispatch path that never did this — it only ever
    // INSERTed. Two consequences it fixes: the MCP gateway authorises any
    // caller presenting this trace id for as long as the row is `running`
    // and younger than `flow_timeout_secs`, so leaving it open held the
    // window open for the full 120s no matter when the call actually
    // ended; and the rows accumulated permanently in `running`, polluting
    // `/api/flows/*`. Closed on failure too — a failed step's window
    // should shut at least as promptly as a successful one's.
    if let Err(e) = sqlx::query(
        r#"UPDATE flows SET status = 'completed',
           duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
           completed_at = now()
           WHERE flow_id = $1"#,
    )
    .bind(&trace_id)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, flow_id = %trace_id,
            "maf: could not close flow — its authorization window stays open until it ages out"
        );
    }

    let raw_response = match raw_response {
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
    tracing::info!(
        step = step.step_index,
        agent_name = %step.agent_name,
        raw_response_len = raw_response.len(),
        raw_response_preview = %raw_response.chars().take(300).collect::<String>(),
        "maf run: raw agent response"
    );

    finish_step(
        db,
        flow_guard,
        &flow_ctx,
        execution_id,
        user_id,
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
        prompt_usage.total_tokens,
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
    flow_guard: &Arc<FlowGuard>,
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
    // Same per-step flow scope `execute_step` opened for the pre-pause call, rebuilt from the
    // same deterministic traceparent — so the resumed half of the step is charged against the
    // step's own budget rather than opening a second one.
    let flow_ctx = FlowContext {
        flow_id: trace_id.clone(),
        parent_span_id: traceparent
            .split('-')
            .nth(2)
            .unwrap_or_default()
            .to_string(),
    };

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
        flow_guard,
        &flow_ctx,
        execution_id,
        user_id,
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
        // No LLM call 2 on resume — the prompt was already generated (and its tokens already
        // unaccounted-for, same as the pre-existing behavior for a step that errors before
        // reaching this point) before the pause.
        0,
    )
    .await
}
/// Shared tail of both `execute_step` and `continue_paused_step` once a real (non-paused) agent
/// reply is in hand: LLM call 3 (extraction), token accounting, and the `"success"` persist.
#[allow(clippy::too_many_arguments)]
async fn finish_step(
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    flow_ctx: &FlowContext,
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
    actual_prompt: String,
    raw_response: String,
    latency_ms: i64,
    prompt_tokens: i64,
) -> StepLoopOutcome {
    // ── LLM call 3: extract relevant info from agent response ─────────────
    let (extracted, extract_usage) = match metered_llm(
        "extract_info",
        db,
        execution_id,
        user_id,
        Some(step.step_index),
        llm,
        extract_info(
            &plan.prompt,
            &actual_prompt,
            &raw_response,
            &plan.to_extract,
            context,
            llm,
        ),
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

    let llm_tokens = prompt_tokens + extract_usage.total_tokens;

    // Charge this step's reasoning against the flow's token budget. The
    // budget is only meaningful if something reports spend into it, and
    // this is the spend MAF knows synchronously. The step agent's own LLM
    // calls are charged separately by whichever path serves them — they
    // carry this same trace id, so they land against the same flow.
    if llm_tokens > 0
        && let Err(e) = flow_guard.record_tokens(flow_ctx, llm_tokens as u64).await
    {
        tracing::warn!(
            execution_id = %execution_id,
            step_index = step.step_index,
            error = %e,
            "maf: flow token budget exceeded"
        );
    }

    // Agent-side token usage is deliberately NOT collected here. It exists
    // only as OTel span attributes, and agents batch-export spans every
    // ~5s, so reading it inline stalled every single step for up to 10s
    // waiting on a number that nothing downstream consumes (not the next
    // step's context, not the final synthesis) — pure added latency on the
    // user-visible path.
    //
    // The same spans are materialized into `trace_usage` by the
    // trace-usage worker, keyed by trace id. Because `build_traceparent`
    // derives each step's trace id deterministically from
    // (execution_id, step_index), that table can be joined straight back
    // to these steps after the fact: see
    // `GET /api/maf/execution/{id}/usage`, which the UI polls separately
    // and renders when it becomes available.
    //
    // Reading it out-of-band rather than back-filling it in also removes
    // the lost-update race the previous detached backfill had against this
    // loop's own `persist_progress` full-row overwrites.
    let step_tokens = llm_tokens;
    *total_tokens += step_tokens;
    tracing::info!(
        execution_id = %execution_id,
        step_index = step.step_index,
        agent_name = %step.agent_name,
                llm_tokens,
        step_tokens,
        running_total_tokens = *total_tokens,
        "maf run: token usage (MAF reasoning only; agent usage served by the usage API)"
    );

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
    // The agent-usage fields (input/output/cache/model/cost) stay at their
    // zero defaults in the stored row — they are served by the usage API,
    // which reads them from `trace_usage` keyed on the `trace_id` recorded
    // above.
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
#[allow(clippy::too_many_arguments)]
async fn finish_run(
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    mut step_results: Vec<StepResult>,
    mut total_tokens: i64,
    total_cost: f64,
    output_generation: &str,
    llm: &LlmClient,
) -> Result<StepOutcome, String> {
    // ── LLM call 4: synthesise final output ───────────────────────────────────
    // Use the guidelines generated by the planner at runtime.
    let guidelines = output_generation;

    let (output, output_usage) = metered_llm(
        "generate_final_output",
        db,
        execution_id,
        user_id,
        None,
        llm,
        generate_final_output(&step_results, guidelines, llm),
    )
    .await
    .map_err(|e| format!("final output generation failed: {e}"))?;
    total_tokens += output_usage.total_tokens;

    // Report what the platform recorded, not what MAF counted.
    //
    // Every LLM call above wrote a `token_usage` row, and the
    // `calculate_usage_cost_trigger` priced it from `model_pricing`. Those
    // rows are what the platform bills from, so reading them back is what
    // makes the reported figure true by construction rather than a second
    // tally that can drift from it.
    //
    // Safe to read here: `metered_llm` awaits each insert, so every row for
    // this execution is committed. It is a plain read at the very end of the
    // run, so it reintroduces none of the write-race the old detached
    // backfill had.
    let spend = platform_spend(db, execution_id).await;

    // Restate the per-step figures from the same rows the total came from.
    //
    // Until now the execution total was platform-sourced while the step rows
    // were MAF's own running tally — two sources for one quantity, free to
    // drift (a metering write that failed lowers the total but not the steps).
    // Overwriting them here means every token figure MAF reports, at every
    // level, traces to the same `token_usage` rows the platform bills from.
    //
    // Note the steps still will not sum to the total, and correctly so:
    // planning and final synthesis belong to the run, not to any step.
    for result in step_results.iter_mut() {
        if let Some((tokens, cost)) = spend.by_step.get(&result.step_index) {
            result.tokens_used = *tokens;
            result.cost_usd = *cost;
        }
    }

    // MAF's own running tally survives only for the case where metering itself
    // failed and the read came back empty — a stale number beats a zero that
    // reads as "this run was free".
    let tokens_used = if spend.total_tokens > 0 {
        spend.total_tokens
    } else {
        total_tokens
    };
    let cost_usd = if spend.total_tokens > 0 {
        spend.total_cost_usd
    } else {
        total_cost
    };

    Ok(StepOutcome::Completed(ExecutionResult {
        output,
        step_results,
        tokens_used,
        cost_usd,
    }))
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

/// This execution's orchestration spend — tokens and USD — as recorded and
/// priced by the platform.
///
/// Read back rather than accumulated in memory, so the number MAF reports is
/// the same one the platform bills from. A locally-summed total can silently
/// disagree with `token_usage` (a metering write that failed, a partially
/// completed run, a retry that re-ran some steps); reading makes the two
/// agree by construction, and there is exactly one source of truth.
///
/// Scope is MAF's own LLM calls only. Agent-side spend is metered separately
/// against each step's own trace and served by
/// `GET /api/maf/execution/{id}/usage`.
///
/// Returns zeroes on error — a missing figure must never fail a run, or turn
/// a real failure into a different one.
pub(super) async fn platform_spend(db: &PgPool, execution_id: Uuid) -> PlatformSpend {
    /// `(step_index, tokens, cost_usd)` — step_index is NULL for run-level phases.
    type SpendRow = (Option<i32>, Option<i64>, Option<f64>);

    let result: Result<Vec<SpendRow>, sqlx::Error> = sqlx::query_as(
        "SELECT (metadata->>'step_index')::int,
                COALESCE(SUM(total_tokens), 0)::BIGINT,
                COALESCE(SUM(cost_usd), 0)::DOUBLE PRECISION
           FROM token_usage
          WHERE operation_type = 'maf_orchestration'
            AND session_id = $1
          GROUP BY 1",
    )
    .bind(execution_id.to_string())
    .fetch_all(db)
    .await;

    let rows = match result {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                execution_id = %execution_id,
                error = %e,
                "maf: could not read back platform-recorded spend (reporting 0)"
            );
            return PlatformSpend::default();
        }
    };

    let mut spend = PlatformSpend::default();
    for (step_index, tokens, cost) in rows {
        let tokens = tokens.unwrap_or(0);
        let cost = cost.unwrap_or(0.0);
        spend.total_tokens += tokens;
        spend.total_cost_usd += cost;
        // A NULL step_index is a run-level phase (planning, final synthesis) —
        // real spend that belongs to no single step, which is exactly why the
        // execution total is larger than the sum of its steps.
        if let Some(i) = step_index {
            spend.by_step.insert(i, (tokens, cost));
        }
    }
    spend
}

/// One execution's orchestration spend, split the way the platform recorded it.
#[derive(Default)]
pub(super) struct PlatformSpend {
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    /// `step_index -> (tokens, cost_usd)`. Excludes run-level phases, so these
    /// deliberately do not sum to the total.
    pub by_step: std::collections::HashMap<i32, (i64, f64)>,
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
        trace_id: None,
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

/// This execution's last persisted step snapshot, or an empty vec when there
/// is none (first attempt) or it can't be read.
///
/// An unreadable or malformed snapshot is not an error: `resume_from` treats
/// an empty prior as "start clean", which is exactly the old behaviour. A
/// retry that re-runs everything is wasteful, never wrong.
async fn load_prior_results(db: &PgPool, execution_id: Uuid) -> Vec<StepResult> {
    let stored: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT step_results FROM maf_executions WHERE id = $1")
            .bind(execution_id)
            .fetch_optional(db)
            .await
            .unwrap_or(None)
            .flatten();

    let Some(value) = stored else {
        return Vec::new();
    };
    serde_json::from_value(value).unwrap_or_else(|e| {
        tracing::warn!(
            error = %e, execution_id = %execution_id,
            "maf: previous step snapshot could not be parsed — restarting from the first step"
        );
        Vec::new()
    })
}

/// Builds this attempt's starting step list, carrying forward the leading run
/// of steps that already succeeded on a previous attempt.
///
/// Only a *prefix* is reused. Execution is sequential and aborts at the first
/// failure, so successes always form one; taking only the prefix means a
/// snapshot that somehow disagrees degrades into re-running more, never into
/// running a step against context its predecessor never produced.
///
/// Falls back to a clean run whenever the snapshot doesn't describe this
/// definition — a different step count, or the same position holding a
/// different `step_id`. Reusing output across a changed definition would feed
/// one step's result into another step's prompt.
fn resume_from(steps: &[MafStep], prior: &[StepResult]) -> Vec<StepResult> {
    let fresh = || steps.iter().map(pending_result).collect::<Vec<_>>();

    if prior.len() != steps.len() {
        return fresh();
    }
    if prior
        .iter()
        .zip(steps)
        .any(|(old, s)| old.step_id != s.step_id)
    {
        return fresh();
    }

    let completed = prior
        .iter()
        .take_while(|old| old.status == "success")
        .count();

    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            if i < completed {
                prior[i].clone()
            } else {
                // Anything not carried over restarts clean, which also clears
                // the failed step's stale `error` so the UI doesn't show last
                // attempt's message while this one is running.
                pending_result(step)
            }
        })
        .collect()
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
) -> Result<((Vec<StepPlan>, String), LlmUsage), String> {
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

    let (json, usage) = llm
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

    Ok(((plans, output_generation), usage))
}

// ─── LLM call 2: prompt generator ────────────────────────────────────────────

async fn generate_step_prompt(
    template: &str,
    task_description: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, LlmUsage), String> {
    // No placeholders — send the template verbatim, no LLM call needed.
    // Zeroed usage rather than a real one: nothing was spent, so no
    // `token_usage` row should be written for this step's prompt phase.
    if !template.contains('<') {
        return Ok((template.to_string(), LlmUsage::default()));
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

    let (json, usage) = llm
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

    Ok((prompt, usage))
}

// ─── LLM call 3: extractor ────────────────────────────────────────────────────

async fn extract_info(
    template: &str,
    actual_prompt: &str,
    response: &str,
    goal: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, LlmUsage), String> {
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

    let (json, usage) = llm
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

    Ok((extracted, usage))
}

// ─── LLM call 4: final output synthesiser ────────────────────────────────────

async fn generate_final_output(
    step_results: &[StepResult],
    guidelines: &str,
    llm: &LlmClient,
) -> Result<(String, LlmUsage), String> {
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

    let (json, usage) = llm
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

    Ok((output, usage))
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
        // Some agents (e.g. web-search-agent) stream their answer as many
        // artifacts, one word/token each ({"parts":[{"text":"The"}]},
        // {"parts":[{"text":" Eiffel"}]}, ...) rather than one artifact
        // holding the full text — concatenate every part across every
        // artifact, in order, or only the first token survives.
        if let Some(artifacts) = root["artifacts"].as_array() {
            let combined: String = artifacts
                .iter()
                .flat_map(|a| a["parts"].as_array().into_iter().flatten())
                .filter_map(|p| p["text"].as_str())
                .collect();
            if !combined.is_empty() {
                return combined;
            }
        }
        if let Some(parts) = root["status"]["message"]["parts"].as_array() {
            let combined: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
            if !combined.is_empty() {
                return combined;
            }
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

#[cfg(test)]
mod resume_tests {
    use super::*;

    fn step(index: i32) -> MafStep {
        MafStep {
            step_id: Uuid::new_v4(),
            step_index: index,
            agent_id: Uuid::new_v4(),
            agent_name: format!("agent-{index}"),
            agent_endpoint: "http://localhost:1".to_string(),
            task_description: format!("task {index}"),
        }
    }

    /// A finished step as the previous attempt would have persisted it.
    fn succeeded(step: &MafStep) -> StepResult {
        StepResult {
            status: "success".to_string(),
            extracted_info: Some(format!("output {}", step.step_index)),
            prompt: format!("prompt {}", step.step_index),
            prompt_template: format!("template {}", step.step_index),
            tokens_used: 100,
            ..pending_result(step)
        }
    }

    fn failed(step: &MafStep) -> StepResult {
        StepResult {
            status: "failed".to_string(),
            error: Some("agent timed out".to_string()),
            ..pending_result(step)
        }
    }

    #[test]
    fn first_attempt_starts_every_step_pending() {
        let steps = vec![step(0), step(1)];
        let merged = resume_from(&steps, &[]);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|s| s.status == "pending"));
    }

    #[test]
    fn completed_prefix_is_carried_forward_and_the_rest_restarts() {
        let steps = vec![step(0), step(1), step(2)];
        let prior = vec![
            succeeded(&steps[0]),
            succeeded(&steps[1]),
            failed(&steps[2]),
        ];

        let merged = resume_from(&steps, &prior);

        // The two that finished keep their output, so `build_context` can
        // still feed the step that failed.
        assert_eq!(merged[0].status, "success");
        assert_eq!(merged[1].extracted_info.as_deref(), Some("output 1"));
        assert_eq!(merged[1].tokens_used, 100);
        // The failed one restarts clean, without last attempt's error.
        assert_eq!(merged[2].status, "pending");
        assert_eq!(merged[2].error, None);
    }

    #[test]
    fn a_run_that_failed_after_its_last_step_reruns_no_steps() {
        // Every step succeeded and the failure came later (final synthesis).
        // The retry should go straight to that synthesis.
        let steps = vec![step(0), step(1)];
        let prior = vec![succeeded(&steps[0]), succeeded(&steps[1])];

        let merged = resume_from(&steps, &prior);

        assert!(merged.iter().all(|s| s.status == "success"));
    }

    #[test]
    fn only_the_leading_run_of_successes_is_reused() {
        // A snapshot with a gap can't have come from a sequential run. Reusing
        // step 2 would run it against context step 1 never produced, so the
        // prefix rule stops at the gap.
        let steps = vec![step(0), step(1), step(2)];
        let prior = vec![
            succeeded(&steps[0]),
            failed(&steps[1]),
            succeeded(&steps[2]),
        ];

        let merged = resume_from(&steps, &prior);

        assert_eq!(merged[0].status, "success");
        assert_eq!(merged[1].status, "pending");
        assert_eq!(merged[2].status, "pending");
    }

    #[test]
    fn a_snapshot_of_a_different_definition_is_discarded() {
        let steps = vec![step(0), step(1)];
        // Same length and positions, but step 1 is a different step entirely.
        let other = step(1);
        let prior = vec![succeeded(&steps[0]), succeeded(&other)];

        let merged = resume_from(&steps, &prior);

        assert!(
            merged.iter().all(|s| s.status == "pending"),
            "a mismatched snapshot must not seed any step"
        );
    }

    #[test]
    fn a_snapshot_with_a_different_step_count_is_discarded() {
        let steps = vec![step(0), step(1)];
        let prior = vec![succeeded(&steps[0])];

        let merged = resume_from(&steps, &prior);

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().all(|s| s.status == "pending"));
    }

    /// Security/robustness regression (found in review): `step_results` (from `maf_executions`)
    /// and `maf_def.steps` (from the separate `maf_json` snapshot column) are never cross-checked
    /// on the way in. The old guard only bounded `resume_index` itself against both collections,
    /// so a `step_results` shorter than `maf_def.steps` — entirely possible since the two come
    /// from different columns — would pass that check whenever `resume_index` still landed inside
    /// both, then panic on a direct index (`plans[i]`) once the post-resume loop's `i` walked past
    /// `step_results.len()`. This must return a clean `Err`, never panic, and must do so before
    /// touching `db`/`client`/`flow_guard`/`llm` at all — none of those are wired to anything
    /// real below, so a panic or an actual I/O attempt both fail this test.
    #[tokio::test]
    async fn run_maf_from_rejects_a_step_results_length_mismatch_instead_of_panicking() {
        let steps: Vec<MafStep> = (0..3).map(step).collect();
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
        let flow_guard = Arc::new(FlowGuard::new(
            redis::Client::open("redis://127.0.0.1:1").expect("lazy client, never dialled"),
            nasiko_flow::FlowConfig::default(),
        ));
        let llm = LlmClient::new(reqwest::Client::new(), String::new(), None, String::new());

        let result = run_maf_from(
            &client,
            &db,
            &flow_guard,
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
