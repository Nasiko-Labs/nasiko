use crate::outbound::OutboundPrefix;
use crate::target::{AgentTargetResolver, ForwardTarget, ResolveCaller, ResolveFailure};
use nasiko_types::agent_clock::AgentClock;
use std::sync::Arc;
use std::time::Instant;

use nasiko_flow::{FlowContext, FlowGuard};
use nasiko_pricing::{LedgerEntry, UsageLedger};
use nasiko_types::a2a::{
    PauseReason, SseEvent, StreamDisposition, build_send_request_for_task,
    build_stream_request_for_task, classify_sse_event,
};
use nasiko_types::agent_call::AgentCallMeter;
use sqlx::PgPool;
use tracing::Instrument as _;
use uuid::Uuid;

use super::llm::{ChatMessage, LlmClient, LlmError, LlmUsage};
use super::types::{
    ExecutionResult, MafDefinition, MafStep, PausedStep, RunError, StepOutcome, StepResult,
};

/// What one step's agent call produced, before extraction — a normal reply, or a pause that must
/// stop the step (and the whole run) right there instead of being treated as an oddly-worded
/// normal reply.
#[derive(Debug, PartialEq)]
enum AgentCallOutcome {
    Completed(String),
    Paused { task_id: String, raw_data: String },
}

/// What one step's execution produced — shared between a fresh step (`execute_step`) and a
/// resumed one (`continue_paused_step`), so `run_maf`/`run_maf_from` branch on the same three
/// outcomes regardless of which path produced them. `Advance` carries the agent's raw reply.
pub enum StepLoopOutcome {
    Advance(String),
    AwaitingHuman(PausedStep),
    Failed(RunError),
}

/// Times an awaited step and emits it as an `info` event — visible under the
/// default `RUST_LOG=info` with no special filter — giving a per-call timing
/// breakdown of a MAF run (planning, per-step LLM calls, the agent HTTP round
/// trip) without needing an OTel exporter configured at all.
///
/// This is deliberately kept alongside the spans added by `run_maf`: spans go
/// to Tempo and are the only way to see the trace tree, but they produce
/// nothing in a deployment with no collector, which is the common local case.
/// Count one step's agent call as in flight to `agent_id` for exactly as long as `call` runs.
/// A pause is a reply, so only an `Err` counts as a failed call.
async fn metered<T, E>(
    agent_calls: &Arc<dyn AgentCallMeter>,
    agent_id: Uuid,
    call: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut in_flight = agent_calls.begin(agent_id);
    let result = call.await;
    in_flight.set_succeeded(result.is_ok());
    result
}

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
/// swallowed, matching `write_selector_usage` and the router's
/// `spawn_log`.
pub async fn metered_llm<T, E>(
    phase: &'static str,
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    step_index: Option<i32>,
    llm: &LlmClient,
    fut: impl std::future::Future<Output = Result<(T, LlmUsage), E>>,
) -> Result<(T, LlmUsage), E> {
    let source = UsageSource {
        provider: llm.provider(),
        key_source: "platform",
        ledger: llm.ledger(),
    };
    // A routed call is already in `trace_usage`, written by the router; writing it here too
    // would add its tokens to the row twice.
    let record = !llm.is_routed();
    let fut = super::llm::MAF_STEP.scope(step_index, fut);
    metered_call_inner(
        phase,
        db,
        execution_id,
        user_id,
        step_index,
        source,
        record,
        fut,
    )
    .await
}

/// Who served a metered call and whose key paid for it, as recorded on its span and
/// `trace_usage` row, and the ledger the call is handed to. The ledger is shared, not
/// borrowed, because the write runs as its own task (see [`metered_call`]).
#[derive(Clone)]
pub struct UsageSource<'a> {
    pub provider: &'a str,
    /// `"platform"` for the platform's own key, `"user"` for the run owner's.
    pub key_source: &'a str,
    pub ledger: Arc<dyn UsageLedger>,
}

/// [`metered_llm`] for a model call that does not go through [`LlmClient`]: the same span,
/// timing log and `token_usage` row, attributed to `source`. The error type is the
/// caller's, so a call that cannot fail can say so with `Infallible`.
pub async fn metered_call<T, E>(
    phase: &'static str,
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    step_index: Option<i32>,
    source: UsageSource<'_>,
    fut: impl std::future::Future<Output = Result<(T, LlmUsage), E>>,
) -> Result<(T, LlmUsage), E> {
    metered_call_inner(
        phase,
        db,
        execution_id,
        user_id,
        step_index,
        source,
        true,
        fut,
    )
    .await
}

/// [`metered_call`]; `record` false keeps the span but skips the `trace_usage` write.
#[allow(clippy::too_many_arguments)]
async fn metered_call_inner<T, E>(
    phase: &'static str,
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    step_index: Option<i32>,
    source: UsageSource<'_>,
    record: bool,
    fut: impl std::future::Future<Output = Result<(T, LlmUsage), E>>,
) -> Result<(T, LlmUsage), E> {
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
        gen_ai.system = source.provider,
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
        // Awaited, so a run's spend is recorded before it reads it back (`platform_spend`), but
        // run as its own task: a stopped run is dropped wherever it is, and a write dropped
        // mid-transaction would roll back the record of a call that was already paid for.
        // Router-backed calls log usage in the router, so `record` is false for them.
        if record {
            let write = write_trace_usage(
                db.clone(),
                execution_id,
                user_id,
                phase,
                step_index,
                OwnedSource::from(&source),
                usage.clone(),
            );
            if let Err(e) = super::stop::spawn_usage_write(execution_id, write).await {
                tracing::warn!(
                    execution_id = %execution_id,
                    phase,
                    error = %e,
                    "maf: trace_usage write task failed (non-fatal)"
                );
            }
        }
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

/// Add one MAF orchestration LLM call to `trace_usage`.
///
/// `session_id` is the execution id, which is also the A2A `contextId` MAF
/// sends its step agents — so MAF's own spend and its agents' spend aggregate
/// under one session, the same convention the LLM router follows by writing
/// the flow id there.
async fn write_trace_usage(
    db: PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    phase: &'static str,
    step_index: Option<i32>,
    source: OwnedSource,
    usage: LlmUsage,
) {
    let metadata = serde_json::json!({
        "key_source": source.key_source,
        "component": "maf",
        "phase": phase,
        "execution_id": execution_id.to_string(),
        "step_index": step_index,
    });

    let session_id = execution_id.to_string();
    let agent_name: String = maf_agent_name(step_index);
    let result = insert_trace_usage(
        &db,
        source.ledger.as_ref(),
        user_id,
        Some(&session_id),
        &agent_name,
        &source.provider,
        &usage,
        metadata,
    )
    .await;

    if let Err(e) = result {
        tracing::warn!(
            execution_id = %execution_id,
            phase,
            error = %e,
            "maf: trace_usage write failed (non-fatal)"
        );
    }
}

/// A [`UsageSource`] the write's own task can hold.
struct OwnedSource {
    provider: String,
    key_source: String,
    ledger: Arc<dyn UsageLedger>,
}

impl From<&UsageSource<'_>> for OwnedSource {
    fn from(source: &UsageSource<'_>) -> Self {
        Self {
            provider: source.provider.to_string(),
            key_source: source.key_source.to_string(),
            ledger: source.ledger.clone(),
        }
    }
}

/// `trace_usage.agent_name` for a MAF call: `maf:step<i>` for a step phase,
/// `maf` for run-level phases (planning, synthesis) and branch-condition writes.
/// Per-step rows keep [`platform_spend`]'s step split readable from the upsert.
pub fn maf_agent_name(step_index: Option<i32>) -> String {
    match step_index {
        Some(i) => format!("maf:step{i}"),
        None => "maf".to_string(),
    }
}

/// Add one MAF orchestration LLM call to its `trace_usage` row
/// `(session_id or direct:<uuid v4>, agent_name)`, source `agent`, priced_by `router`.
/// The one place MAF's spend columns are written; each caller describes its call
/// in `metadata` (merged into the row's metadata, later keys win).
///
///
/// The row is priced here, by the same engine and with the same `metadata.pricing`
/// record as the LLM router's rows: an unpriced row would stay at $0 (the pricing
/// trigger was dropped in migration 0043). The call is handed to `ledger` on the same
/// transaction.
// One value per column the row and its ledger entry need; a params struct would only repeat them.
#[allow(clippy::too_many_arguments)]
pub async fn insert_trace_usage(
    db: &PgPool,
    ledger: &dyn UsageLedger,
    user_id: Uuid,
    session_id: Option<&str>,
    agent_name: &str,
    provider: &str,
    usage: &LlmUsage,
    mut metadata: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let priced = nasiko_pricing::PricingEngine::new(db.clone())
        .price(
            Some(provider),
            &usage.model,
            nasiko_pricing::RawUsage {
                input: usage.input_tokens.max(0) as u64,
                output: usage.output_tokens.max(0) as u64,
                cache_read: usage.cached_tokens.max(0) as u64,
                cache_creation: 0,
                total: Some(usage.total_tokens.max(0) as u64),
            },
            nasiko_pricing::PromptConvention::Inclusive,
            chrono::Utc::now(),
        )
        .await;
    if let Some(fields) = metadata.as_object_mut() {
        fields.insert("pricing".into(), priced.provenance());
    }
    let trace_id: String = match session_id {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => format!("direct:{}", Uuid::new_v4()),
    };
    let mut tx = db.begin().await?;
    sqlx::query(
        r#"INSERT INTO trace_usage (
               trace_id, agent_name, session_id, user_id, model, provider,
               input_tokens, output_tokens, cache_read_tokens,
               latency_ms, started_at, source, priced_by, materialized_at, metadata,
               router_input_tokens, router_output_tokens, router_cache_read_tokens, cost_usd
           ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now(),'agent','router',now(),$11,$7,$8,$9,$12)
           ON CONFLICT (trace_id, agent_name) DO UPDATE SET
               -- Visible columns and the router share alike (0080): a Tempo-priced
               -- row keeps what Tempo counted.
               input_tokens             = trace_usage.input_tokens + EXCLUDED.input_tokens,
               output_tokens            = trace_usage.output_tokens + EXCLUDED.output_tokens,
               cache_read_tokens        = trace_usage.cache_read_tokens + EXCLUDED.cache_read_tokens,
               router_input_tokens      = trace_usage.router_input_tokens + EXCLUDED.router_input_tokens,
               router_output_tokens     = trace_usage.router_output_tokens + EXCLUDED.router_output_tokens,
               router_cache_read_tokens = trace_usage.router_cache_read_tokens + EXCLUDED.router_cache_read_tokens,
               model             = EXCLUDED.model,
               provider          = EXCLUDED.provider,
               user_id           = COALESCE(trace_usage.user_id, EXCLUDED.user_id),
               latency_ms        = COALESCE(trace_usage.latency_ms, 0) + EXCLUDED.latency_ms,
               priced_by         = 'router',
               materialized_at   = now(),
               cost_usd          = COALESCE(trace_usage.cost_usd, 0) + EXCLUDED.cost_usd,
               metadata          = trace_usage.metadata || EXCLUDED.metadata"#,
    )
    .bind(&trace_id)
    .bind(agent_name)
    .bind(session_id)
    .bind(user_id)
    .bind(&usage.model)
    .bind(provider)
    .bind(usage.input_tokens)
    .bind(usage.output_tokens)
    .bind(usage.cached_tokens)
    .bind(usage.latency_ms)
    .bind(&metadata)
    .bind(priced.cost.total_usd)
    .execute(&mut *tx)
    .await?;
    let entry = LedgerEntry {
        user_id,
        agent_id: None,
        operation_type: MAF_OPERATION,
        provider,
        model: &usage.model,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        cache_read_tokens: usage.cached_tokens,
        cache_creation_tokens: 0,
        cost_usd: Some(priced.cost.total_usd),
        finish_reason: None,
        metadata: &metadata,
    };
    ledger.record(&mut tx, &entry).await?;
    tx.commit().await
}

/// What a MAF call is recorded as on the usage ledger.
const MAF_OPERATION: &str = "maf_orchestration";

/// Every step as the planner sees it: this run's content, when it has any,
/// folded into step 0's task description. Lets one saved workflow shape (e.g.
/// "summarise this, then translate to japanese") be re-run against different
/// content each time instead of baking it in at creation. Every step past 0
/// still gets its input purely from prior steps' extracted context (see
/// `build_context`), unchanged.
pub fn planning_steps(definition: &MafDefinition, content: Option<&str>) -> Vec<MafStep> {
    definition
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
        .collect()
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
    agent_calls: &Arc<dyn AgentCallMeter>,
    targets: &dyn AgentTargetResolver,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
    content: Option<&str>,
    outbound: &dyn OutboundPrefix,
    clock: AgentClock,
) -> Result<StepOutcome, RunError> {
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
        agent_calls,
        targets,
        execution_id,
        user_id,
        maf_def,
        llm,
        content,
        outbound,
        clock,
    )
    .instrument(span)
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_maf_inner(
    client: &reqwest::Client,
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    agent_calls: &Arc<dyn AgentCallMeter>,
    targets: &dyn AgentTargetResolver,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
    llm: &LlmClient,
    content: Option<&str>,
    outbound: &dyn OutboundPrefix,
    clock: AgentClock,
) -> Result<StepOutcome, RunError> {
    // Resolved ONCE for the whole execution, not per step: a run is one unit of work, and a
    // change landing between step 2 and step 3 would otherwise leave a single execution governed
    // by two different prefixes — harder to reason about, and harder to audit, than either end.
    let outbound_prefix = outbound.resolve().await;
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

    register_execution_session(db, execution_id, user_id, maf_def).await;

    let planning = planning_steps(maf_def, content);

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
        plan_execution(&planning, llm, None),
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
            agent_calls,
            targets,
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
            outbound_prefix.as_deref(),
            clock,
        )
        .await
        {
            StepLoopOutcome::Advance(_) => {}
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
        outbound,
        outbound_prefix.as_deref(),
    )
    .await
}

/// Registers a run as a platform session (a `chat_sessions` row keyed by the execution id).
pub async fn register_execution_session(
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    maf_def: &MafDefinition,
) {
    // Register the execution as a platform session.
    //
    // `chat_sessions.session_id` is the A2A `contextId` — per its own schema
    // comment, "the platform-wide session key shared by chat history, agent
    // tasks, and observability". MAF already sends `execution_id` as the
    // contextId to every step agent, so the run IS a session; it just never
    // said so. `agent_proxy` upserts this row before forwarding any message,
    // and MAF has to as well, because `session_traces.session_id` is a
    // foreign key onto this table — without the row, the per-step trace index
    // `execute_step` writes cannot be written and the run's agent spend stays undiscoverable.
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
    agent_calls: &Arc<dyn AgentCallMeter>,
    targets: &dyn AgentTargetResolver,
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
    outbound: &dyn OutboundPrefix,
    clock: AgentClock,
) -> Result<StepOutcome, RunError> {
    // Re-resolved on resume rather than carried across the pause. A paused execution can sit for
    // days, and the steps that run after a human answers should run under the policy in force
    // when they actually run — the stored `step_results[i].prompt` deliberately holds the clean
    // prompt, never a prefixed copy, so there is nothing stale to carry.
    let outbound_prefix = outbound.resolve().await;
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
        )
        .into());
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
        )
        .into());
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
        agent_calls,
        targets,
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
        StepLoopOutcome::Advance(_) => {}
        StepLoopOutcome::AwaitingHuman(paused) => return Ok(StepOutcome::AwaitingHuman(paused)),
        StepLoopOutcome::Failed(e) => return Err(e),
    }

    for i in (resume_index + 1)..maf_def.steps.len() {
        let context = build_context(&step_results[..i]);
        match execute_step(
            client,
            db,
            flow_guard,
            agent_calls,
            targets,
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
            outbound_prefix.as_deref(),
            clock,
        )
        .await
        {
            StepLoopOutcome::Advance(_) => {}
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
        outbound,
        outbound_prefix.as_deref(),
    )
    .await
}

/// Where `step`'s agent call goes, asked of the resolver (which wakes a parked agent) rather than
/// read from `step.agent_endpoint`, a snapshot from when the workflow was saved and stale as soon
/// as the container is recreated. The snapshot is used only when the agent is routable but has no
/// endpoint of its own to give (an empty stored URL), the one way an externally registered agent
/// still works.
async fn step_endpoint(
    targets: &dyn AgentTargetResolver,
    step: &MafStep,
    user_id: Uuid,
    flow_id: &str,
    execution_id: Uuid,
) -> Result<ForwardTarget, ResolveFailure> {
    let caller = ResolveCaller {
        user_id: Some(user_id),
        flow_id: Some(flow_id.to_string()),
    };
    // The contextId a step's agent sees is the execution id (see `call_agent`'s caller).
    match targets
        .resolve(step.agent_id, &caller, Some(&execution_id.to_string()))
        .await
    {
        Ok(target) => Ok(target),
        Err(failure) if failure.status == ResolveFailure::NO_ENDPOINT => {
            Ok(ForwardTarget::from(step.agent_endpoint.clone()))
        }
        Err(failure) => Err(failure),
    }
}

/// The `flows.metadata` a step's flow row carries. `user_message` is the text the step's agent is
/// sent: the LLM router scores that, not the framework framing around it, when it picks a model
/// for the agent's calls under this flow.
fn step_flow_metadata(execution_id: Uuid, step_prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "context_id": execution_id.to_string(),
        "mode": "free_flowing",
        "user_message": step_prompt,
    })
}

/// Runs one step fresh: prompt generation (LLM call 2) → agent call → extraction (LLM call 3),
/// persisting progress at every transition exactly as the original inline loop body did. Shared by
/// `run_maf` and, for every step after the resumed one, `run_maf_from`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_step(
    client: &reqwest::Client,
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    agent_calls: &Arc<dyn AgentCallMeter>,
    targets: &dyn AgentTargetResolver,
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
    outbound_prefix: Option<&str>,
    // The current date and time, sent with the step's message after `outbound_prefix`.
    clock: AgentClock,
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
            let err = step_llm_failure(step.step_index, "prompt generation", &e);
            step_results[i].status = "failed".to_string();
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err.into());
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
    //
    // A retried step reuses its trace id (`build_traceparent` derives it from the execution and
    // the step), so the row its last attempt closed is already here. The gateway and the router
    // only honour a `running` row younger than the flow timeout, so that row is reopened, fresh,
    // with this attempt's prompt; leaving it closed refused every model and tool call the retried
    // step's agent made. Only the run's own user's row is reopened.
    let flow_metadata = step_flow_metadata(execution_id, &actual_prompt);
    let _ = sqlx::query(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_name, title, status, metadata, session_id)
           VALUES ($1, $2, $3, $4, 'running', $5, $6)
           ON CONFLICT (flow_id) DO UPDATE
              SET status = 'running', created_at = now(), completed_at = NULL,
                  duration_ms = NULL, error_message = NULL, metadata = EXCLUDED.metadata
            WHERE flows.user_id = EXCLUDED.user_id"#,
    )
    .bind(&trace_id)
    .bind(user_id)
    .bind(&step.agent_name)
    .bind(&step.task_description)
    .bind(&flow_metadata)
    // A MAF run has no chat session; the execution is the session (same key as `context_id`).
    .bind(execution_id.to_string())
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

    // The agent's address is not taken from the one frozen into `maf_json` when the workflow was
    // saved (`step.agent_endpoint`): that snapshot goes stale as soon as the container is
    // recreated (Docker hands out a fresh host port each time) or the agent is parked. It is
    // resolved below, after the cascade limits, through the platform's resolver; the snapshot is
    // only the fallback for an agent that is routable but has no address of its own.
    //
    // The A2A mount from the agent's card (`agents.transport_path`) is read here: it does not
    // depend on where the agent runs, and an agent mounted off root is called nowhere else. A
    // resolved URL that already ends with it is not mounted twice (`mounted_a2a_url`).
    let a2a_path: Option<String> =
        sqlx::query_scalar("SELECT transport_path FROM agents WHERE id = $1")
            .bind(step.agent_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .flatten();

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
        return StepLoopOutcome::Failed(err.into());
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
        return StepLoopOutcome::Failed(err.into());
    }

    // Resolved only now, past the cascade limits, so a step they refuse never wakes a parked agent.
    let endpoint = match step_endpoint(targets, step, user_id, &trace_id, execution_id).await {
        Ok(endpoint) => endpoint,
        Err(failure) => {
            // The invocation recorded above would otherwise never be unwound.
            flow_guard.record_return(&flow_ctx).await;
            let err = format!(
                "step {} (agent '{}'): {failure}",
                step.step_index, step.agent_name
            );
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err.into());
        }
    };

    // Org policy is appended as its own part inside `call_agent`, never composed back into
    // `actual_prompt`: that value is persisted to `step_results[i].prompt` below and read back
    // verbatim on resume, so folding the policy into it in place would put the policy in the
    // record — and then re-append it to the already-appended copy on every resume.
    let raw_response = timed(
        "call_agent",
        execution_id,
        Some(step.step_index),
        metered(
            agent_calls,
            step.agent_id,
            call_agent(
                client,
                &endpoint,
                a2a_path.as_deref(),
                &execution_id.to_string(),
                &user_id.to_string(),
                &actual_prompt,
                outbound_prefix,
                clock,
                &traceparent,
            ),
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
            let err = e.into_run_error(step, "failed");
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].error = Some(err.to_string());
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
pub async fn continue_paused_step(
    client: &reqwest::Client,
    db: &PgPool,
    flow_guard: &Arc<FlowGuard>,
    agent_calls: &Arc<dyn AgentCallMeter>,
    targets: &dyn AgentTargetResolver,
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

    // `execute_step` stamped `created_at` when the paused attempt called the agent, and the
    // pause closed the row. The MCP gateway's
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

    // Resolved now, not read from the saved definition: the human may have taken long enough
    // that the agent was parked meanwhile, and the snapshot is stale either way.
    let endpoint = match step_endpoint(targets, step, user_id, &trace_id, execution_id).await {
        Ok(endpoint) => endpoint,
        Err(failure) => {
            let err = format!(
                "step {} (agent '{}') resume failed: {failure}",
                step.step_index, step.agent_name
            );
            step_results[i].status = "failed".to_string();
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err.into());
        }
    };

    let call_result = metered(
        agent_calls,
        step.agent_id,
        call_agent_continuation(
            client,
            &endpoint,
            context_id,
            task_id,
            &user_id.to_string(),
            injected_answer,
            &traceparent,
        ),
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
            let err = e.into_run_error(step, "resume failed");
            step_results[i].status = "failed".to_string();
            step_results[i].error = Some(err.to_string());
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
            let err = step_llm_failure(step.step_index, "extraction", &e);
            step_results[i].status = "failed".to_string();
            step_results[i].prompt = actual_prompt;
            step_results[i].latency_ms = latency_ms;
            step_results[i].error = Some(err.clone());
            persist_progress(db, execution_id, step_results, *total_tokens, *total_cost).await;
            return StepLoopOutcome::Failed(err.into());
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

    StepLoopOutcome::Advance(raw_response)
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
    step_results: Vec<StepResult>,
    total_tokens: i64,
    total_cost: f64,
    output_generation: &str,
    llm: &LlmClient,
    outbound: &dyn OutboundPrefix,
    outbound_prefix: Option<&str>,
) -> Result<StepOutcome, RunError> {
    let context = build_context(&step_results);
    finish_run_with_context(
        db,
        execution_id,
        user_id,
        step_results,
        total_tokens,
        total_cost,
        output_generation,
        &context,
        llm,
        outbound,
        outbound_prefix,
    )
    .await
}

/// [`finish_run`] for a caller that assembles the synthesis context itself.
#[allow(clippy::too_many_arguments)]
pub async fn finish_run_with_context(
    db: &PgPool,
    execution_id: Uuid,
    user_id: Uuid,
    mut step_results: Vec<StepResult>,
    mut total_tokens: i64,
    total_cost: f64,
    output_generation: &str,
    context: &str,
    llm: &LlmClient,
    outbound: &dyn OutboundPrefix,
    outbound_prefix: Option<&str>,
) -> Result<StepOutcome, RunError> {
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
        generate_final_output(context, guidelines, llm, outbound, outbound_prefix),
    )
    .await
    .map_err(|e| final_output_failure(&e))?;
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
pub async fn persist_output_generation(db: &PgPool, execution_id: Uuid, output_generation: &str) {
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
/// disagree with `trace_usage` (a metering write that failed, a partially
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
        "SELECT COALESCE((metadata->>'step_index')::int,
                         (substring(agent_name from '^maf:step([0-9]+)$'))::int),
                COALESCE(SUM(input_tokens + output_tokens + cache_read_tokens), 0)::BIGINT,
                COALESCE(SUM(cost_usd), 0)::DOUBLE PRECISION
           FROM trace_usage
          WHERE trace_id = $1
            AND (agent_name = 'maf' OR agent_name LIKE 'maf:step%')
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
pub fn pending_result(step: &MafStep) -> StepResult {
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
        ext: serde_json::Map::new(),
    }
}

/// This execution's last persisted step snapshot, or an empty vec when there
/// is none (first attempt) or it can't be read.
///
/// An unreadable or malformed snapshot is not an error: `resume_from` treats
/// an empty prior as "start clean", which is exactly the old behaviour. A
/// retry that re-runs everything is wasteful, never wrong.
pub async fn load_prior_results(db: &PgPool, execution_id: Uuid) -> Vec<StepResult> {
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
///
/// A stopped execution is never written: the stop already recorded where it ended. Finding
/// the row stopped also stops the run here, the backstop for a stop message that never reached
/// this worker (`super::stop`), so the run ends at this step rather than running to completion.
pub async fn persist_progress(
    db: &PgPool,
    execution_id: Uuid,
    step_results: &[StepResult],
    tokens_used: i64,
    cost_usd: f64,
) {
    let step_json = serde_json::to_value(step_results).unwrap_or_default();
    let written = sqlx::query(
        "UPDATE maf_executions SET step_results = $1::jsonb, tokens_used = $2, cost_usd = $3 \
         WHERE id = $4 AND status <> $5",
    )
    .bind(step_json.to_string())
    .bind(tokens_used)
    .bind(cost_usd)
    .bind(execution_id)
    .bind(nasiko_types::maf::STOPPED)
    .execute(db)
    .await;
    if let Ok(result) = written
        && result.rows_affected() == 0
        && super::stop::stop_local(execution_id)
    {
        tracing::info!(
            execution_id = %execution_id,
            "maf: execution found stopped at its progress write; ending this run"
        );
    }
}

// ─── Step plan produced by plan_execution ────────────────────────────────────

pub struct StepPlan {
    pub prompt: String,
    pub to_extract: String,
}

/// What a step whose model call failed reads as on the run page. A provider refusal
/// (missing or refused key, error status) says the step couldn't run and why; anything
/// else keeps naming the phase that broke.
fn step_llm_failure(step_index: i32, phase: &str, error: &LlmError) -> String {
    if error.is_provider_refusal() {
        format!("Step {step_index} couldn't run: {error}")
    } else {
        format!("step {step_index}: {phase} failed: {error}")
    }
}

/// [`step_llm_failure`] for the final synthesis, which belongs to no step.
fn final_output_failure(error: &LlmError) -> String {
    if error.is_provider_refusal() {
        format!("The final answer couldn't be written: {error}")
    } else {
        format!("final output generation failed: {error}")
    }
}

// ─── LLM call 1: runtime planner ─────────────────────────────────────────────

/// `planner_note`, when given, is appended to the step list as extra guidance for the planner.
pub async fn plan_execution(
    steps: &[MafStep],
    llm: &LlmClient,
    planner_note: Option<&str>,
) -> Result<((Vec<StepPlan>, String), LlmUsage), LlmError> {
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

    let user = match planner_note {
        Some(note) => format!("Steps:\n{step_list}\n\n{note}"),
        None => format!("Steps:\n{step_list}"),
    };

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
        .ok_or_else(|| LlmError::Other("planner returned no 'steps' array".into()))?;

    if plans_json.len() != steps.len() {
        return Err(LlmError::Other(format!(
            "planner returned {} step plans but MAF has {} steps",
            plans_json.len(),
            steps.len()
        )));
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
        .collect::<Result<Vec<_>, String>>()
        .map_err(LlmError::Other)?;

    Ok(((plans, output_generation), usage))
}

// ─── LLM call 2: prompt generator ────────────────────────────────────────────

async fn generate_step_prompt(
    template: &str,
    task_description: &str,
    context: &str,
    llm: &LlmClient,
) -> Result<(String, LlmUsage), LlmError> {
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
        .ok_or_else(|| {
            LlmError::Other("LLM prompt generation returned no 'prompt' field".into())
        })?;

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
) -> Result<(String, LlmUsage), LlmError> {
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
        .ok_or_else(|| {
            LlmError::Other("LLM extraction returned no 'extracted_info' field".into())
        })?;

    Ok((extracted, usage))
}

// ─── LLM call 4: final output synthesiser ────────────────────────────────────

async fn generate_final_output(
    context: &str,
    guidelines: &str,
    llm: &LlmClient,
    outbound: &dyn OutboundPrefix,
    outbound_prefix: Option<&str>,
) -> Result<(String, LlmUsage), LlmError> {
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

    // This is the one LLM on the MAF path whose output the USER reads. Every step's agent gets
    // the prefix on its own prompt, but those agents only feed this synthesiser, which is then
    // free to put anything it was handed into the final answer — so a rule like "never repeat a
    // customer email address" is obeyed by every agent and then broken here, by the platform
    // itself, unless the same text governs this call too. Prepended to the system prompt rather
    // than the user turn so it also precedes the one-shot exchange below.
    //
    // With nothing configured `compose` returns `system` unchanged, so the prompt stays
    // byte-identical to Python's `MAFExecutor._create_final_output`.
    let system = outbound.compose(outbound_prefix, system);

    let (json, usage) = llm
        .chat_json_schema(
            vec![
                ChatMessage::system(&system),
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
        .ok_or_else(|| {
            LlmError::Other("LLM final output returned no 'final_output' field".into())
        })?;

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

/// Why a call to a step's agent produced no reply.
#[derive(Debug, PartialEq)]
enum AgentCallError {
    /// No answer within the agent-call budget: the worker's agent client carries
    /// `AGENT_CALL_TIMEOUT_SECS` as its total timeout, streamed reply included. Holds how long the
    /// call had run.
    TimedOut(std::time::Duration),
    /// Anything else: the agent was unreachable, refused the call, broke off its stream, or
    /// reported its task failed.
    Failed(String),
}

impl AgentCallError {
    fn from_http(err: reqwest::Error, started: Instant) -> Self {
        if err.is_timeout() {
            Self::TimedOut(started.elapsed())
        } else {
            Self::Failed(err.to_string())
        }
    }

    /// The run's error for this failure, worded for `step` of a call `verb` ("failed", "resume
    /// failed"). A timeout names its own reason, so it reads the same wherever the call was.
    fn into_run_error(self, step: &MafStep, verb: &str) -> RunError {
        match self {
            Self::TimedOut(after) => RunError::TimedOut(format!(
                "step {} (agent '{}') timed out after {}s",
                step.step_index,
                step.agent_name,
                after.as_secs()
            )),
            Self::Failed(e) => RunError::Failed(format!(
                "step {} (agent '{}') {verb}: {e}",
                step.step_index, step.agent_name
            )),
        }
    }
}

/// The A2A method-name and message dialects an agent may speak. MAF's agents are proto-style
/// (`SendStreamingMessage`, `role: "ROLE_USER"`, untagged `{"text": …}` parts — a real `a2a-lf`
/// agent rejects an unexpected `"kind"` field outright), so that is tried first; an agent that
/// does not know it (`-32601`) is asked in the JSON-RPC dialect (`message/stream`, typed parts),
/// the order reversed from the chat orchestrator's (`nasiko_react_agent::a2a`), whose agents
/// lean the other way.
#[derive(Clone, Copy)]
enum Dialect {
    Proto,
    JsonRpc,
}

/// A fresh message to an agent: `texts` as its parts, the prompt first.
fn message_request(
    dialect: Dialect,
    streaming: bool,
    texts: &[&str],
    context_id: &str,
) -> serde_json::Value {
    let (method, role) = match (dialect, streaming) {
        (Dialect::Proto, true) => ("SendStreamingMessage", "ROLE_USER"),
        (Dialect::Proto, false) => ("SendMessage", "ROLE_USER"),
        (Dialect::JsonRpc, true) => ("message/stream", "user"),
        (Dialect::JsonRpc, false) => ("message/send", "user"),
    };
    let parts: Vec<serde_json::Value> = texts
        .iter()
        .map(|text| match dialect {
            Dialect::Proto => serde_json::json!({ "text": text }),
            Dialect::JsonRpc => serde_json::json!({ "kind": "text", "text": text }),
        })
        .collect();
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "1",
        "method": method,
        "params": {
            "message": {
                "messageId": uuid::Uuid::new_v4().to_string(),
                "contextId": context_id,
                "role": role,
                "parts": parts
            }
        }
    })
}

/// Where to send a call: the agent's address, its A2A mount, and who and what the call is for.
struct AgentCallTarget<'a> {
    endpoint: &'a ForwardTarget,
    /// The agent's A2A mount from its card (`agents.transport_path`); see [`send_to_agent`].
    a2a_path: Option<&'a str>,
    user_id: &'a str,
    traceparent: &'a str,
}

/// Calls the step's agent the way the chat orchestrator calls one (`nasiko_react_agent::tool`):
/// streaming first, reading the agent's events as they arrive, and asking again without
/// streaming only when the agent never started on the streamed request. A call cut off after it
/// started is never sent again, since the agent may already have acted on it.
#[allow(clippy::too_many_arguments)]
async fn call_agent(
    client: &reqwest::Client,
    endpoint: &ForwardTarget,
    // The agent's A2A mount from its card (`agents.transport_path`); see `send_to_agent`.
    a2a_path: Option<&str>,
    context_id: &str,
    user_id: &str,
    prompt: &str,
    // Organization policy (L0), appended as its own part — never spliced into `prompt`, which is
    // persisted verbatim to `step_results[i].prompt` and read back on resume (see this function's
    // caller). Same representation `a2a_dispatch.rs`/`agent_proxy.rs`/`tool.rs` use for the exact
    // same reason: a second, independently-addressable part rather than a merged string.
    outbound_prefix: Option<&str>,
    // The current date and time, its own part after the policy. A new step's message only: the
    // human's answer on a resume (`call_agent_continuation`) goes out alone, like the policy.
    clock: AgentClock,
    traceparent: &str,
) -> Result<AgentCallOutcome, AgentCallError> {
    let target = AgentCallTarget {
        endpoint,
        a2a_path,
        user_id,
        traceparent,
    };
    let now = clock.note();
    let mut texts = vec![prompt];
    texts.extend(outbound_prefix);
    texts.extend(now.as_deref());
    let started = Instant::now();

    for dialect in [Dialect::Proto, Dialect::JsonRpc] {
        let request = message_request(dialect, true, &texts, context_id);
        match stream_call(client, &target, &request, context_id, started).await {
            StreamAttempt::Answered(outcome) => return outcome,
            StreamAttempt::MethodNotFound => continue,
            StreamAttempt::NotStarted => break,
        }
    }
    let request = message_request(Dialect::Proto, false, &texts, context_id);
    unary_call(client, &target, &request, context_id, started).await
}

/// Resumes a specific paused task with the human's answer — streaming first, as [`call_agent`],
/// with the same requests the direct-chat HITL resume dispatcher sends
/// (`nasiko_types::a2a::build_stream_request_for_task`, then `build_send_request_for_task`).
/// Unlike `call_agent`, this always targets an existing `taskId`, never starts a new one.
#[allow(clippy::too_many_arguments)]
async fn call_agent_continuation(
    client: &reqwest::Client,
    endpoint: &ForwardTarget,
    context_id: &str,
    task_id: &str,
    user_id: &str,
    answer: &str,
    traceparent: &str,
) -> Result<AgentCallOutcome, AgentCallError> {
    // The resume path holds only the step's stored endpoint, not the agent's resolved mount.
    let target = AgentCallTarget {
        endpoint,
        a2a_path: None,
        user_id,
        traceparent,
    };
    let started = Instant::now();
    let request = build_stream_request_for_task(answer, context_id, task_id);
    match stream_call(client, &target, &request, task_id, started).await {
        StreamAttempt::Answered(outcome) => return outcome,
        StreamAttempt::MethodNotFound | StreamAttempt::NotStarted => {}
    }
    let request = build_send_request_for_task(answer, context_id, task_id);
    unary_call(client, &target, &request, task_id, started).await
}

/// `{base}{path}` for an agent mounted at a non-root path, `None` for a root mount or no path.
/// A `base` that already ends with the mount is returned as is: the server's resolver hands back
/// the message URL with the mount appended, while the saved snapshot and the catalog address come
/// without it, and appending a second time is a 404 (`/a2a/jsonrpc/a2a/jsonrpc`).
fn mounted_a2a_url(base: &str, a2a_path: Option<&str>) -> Option<String> {
    let path = a2a_path
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "/")?;
    let sep = if path.starts_with('/') { "" } else { "/" };
    if base.ends_with(&format!("{sep}{}", path.trim_end_matches('/'))) {
        return Some(base.to_string());
    }
    Some(format!("{base}{sep}{path}"))
}

/// Attaches whatever the resolver asked to have sent with every call to this target (the EE
/// injection proxy's own credential when memory is enabled; empty otherwise).
fn with_target_headers(
    mut request: reqwest::RequestBuilder,
    target: &ForwardTarget,
) -> reqwest::RequestBuilder {
    for (name, value) in &target.headers {
        request = request.header(name, value);
    }
    request
}

/// Posts `body` to the agent: at its A2A mount when its card names one, otherwise at `/jsonrpc`,
/// falling back to `/` on a 404 (some agents expose A2A at one, some at the other). Returns the
/// response whatever its status; the callers decide what a status means.
async fn send_to_agent<T: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    target: &AgentCallTarget<'_>,
    body: &T,
    streaming: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    // An agent whose card names a non-root A2A mount (OpenCode serves only `/a2a/jsonrpc`) is
    // called there and nowhere else, exactly as direct chat calls it.
    let base = target.endpoint.url.trim_end_matches('/');
    let mounted = mounted_a2a_url(base, target.a2a_path);
    let url_jsonrpc = mounted.clone().unwrap_or_else(|| format!("{base}/jsonrpc"));
    // No trailing slash: the request line is `/` either way, and a bare `/` appended to the
    // injection proxy's address matches none of its routes.
    let url_root = base.to_string();

    // No per-request MCP credential: the agent authenticates to `/api/mcp` with its own
    // deploy-time MCP_GATEWAY_TOKEN, and the user binding rides the forwarded traceparent + the
    // flow_participants record written before this call (docs/MCP_GATEWAY_AGENT_AUTH.md).
    // No per-request timeout: `client` is the worker's agent client, built with the platform's
    // agent-call budget because agent A2A calls are all it makes.
    let post = |url: &str| {
        let request = with_target_headers(client.post(url), target.endpoint)
            .header("X-User-Id", target.user_id)
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("traceparent", target.traceparent);
        let request = if streaming {
            request.header("Accept", "text/event-stream")
        } else {
            request
        };
        request.json(body).send()
    };
    let response = post(&url_jsonrpc).await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND && mounted.is_none() {
        return post(&url_root).await;
    }
    Ok(response)
}

/// What one streamed attempt at a call came to.
enum StreamAttempt {
    /// The agent answered — a reply, a pause, or a failure: the call is over.
    Answered(Result<AgentCallOutcome, AgentCallError>),
    /// The agent does not know this streaming method (`-32601`); another dialect may work.
    MethodNotFound,
    /// The agent never started on the request (it refused its form with a 4xx, answered with
    /// something that is not A2A, or said `-32004`: streaming unsupported), so asking again
    /// without streaming repeats nothing.
    NotStarted,
}

async fn stream_call<T: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    target: &AgentCallTarget<'_>,
    body: &T,
    fallback_task_id: &str,
    started: Instant,
) -> StreamAttempt {
    let response = match send_to_agent(client, target, body, true).await {
        Ok(response) => response,
        Err(e) => return StreamAttempt::Answered(Err(AgentCallError::from_http(e, started))),
    };
    // A 4xx refuses the request's form (no such route, method or media type): another form may
    // be accepted. A 5xx is the agent failing while it worked on it, and it may already have
    // acted, so it is not asked again. Here MAF parts from the chat orchestrator, which retries
    // on any HTTP status.
    let status = response.status();
    if status.is_client_error() {
        return StreamAttempt::NotStarted;
    }
    if !status.is_success() {
        return StreamAttempt::Answered(Err(AgentCallError::Failed(format!("HTTP {status}"))));
    }
    let is_event_stream = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    if is_event_stream {
        return StreamAttempt::Answered(
            read_event_stream(response, fallback_task_id, started).await,
        );
    }

    // Answered in one piece, as an agent that does not stream does.
    let json: serde_json::Value = match response.json().await {
        Ok(json) => json,
        Err(e) if e.is_timeout() => {
            return StreamAttempt::Answered(Err(AgentCallError::TimedOut(started.elapsed())));
        }
        Err(_) => return StreamAttempt::NotStarted,
    };
    match json["error"]["code"].as_i64() {
        Some(-32601) => StreamAttempt::MethodNotFound,
        Some(-32004) => StreamAttempt::NotStarted,
        _ => StreamAttempt::Answered(read_reply(json, fallback_task_id)),
    }
}

/// One request, answered in one piece: the non-streaming fallback.
async fn unary_call<T: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    target: &AgentCallTarget<'_>,
    body: &T,
    fallback_task_id: &str,
    started: Instant,
) -> Result<AgentCallOutcome, AgentCallError> {
    let response = send_to_agent(client, target, body, false)
        .await
        .map_err(|e| AgentCallError::from_http(e, started))?;
    if !response.status().is_success() {
        return Err(AgentCallError::Failed(format!(
            "HTTP {}",
            response.status()
        )));
    }
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AgentCallError::from_http(e, started))?;
    read_reply(json, fallback_task_id)
}

/// Reads an agent's event stream until an event ends the call: the task completed, failed, or
/// needs a human. Reply text arrives in pieces and is gathered as it comes; the agent's own
/// progress narration is not kept, as nothing shows a step's progress live. A stream that just
/// ends counts as complete with whatever text it carried, as in the chat orchestrator.
async fn read_event_stream(
    response: reqwest::Response,
    fallback_task_id: &str,
    started: Instant,
) -> Result<AgentCallOutcome, AgentCallError> {
    use futures::StreamExt as _;

    let mut collected = String::new();
    // Bytes, decoded per complete line: a multibyte character split across chunks must not be
    // decoded half at a time.
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AgentCallError::from_http(e, started))?;
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim_end_matches(['\n', '\r']).strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(outcome) = read_event(&event, data, &mut collected, fallback_task_id)? {
                return Ok(outcome);
            }
        }
    }
    Ok(AgentCallOutcome::Completed(collected))
}

/// One event of an agent's stream: `Some` when it ends the call.
fn read_event(
    event: &serde_json::Value,
    data: &str,
    collected: &mut String,
    fallback_task_id: &str,
) -> Result<Option<AgentCallOutcome>, AgentCallError> {
    // How the a2a SDKs end a stream whose task raised. Skipped, it would leave no text and read
    // as an empty success.
    if let Some(error) = event.get("error") {
        let message = error["message"]
            .as_str()
            .unwrap_or("the agent's stream ended in an error");
        return Err(AgentCallError::Failed(format!("A2A error: {message}")));
    }
    for item in classify_sse_event(event) {
        match item {
            SseEvent::ArtifactText(text) => collected.push_str(&text),
            SseEvent::StatusText(_) | SseEvent::StatusData(_) => {}
            SseEvent::Completed { snapshot_text } => {
                if collected.is_empty() {
                    *collected = snapshot_text.unwrap_or_else(|| extract_text(event));
                }
                return Ok(Some(AgentCallOutcome::Completed(std::mem::take(collected))));
            }
            SseEvent::Failed { reason } => {
                return Err(AgentCallError::Failed(task_failed(&reason)));
            }
            SseEvent::AwaitingHuman { .. } => {
                return Ok(Some(AgentCallOutcome::Paused {
                    task_id: nasiko_types::a2a::paused_task_id(data, fallback_task_id),
                    raw_data: data.to_string(),
                }));
            }
        }
    }
    Ok(None)
}

/// Classifies a reply answered in one piece into a normal reply or a pause, via the same
/// wire-shape parsing `classify_stream_disposition` uses for the streaming direct-chat path.
/// `fallback_task_id` is used only if the payload carries no task id of its own (§`paused_task_id`).
fn read_reply(
    json: serde_json::Value,
    fallback_task_id: &str,
) -> Result<AgentCallOutcome, AgentCallError> {
    if let Some(error) = json["error"]["message"].as_str() {
        return Err(AgentCallError::Failed(format!("A2A error: {error}")));
    }
    let data = json.to_string();
    match nasiko_types::a2a::classify_stream_disposition(&data) {
        StreamDisposition::Paused => {
            let task_id = nasiko_types::a2a::paused_task_id(&data, fallback_task_id);
            Ok(AgentCallOutcome::Paused {
                task_id,
                raw_data: data,
            })
        }
        // An agent that reports its task failed has no answer to extract: passing its error text
        // on as a reply would mark the step a success and feed the error to every later step.
        StreamDisposition::Failed => Err(AgentCallError::Failed(task_failed(&extract_text(&json)))),
        StreamDisposition::Completed | StreamDisposition::Continue => {
            Ok(AgentCallOutcome::Completed(extract_text(&json)))
        }
    }
}

fn task_failed(reason: &str) -> String {
    if reason.is_empty() {
        "agent reported the task failed".to_string()
    } else {
        format!("agent reported the task failed: {reason}")
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
pub fn build_context(step_results: &[StepResult]) -> String {
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
    use crate::maf::llm::KeySource;

    #[test]
    fn a_step_refused_by_the_provider_says_it_couldnt_run_and_why() {
        let rejected = LlmError::KeyRejected(KeySource::Owner);
        assert_eq!(
            step_llm_failure(2, "prompt generation", &rejected),
            "Step 2 couldn't run: OpenAI rejected your saved API key. \
             Update it in LLM router, then try again."
        );
        let malformed = LlmError::Other("LLM returned no choices".into());
        assert_eq!(
            step_llm_failure(2, "extraction", &malformed),
            "step 2: extraction failed: LLM returned no choices"
        );
        assert!(final_output_failure(&LlmError::NoKey).starts_with("The final answer couldn't"));
    }

    #[test]
    fn an_agent_mounted_below_root_is_called_at_its_mount() {
        let base = "http://10.0.0.5:8080";
        assert_eq!(
            mounted_a2a_url(base, Some("/a2a/jsonrpc")).as_deref(),
            Some("http://10.0.0.5:8080/a2a/jsonrpc")
        );
        assert_eq!(
            mounted_a2a_url(base, Some("a2a")).as_deref(),
            Some("http://10.0.0.5:8080/a2a")
        );
    }

    #[test]
    fn a_root_or_missing_mount_keeps_the_jsonrpc_then_root_fallback() {
        let base = "http://10.0.0.5:8080";
        assert_eq!(mounted_a2a_url(base, None), None);
        assert_eq!(mounted_a2a_url(base, Some("/")), None);
        assert_eq!(mounted_a2a_url(base, Some("")), None);
    }

    fn task_snapshot(
        state: &str,
        status_text: &str,
        artifact_text: Option<&str>,
    ) -> serde_json::Value {
        let mut task = serde_json::json!({
            "id": "t1",
            "contextId": "c1",
            "status": {"state": state, "message": {"parts": [{"text": status_text}]}},
        });
        if let Some(text) = artifact_text {
            task["artifacts"] = serde_json::json!([{"parts": [{"text": text}]}]);
        }
        serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": {"task": task}})
    }

    #[test]
    fn an_agent_reported_failure_fails_the_step() {
        let failed = task_snapshot("TASK_STATE_FAILED", "Error: Error code: 502", None);
        match read_reply(failed, "c1") {
            Err(AgentCallError::Failed(reason)) => {
                assert!(reason.contains("Error code: 502"), "got: {reason}")
            }
            Err(other) => panic!("a failed task is a failure, not {other:?}"),
            Ok(_) => panic!("a failed task must not be classified as a reply"),
        }
    }

    #[test]
    fn a_completed_task_is_a_reply() {
        let completed = task_snapshot("TASK_STATE_COMPLETED", "done", Some("Hola"));
        match read_reply(completed, "c1") {
            Ok(AgentCallOutcome::Completed(text)) => assert_eq!(text, "Hola"),
            _ => panic!("a completed task must be classified as a reply"),
        }
    }

    /// Q5-1: the server's resolver returns the URL with the mount already on it; a second copy
    /// (`/a2a/jsonrpc/a2a/jsonrpc`) is a 404 from OpenCode.
    #[test]
    fn a_url_that_already_carries_the_mount_is_not_mounted_twice() {
        let resolved = "http://10.0.0.5:8080/a2a/jsonrpc";
        assert_eq!(
            mounted_a2a_url(resolved, Some("/a2a/jsonrpc")).as_deref(),
            Some(resolved)
        );
        assert_eq!(
            mounted_a2a_url("http://10.0.0.5:8080/a2a", Some("/a2a/")).as_deref(),
            Some("http://10.0.0.5:8080/a2a")
        );
    }

    /// NAS-1256: the router scores a step's flow by `user_message`, so the step row must carry
    /// the text the step's agent is sent.
    #[test]
    fn a_step_flow_row_carries_the_step_prompt_as_its_user_message() {
        let execution = Uuid::new_v4();
        let metadata = step_flow_metadata(execution, "summarise the Q3 report");
        assert_eq!(metadata["user_message"], "summarise the Q3 report");
        assert_eq!(metadata["context_id"], execution.to_string());
        assert_eq!(metadata["mode"], "free_flowing");
    }

    fn step(index: i32) -> MafStep {
        MafStep {
            step_id: Uuid::new_v4(),
            step_index: index,
            agent_id: Uuid::new_v4(),
            agent_name: format!("agent-{index}"),
            agent_endpoint: "http://localhost:1".to_string(),
            task_description: format!("task {index}"),
            ext: serde_json::Map::new(),
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
    fn every_step_is_planned_and_content_goes_to_step_0() {
        let definition = MafDefinition {
            description: None,
            steps: vec![step(0), step(1)],
            output_generation: None,
        };
        let planned = planning_steps(&definition, Some("refund me"));
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].task_description, "task 0 \"refund me\"");
        assert_eq!(planned[1].task_description, "task 1");
        assert_eq!(
            planning_steps(&definition, None)[0].task_description,
            "task 0"
        );
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
            nasiko_redis::SharedRedis::new(
                redis::Client::open("redis://127.0.0.1:1").expect("lazy client, never dialled"),
            ),
            nasiko_flow::FlowConfig::default(),
        ));
        let llm = LlmClient::new(reqwest::Client::new(), String::new(), None, String::new());
        let agent_calls: Arc<dyn AgentCallMeter> = Arc::new(RecordingMeter::default());

        let result = run_maf_from(
            &client,
            &db,
            &flow_guard,
            &agent_calls,
            &crate::target::DirectAddress::new(db.clone()),
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
            &crate::outbound::NoOutboundPrefix,
            AgentClock::OFF,
        )
        .await;

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("a length mismatch must be rejected, not silently succeed"),
        };
        assert!(
            err.to_string().contains("does not match"),
            "error should name the actual mismatch: {err}"
        );
    }

    /// Per call: the agent, the result it was told (if any), and whether its handle was dropped.
    type RecordedCalls = Arc<std::sync::Mutex<Vec<(Uuid, Option<bool>, bool)>>>;

    /// Records what a step's agent call reports through the metering seam.
    #[derive(Default)]
    struct RecordingMeter {
        calls: RecordedCalls,
    }

    struct RecordingHandle {
        calls: RecordedCalls,
        index: usize,
    }

    impl AgentCallMeter for RecordingMeter {
        fn begin(&self, agent_id: Uuid) -> Box<dyn nasiko_types::agent_call::AgentCallHandle> {
            let mut calls = self.calls.lock().unwrap();
            calls.push((agent_id, None, false));
            Box::new(RecordingHandle {
                calls: self.calls.clone(),
                index: calls.len() - 1,
            })
        }
    }

    impl nasiko_types::agent_call::AgentCallHandle for RecordingHandle {
        fn set_succeeded(&mut self, succeeded: bool) {
            self.calls.lock().unwrap()[self.index].1 = Some(succeeded);
        }
    }

    impl Drop for RecordingHandle {
        fn drop(&mut self) {
            self.calls.lock().unwrap()[self.index].2 = true;
        }
    }

    #[tokio::test]
    async fn a_metered_call_is_in_flight_only_while_it_runs_and_reports_its_result() {
        let meter = RecordingMeter::default();
        let calls = meter.calls.clone();
        let meter: Arc<dyn AgentCallMeter> = Arc::new(meter);
        let agent = Uuid::new_v4();

        let observed = metered(&meter, agent, async {
            // Mid-call: begun, no result yet, still held.
            Ok::<_, String>(calls.lock().unwrap().clone())
        })
        .await
        .unwrap();
        assert_eq!(observed, vec![(agent, None, false)]);
        assert_eq!(calls.lock().unwrap()[0], (agent, Some(true), true));

        let failed = metered(&meter, agent, async {
            Err::<(), _>("HTTP 502".to_string())
        })
        .await;
        assert!(failed.is_err());
        assert_eq!(calls.lock().unwrap()[1], (agent, Some(false), true));
    }

    /// An `OutboundPrefix` that supplies a recognisable prefix and joins it the way the real
    /// server adapter does (`nasiko_react_agent::prefixed`, which this crate cannot reach).
    #[derive(Debug)]
    struct FixedPrefix(&'static str);

    #[async_trait::async_trait]
    impl crate::outbound::OutboundPrefix for FixedPrefix {
        async fn resolve(&self) -> Option<String> {
            Some(self.0.to_string())
        }

        /// Delegates to the real join rather than re-spelling it. A hand-rolled copy here made
        /// the assertion below self-fulfilling: it would have kept passing if
        /// `nasiko_react_agent::prefixed` changed, which is exactly the drift
        /// `OutboundPrefix::compose` exists to prevent.
        fn compose(&self, prefix: Option<&str>, body: &str) -> String {
            nasiko_react_agent::prefixed(prefix, body)
        }
    }

    /// The MAF synthesiser (LLM call 4) is the only model on that path whose output the USER
    /// reads. Every step's agent is handed the prefix on its own prompt, but those agents just
    /// feed this call, which is then free to put anything it was given into the final answer — so
    /// a rule like "never repeat a customer email address" is honoured by every agent and then
    /// broken by the platform itself. Asserted on the wire, against the SYSTEM message, because
    /// that is the only place the guarantee actually lives.
    #[tokio::test]
    async fn the_final_output_synthesiser_is_governed_by_the_outbound_prefix() {
        let mut llm_api = mockito::Server::new_async().await;
        let hit = llm_api
            .mock("POST", "/chat/completions")
            // Keys come out alphabetical (`serde_json` without `preserve_order`), so the
            // message serialises as `{"content":...,"role":...}`. Anchoring on the content that
            // STARTS with the prefix and still carries the synthesiser's own opening line pins
            // the prefix to the system prompt specifically, not merely somewhere in the body.
            .match_body(mockito::Matcher::Regex(
                r#""content":"ORG POLICY[\s\S]*You are a Multi-Agent Flow"#.into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"choices":[{"message":{"content":"{\"final_output\":\"done\"}"}}],"usage":{"total_tokens":7}}"#,
            )
            .create_async()
            .await;

        let llm = LlmClient::new(
            reqwest::Client::new(),
            "k".into(),
            Some(llm_api.url()),
            "test-model".into(),
        );
        let prefix = FixedPrefix("ORG POLICY");
        let (out, _) = generate_final_output("", "guidelines", &llm, &prefix, Some("ORG POLICY"))
            .await
            .expect("synthesiser call");

        hit.assert_async().await;
        assert_eq!(out, "done");
    }

    /// And with nothing configured the system prompt must go out byte-identical to Python's
    /// `MAFExecutor._create_final_output` — the parity this file's own comment claims.
    #[tokio::test]
    async fn an_unconfigured_deployment_leaves_the_synthesiser_prompt_untouched() {
        let mut llm_api = mockito::Server::new_async().await;
        let hit = llm_api
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(
                r#""content":"You are a Multi-Agent Flow"#.into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"choices":[{"message":{"content":"{\"final_output\":\"done\"}"}}],"usage":{"total_tokens":7}}"#,
            )
            .create_async()
            .await;

        let llm = LlmClient::new(
            reqwest::Client::new(),
            "k".into(),
            Some(llm_api.url()),
            "test-model".into(),
        );
        let (_, _) = generate_final_output(
            "",
            "guidelines",
            &llm,
            &crate::outbound::NoOutboundPrefix,
            None,
        )
        .await
        .expect("synthesiser call");

        hit.assert_async().await;
    }

    /// Organization policy, appended as its own `parts[]` entry — never composed into the prompt
    /// text itself, which is what `step_results[i].prompt` persists and a resume reads back
    /// verbatim (see `execute_step`'s call site: this must never leak the policy into that
    /// record). Untagged (no `"kind"` field): `call_agent` always builds a proto-style
    /// `"SendMessage"` request, and this platform's real `a2a-lf` agents reject an unexpected
    /// `"kind"` outright.
    #[tokio::test]
    async fn call_agent_appends_org_policy_as_its_own_part() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/jsonrpc")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "params": {"message": {"parts": [
                    {"text": "do the thing"},
                    {"text": "POLICY"}
                ]}}
            })))
            .match_request(|request| forwarded_part_count(request) == 2)
            .with_status(200)
            .with_body(completed_agent_response())
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let result = call_agent(
            &client,
            &ForwardTarget::from(server.url()),
            None,
            "ctx-1",
            "user-1",
            "do the thing",
            Some("POLICY"),
            AgentClock::OFF,
            "00-trace-span-01",
        )
        .await;

        mock.assert_async().await;
        assert!(matches!(result, Ok(AgentCallOutcome::Completed(_))));
    }

    /// The current date and time follows the policy as its own part. The step's prompt stays the
    /// first part, unchanged: it is what `step_results[i].prompt` records.
    #[tokio::test]
    async fn call_agent_tells_the_time_after_the_policy() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/jsonrpc")
            .match_request(|request| tells_the_time_after(request, "do the thing"))
            .with_status(200)
            .with_body(completed_agent_response())
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let result = call_agent(
            &client,
            &ForwardTarget::from(server.url()),
            None,
            "ctx-1",
            "user-1",
            "do the thing",
            Some("POLICY"),
            AgentClock::ON,
            "00-trace-span-01",
        )
        .await;

        mock.assert_async().await;
        assert!(matches!(result, Ok(AgentCallOutcome::Completed(_))));
    }

    /// HD-02: OpenCode serves A2A only at `/a2a/jsonrpc`. MAF must call that mount, and not
    /// probe `/jsonrpc` or `/` first.
    #[tokio::test]
    async fn call_agent_uses_the_agents_a2a_mount() {
        let mut server = mockito::Server::new_async().await;
        let mounted = server
            .mock("POST", "/a2a/jsonrpc")
            .with_status(200)
            .with_body(completed_agent_response())
            .create_async()
            .await;
        let jsonrpc = server
            .mock("POST", "/jsonrpc")
            .expect(0)
            .create_async()
            .await;
        let root = server.mock("POST", "/").expect(0).create_async().await;

        let client = reqwest::Client::new();
        let result = call_agent(
            &client,
            &ForwardTarget::from(server.url()),
            Some("/a2a/jsonrpc"),
            "ctx-1",
            "user-1",
            "do the thing",
            None,
            AgentClock::OFF,
            "00-trace-span-01",
        )
        .await;

        mounted.assert_async().await;
        jsonrpc.assert_async().await;
        root.assert_async().await;
        assert!(matches!(result, Ok(AgentCallOutcome::Completed(_))));
    }

    /// Q5-1: a resolver-sourced target already ends with the mount and is called at exactly that
    /// URL, never at the mount appended a second time.
    #[tokio::test]
    async fn call_agent_does_not_mount_a_resolved_url_twice() {
        let mut server = mockito::Server::new_async().await;
        let mounted = server
            .mock("POST", "/a2a/jsonrpc")
            .with_status(200)
            .with_body(completed_agent_response())
            .create_async()
            .await;
        let doubled = server
            .mock("POST", "/a2a/jsonrpc/a2a/jsonrpc")
            .expect(0)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let result = call_agent(
            &client,
            &ForwardTarget::from(format!("{}/a2a/jsonrpc", server.url())),
            Some("/a2a/jsonrpc"),
            "ctx-1",
            "user-1",
            "do the thing",
            None,
            AgentClock::OFF,
            "00-trace-span-01",
        )
        .await;

        mounted.assert_async().await;
        doubled.assert_async().await;
        assert!(matches!(result, Ok(AgentCallOutcome::Completed(_))));
    }

    /// The ordinary case, alongside it: no policy configured must send exactly one part, the
    /// prompt's own text, unchanged.
    #[tokio::test]
    async fn call_agent_sends_only_the_prompt_when_no_policy_is_configured() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/jsonrpc")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "params": {"message": {"parts": [{"text": "do the thing"}]}}
            })))
            .match_request(|request| forwarded_part_count(request) == 1)
            .with_status(200)
            .with_body(completed_agent_response())
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let result = call_agent(
            &client,
            &ForwardTarget::from(server.url()),
            None,
            "ctx-1",
            "user-1",
            "do the thing",
            None,
            AgentClock::OFF,
            "00-trace-span-01",
        )
        .await;

        mock.assert_async().await;
        assert!(matches!(result, Ok(AgentCallOutcome::Completed(_))));
    }

    /// How many message parts a forwarded request carried. `mockito::Matcher::PartialJson` accepts
    /// extra trailing array elements, so on its own it cannot say "and nothing else was sent"; a
    /// test that claims an exact list pairs it with this.
    /// The text of each part of the forwarded message, in order.
    fn forwarded_texts(request: &mockito::Request) -> Vec<String> {
        request
            .body()
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(raw).ok())
            .and_then(|body| body.pointer("/params/message/parts").cloned())
            .and_then(|parts| parts.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|part| part["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The model's message, then the policy, then the current time, each its own part.
    fn tells_the_time_after(request: &mockito::Request, first: &str) -> bool {
        let texts = forwarded_texts(request);
        texts.len() == 3
            && texts[0] == first
            && texts[1] == "POLICY"
            && texts[2].starts_with(nasiko_types::agent_clock::NOTE_LEAD)
    }

    fn forwarded_part_count(request: &mockito::Request) -> usize {
        request
            .body()
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(raw).ok())
            .and_then(|body| {
                body.pointer("/params/message/parts")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0)
    }

    fn completed_agent_response() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]}
        })
        .to_string()
    }

    /// One recorded ask: agent, user, flow, contextId.
    type Asked = (Uuid, Option<Uuid>, Option<String>, Option<String>);

    /// A fake resolver answering with a fixed result and recording who it was asked about.
    struct FakeTargets {
        answer: Result<String, ResolveFailure>,
        asked: std::sync::Mutex<Vec<Asked>>,
    }

    impl FakeTargets {
        fn answering(answer: Result<String, ResolveFailure>) -> Self {
            Self {
                answer,
                asked: Default::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentTargetResolver for FakeTargets {
        async fn resolve(
            &self,
            agent_id: Uuid,
            caller: &ResolveCaller,
            context_id: Option<&str>,
        ) -> Result<crate::target::ForwardTarget, ResolveFailure> {
            self.asked.lock().unwrap().push((
                agent_id,
                caller.user_id,
                caller.flow_id.clone(),
                context_id.map(str::to_owned),
            ));
            self.answer.clone().map(crate::target::ForwardTarget::from)
        }
    }

    #[tokio::test]
    async fn a_step_is_sent_to_the_url_the_resolver_returns() {
        let targets = FakeTargets::answering(Ok("http://woken.sandbox:8000".into()));
        let step = step(0);
        let (user, execution) = (Uuid::new_v4(), Uuid::new_v4());

        let url = step_endpoint(&targets, &step, user, "flow-1", execution)
            .await
            .unwrap();

        assert_eq!(url.url, "http://woken.sandbox:8000");
        assert_eq!(
            targets.asked.lock().unwrap().as_slice(),
            [(
                step.agent_id,
                Some(user),
                Some("flow-1".to_string()),
                Some(execution.to_string())
            )]
        );
    }

    #[tokio::test]
    async fn the_saved_snapshot_is_used_only_when_the_agent_has_no_endpoint_of_its_own() {
        let step = step(0);
        let none = FakeTargets::answering(Err(ResolveFailure::new(
            ResolveFailure::NO_ENDPOINT,
            "no stored endpoint",
        )));
        assert_eq!(
            step_endpoint(&none, &step, Uuid::new_v4(), "f", Uuid::new_v4())
                .await
                .unwrap()
                .url,
            step.agent_endpoint
        );

        let harness = FakeTargets::answering(Err(ResolveFailure::new(
            ResolveFailure::HARNESS,
            "MAF cannot drive a harness session yet",
        )));
        let err = step_endpoint(&harness, &step, Uuid::new_v4(), "f", Uuid::new_v4())
            .await
            .unwrap_err();
        assert_eq!(err.status, 501);
    }

    // ─── Agent calls: streaming first, the chat orchestrator's fallbacks, the timeout ────────

    fn sse(events: &[serde_json::Value]) -> String {
        events.iter().map(|e| format!("data: {e}\n\n")).collect()
    }

    fn artifact_event(text: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": {"artifactUpdate": {
            "taskId": "task-9", "artifact": {"parts": [{"text": text}]}
        }}})
    }

    fn status_event(state: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": "1", "result": {"statusUpdate": {
            "taskId": "task-9", "contextId": "ctx-1",
            "status": {"state": state, "message": {"parts": [{"text": text}]}}
        }}})
    }

    fn method_not_found() -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": "1",
            "error": {"code": -32601, "message": "Method not found"}})
        .to_string()
    }

    fn method(name: &str) -> mockito::Matcher {
        mockito::Matcher::PartialJson(serde_json::json!({ "method": name }))
    }

    async fn call(
        client: &reqwest::Client,
        url: String,
    ) -> Result<AgentCallOutcome, AgentCallError> {
        call_agent(
            client,
            &ForwardTarget::from(url),
            None,
            "ctx-1",
            "user-1",
            "do the thing",
            None,
            AgentClock::OFF,
            "00-trace-span-01",
        )
        .await
    }

    #[tokio::test]
    async fn a_streamed_reply_is_gathered_from_its_events() {
        let mut server = mockito::Server::new_async().await;
        let stream = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendStreamingMessage"))
            .match_header("accept", "text/event-stream")
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[
                status_event("TASK_STATE_WORKING", "thinking"),
                artifact_event("Hel"),
                artifact_event("lo"),
                status_event("TASK_STATE_COMPLETED", ""),
            ]))
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        stream.assert_async().await;
        assert_eq!(result, Ok(AgentCallOutcome::Completed("Hello".into())));
    }

    #[tokio::test]
    async fn a_stream_that_asks_for_input_pauses_the_step_on_the_agents_task() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/jsonrpc")
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[status_event(
                "TASK_STATE_INPUT_REQUIRED",
                "Which customer?",
            )]))
            .create_async()
            .await;

        match call(&reqwest::Client::new(), server.url()).await {
            Ok(AgentCallOutcome::Paused { task_id, raw_data }) => {
                assert_eq!(task_id, "task-9");
                assert!(
                    nasiko_types::a2a::build_pause_question(&raw_data)
                        .to_string()
                        .contains("Which customer?"),
                    "the question reaches the HITL request: {raw_data}"
                );
            }
            other => panic!("expected a pause, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_stream_reporting_its_task_failed_fails_the_step() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/jsonrpc")
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[status_event(
                "TASK_STATE_FAILED",
                "model call failed",
            )]))
            .create_async()
            .await;

        match call(&reqwest::Client::new(), server.url()).await {
            Err(AgentCallError::Failed(reason)) => {
                assert!(reason.contains("model call failed"), "{reason}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_agent_without_the_proto_streaming_method_is_asked_in_the_json_rpc_dialect() {
        let mut server = mockito::Server::new_async().await;
        let proto = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendStreamingMessage"))
            .with_body(method_not_found())
            .create_async()
            .await;
        let json_rpc = server
            .mock("POST", "/jsonrpc")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "method": "message/stream",
                "params": {"message": {"role": "user", "parts": [{"kind": "text", "text": "do the thing"}]}}
            })))
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[
                artifact_event("done"),
                status_event("TASK_STATE_COMPLETED", ""),
            ]))
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        proto.assert_async().await;
        json_rpc.assert_async().await;
        assert_eq!(result, Ok(AgentCallOutcome::Completed("done".into())));
    }

    #[tokio::test]
    async fn an_agent_that_does_not_stream_is_asked_once_without_streaming() {
        let mut server = mockito::Server::new_async().await;
        let streams = server
            .mock("POST", "/jsonrpc")
            .match_body(mockito::Matcher::Any)
            .match_request(|request| {
                let body =
                    String::from_utf8_lossy(request.body().unwrap_or(&Vec::new())).to_string();
                body.contains("\"SendStreamingMessage\"") || body.contains("\"message/stream\"")
            })
            .with_body(method_not_found())
            .expect(2)
            .create_async()
            .await;
        let unary = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendMessage"))
            .with_body(completed_agent_response())
            .expect(1)
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        streams.assert_async().await;
        unary.assert_async().await;
        assert!(
            matches!(result, Ok(AgentCallOutcome::Completed(_))),
            "{result:?}"
        );
    }

    /// The agent may already have acted on a request it started: one cut off part-way is a
    /// failed step, never sent again.
    #[tokio::test]
    async fn a_stream_cut_off_part_way_fails_without_being_sent_again() {
        let mut server = mockito::Server::new_async().await;
        let stream = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendStreamingMessage"))
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[
                artifact_event("Sent the first email"),
                serde_json::json!({"jsonrpc": "2.0", "id": "1",
                    "error": {"code": -32603, "message": "worker crashed"}}),
            ]))
            .expect(1)
            .create_async()
            .await;
        let unary = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendMessage"))
            .expect(0)
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        stream.assert_async().await;
        unary.assert_async().await;
        match result {
            Err(AgentCallError::Failed(reason)) => {
                assert!(reason.contains("worker crashed"), "{reason}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// A 5xx is the agent failing while it worked: asked once, never a second time.
    #[tokio::test]
    async fn an_agent_failing_with_a_server_error_is_not_asked_again() {
        let mut server = mockito::Server::new_async().await;
        let failing = server
            .mock("POST", "/jsonrpc")
            .with_status(500)
            .expect(1)
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        failing.assert_async().await;
        assert!(
            matches!(&result, Err(AgentCallError::Failed(reason)) if reason.contains("500")),
            "{result:?}"
        );
    }

    /// A 4xx refuses the streamed request's form, so the agent is asked again without streaming.
    #[tokio::test]
    async fn an_agent_refusing_the_streamed_request_is_asked_without_streaming() {
        let mut server = mockito::Server::new_async().await;
        let refused = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendStreamingMessage"))
            .with_status(415)
            .expect(1)
            .create_async()
            .await;
        let unary = server
            .mock("POST", "/jsonrpc")
            .match_body(method("SendMessage"))
            .with_body(completed_agent_response())
            .expect(1)
            .create_async()
            .await;

        let result = call(&reqwest::Client::new(), server.url()).await;

        refused.assert_async().await;
        unary.assert_async().await;
        assert!(
            matches!(result, Ok(AgentCallOutcome::Completed(_))),
            "{result:?}"
        );
    }

    /// An agent that accepts the call and never answers: the worker's agent client carries the
    /// agent-call budget, and running out of it is a timeout, not an ordinary failure.
    #[tokio::test]
    async fn an_agent_that_never_answers_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let held = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            drop(socket);
        });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();

        let result = call(&client, url).await;

        held.abort();
        assert!(
            matches!(result, Err(AgentCallError::TimedOut(_))),
            "{result:?}"
        );
    }

    #[test]
    fn a_timed_out_call_ends_the_run_without_a_retry() {
        let step = MafStep {
            agent_name: "task-breakdown".to_string(),
            ..step(2)
        };
        let timed_out = AgentCallError::TimedOut(std::time::Duration::from_secs(600))
            .into_run_error(&step, "failed");
        assert_eq!(
            timed_out,
            RunError::TimedOut("step 2 (agent 'task-breakdown') timed out after 600s".into())
        );
        assert!(!timed_out.is_retryable());

        let failed =
            AgentCallError::Failed("HTTP 502 Bad Gateway".into()).into_run_error(&step, "failed");
        assert_eq!(
            failed,
            RunError::Failed("step 2 (agent 'task-breakdown') failed: HTTP 502 Bad Gateway".into())
        );
        assert!(failed.is_retryable());
    }

    #[tokio::test]
    async fn a_resume_streams_the_answer_to_the_paused_task() {
        let mut server = mockito::Server::new_async().await;
        let resume = server
            .mock("POST", "/jsonrpc")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "method": "SendStreamingMessage",
                "params": {"message": {"taskId": "task-9", "contextId": "ctx-1"}}
            })))
            .with_header("content-type", "text/event-stream")
            .with_body(sse(&[
                artifact_event("Sent to Acme"),
                status_event("TASK_STATE_COMPLETED", ""),
            ]))
            .create_async()
            .await;

        let result = call_agent_continuation(
            &reqwest::Client::new(),
            &ForwardTarget::from(server.url()),
            "ctx-1",
            "task-9",
            "user-1",
            "Acme",
            "00-trace-span-01",
        )
        .await;

        resume.assert_async().await;
        assert_eq!(
            result,
            Ok(AgentCallOutcome::Completed("Sent to Acme".into()))
        );
    }
}
