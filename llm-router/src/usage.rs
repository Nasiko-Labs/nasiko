//! Fire-and-forget usage logging to the `trace_usage` spend table.
//!
//! Adds each LLM call to its `(trace_id, agent_name)` row, priced here in Rust
//! through the platform's single cost engine, and hands the call to the host's
//! [`UsageLedger`] on the same transaction. Failures are logged and swallowed — usage logging must
//! never break or delay the response.
//!
//! Missing exact prices fall back through the shared engine. Pricing provenance
//! records inferred rates so estimates are distinguishable from matched prices.
//!
//! A call that reached the provider and failed is a call too (NAS-1018): it adds to the row's
//! `error_count`, prices only the usage the provider reported, and when it reported none, books
//! an estimated prompt in `router_estimated_input_tokens` and flags the row `cost_estimated`.

use std::sync::{Arc, LazyLock};

use chrono::Utc;
use nasiko_pricing::{LedgerEntry, PricingEngine, PromptConvention, RawUsage, UsageLedger};
use sqlx::PgPool;
use uuid::Uuid;

use crate::ir::Usage;
use crate::routing::attribution::AttributionSource;

/// One usage row to write.
pub struct UsageRecord {
    /// The billed identity: the chatting user (`flows.user_id`) when the call
    /// was attributed to a flow, else the agent owner from the JWT.
    pub owner_id: String,
    pub agent_id: String,
    /// `trace_usage.metadata.operation_type`, e.g. `"direct_llm"` (chat) or `"embedding"`.
    pub operation_type: &'static str,
    pub provider: String,
    /// Bare provider-native model id (no prefix).
    pub model: String,
    pub usage: Option<Usage>,
    pub cached_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub latency_ms: i64,
    pub streaming: bool,
    pub finish_reason: Option<String>,
    /// The flow this call belongs to — named by the agent-forwarded
    /// `traceparent` (strict attribution rejects calls without one, so served
    /// calls always carry it). Written to `trace_usage.trace_id`/`session_id` — the same
    /// key the orchestrator uses — so per-message usage aggregates across the
    /// platform and its agents.
    pub flow_id: Option<String>,
    /// How `flow_id` was resolved; recorded in the row's metadata so
    /// attribution quality is auditable.
    pub attribution_source: Option<AttributionSource>,
    /// Whether the platform's key paid for this call (vs. the owner's own secret).
    pub platform_paid: bool,
    /// Pre-serialized `metadata.compress` block, or `None` when compression did not run.
    ///
    /// A `Value` rather than a typed struct so this module stays a pure DB concern and does not
    /// depend on the compression module's types. `None` leaves the row's metadata byte-identical
    /// to what it was before compression existed.
    pub compress_metadata: Option<serde_json::Value>,
    /// Pre-serialized `metadata.brevity` block (IP-2).
    ///
    /// Always `Some` once the layer exists, because "skipped, and why" is the answer most worth
    /// having: IP-1 leaves a row only when it acted, so a missing block is ambiguous between
    /// "off" and "nothing to do". This one always says which.
    pub brevity_metadata: Option<serde_json::Value>,
    /// Pre-serialized `metadata.compact_tools` block (`compact_tools::to_metadata`): what this call
    /// did with compact schemas and the tool-definition bytes it sent. Unlike the blocks above it
    /// is added up across a flow's calls, not overwritten (see [`write_trace_usage`]). `None` on
    /// a call that did not come through chat (embeddings, `/v1/responses`, a failed attempt).
    pub compact_tools_metadata: Option<serde_json::Value>,
    /// Pre-serialized `metadata.routing` block (`RouteDecision::to_metadata`): flat counters for
    /// the precedence level that chose this call's model, the tier it drew, and whether that
    /// overrode the resolver's model (NAS-1263). `None` on a call that never routed by model
    /// (embeddings), which is why the block's absence is meaningful and its zeros are not.
    pub routing_metadata: Option<serde_json::Value>,
    /// `(bytes_in, bytes_out)` from IP-1 when it actually reduced the payload, for the
    /// `token_savings` ledger. Two integers rather than the compression module's type, so this
    /// module stays a pure DB concern.
    ///
    /// The ledger write lives here, beside the `trace_usage` write, because pricing the saving
    /// needs the same `PricingEngine` result that prices the call — computing it anywhere else
    /// would price a saving at a different rate than the spend it is subtracted from, and the
    /// reduction percentage would stop being coherent.
    pub compress_bytes: Option<(usize, usize)>,
    /// `(native, compact)` tool-definition bytes when this call was served by a compact-tools
    /// request whose reply decoded. `None` when compaction was skipped, or when the reply did not
    /// decode and the native request was re-sent: that call carried the full schemas.
    pub compact_tools_bytes: Option<(usize, usize)>,
    /// Total text bytes actually sent, after compression and the brevity directive. Calibrates
    /// chars-per-token against this very call rather than a fixed divisor (see `savings.rs`).
    pub request_bytes: Option<usize>,
    /// Set when the call reached the provider and failed. The row still counts the call: it
    /// adds to `error_count`, and when the provider reported no usage its prompt is estimated
    /// from `request_bytes` (never priced, see [`estimated_prompt_tokens`]).
    pub failure: Option<CallFailure>,
}

/// Why a call that reached the provider failed, as its usage row records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallFailure {
    /// `status` (the provider answered with an error), `transport`, `parse`, `timeout`
    /// (the router's deadline cancelled the call), `stream` (it broke after streaming began)
    /// or `response_failed` (the provider reported the response as failed).
    pub class: &'static str,
    /// The provider's HTTP status, when it answered with one.
    pub http_status: Option<u16>,
}

impl CallFailure {
    pub const fn new(class: &'static str, http_status: Option<u16>) -> Self {
        Self { class, http_status }
    }

    /// The provider answered with a 4xx other than 408 (timeout) or 429 (rate limit): it turned
    /// the request away without running the model, so nothing was billed.
    fn was_refused(self) -> bool {
        self.class == "status"
            && self
                .http_status
                .is_some_and(|code| (400..500).contains(&code) && code != 408 && code != 429)
    }
}

/// The context every call of one request shares, so each failed attempt of a fallback chain can
/// be written as its own row without repeating the request's identity at every call site.
pub struct CallScope {
    pub owner_id: String,
    pub agent_id: String,
    pub operation_type: &'static str,
    pub streaming: bool,
    pub flow_id: Option<String>,
    pub attribution_source: Option<AttributionSource>,
    pub request_bytes: Option<usize>,
    /// The request's `metadata.routing` block: the model was chosen once, so every attempt of the
    /// fallback chain was routed the same way and counts the same.
    pub routing_metadata: Option<serde_json::Value>,
}

/// One provider attempt that reached the provider and failed.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedAttempt {
    pub provider: String,
    pub model: String,
    pub platform_paid: bool,
    pub latency_ms: i64,
    pub failure: CallFailure,
}

impl UsageRecord {
    /// The row for one failed attempt: no usage reported (an error answer carries none), no
    /// compression or savings figures (nothing was delivered to save on).
    pub fn failed_attempt(scope: &CallScope, attempt: FailedAttempt) -> Self {
        Self {
            owner_id: scope.owner_id.clone(),
            agent_id: scope.agent_id.clone(),
            operation_type: scope.operation_type,
            provider: attempt.provider,
            model: attempt.model,
            usage: None,
            cached_tokens: None,
            reasoning_tokens: None,
            latency_ms: attempt.latency_ms,
            streaming: scope.streaming,
            finish_reason: Some(format!("failed:{}", attempt.failure.class)),
            flow_id: scope.flow_id.clone(),
            attribution_source: scope.attribution_source,
            platform_paid: attempt.platform_paid,
            compress_metadata: None,
            brevity_metadata: None,
            compact_tools_metadata: None,
            routing_metadata: scope.routing_metadata.clone(),
            compress_bytes: None,
            compact_tools_bytes: None,
            request_bytes: scope.request_bytes,
            failure: Some(attempt.failure),
        }
    }
}

/// Spawn one usage write per failed attempt. A request whose fallback then succeeded writes
/// its success row separately and unchanged; each failed attempt is a call of its own.
pub fn spawn_log_failed(
    db: &PgPool,
    pricing: &Arc<PricingEngine>,
    ledger: &Arc<dyn UsageLedger>,
    scope: &CallScope,
    attempts: Vec<FailedAttempt>,
) {
    for attempt in attempts {
        spawn_log(
            db.clone(),
            pricing.clone(),
            ledger.clone(),
            UsageRecord::failed_attempt(scope, attempt),
        );
    }
}

/// The prompt tokens a failed call most likely sent, estimated from the bytes sent at the
/// fallback chars-per-token ratio (`savings::chars_per_token`, which has no reported count to
/// calibrate against here). `0` for a call that succeeded, reported usage, has no byte count, or
/// was refused (see [`CallFailure::was_refused`]).
fn estimated_prompt_tokens(
    failure: Option<CallFailure>,
    usage: Option<&Usage>,
    request_bytes: Option<usize>,
) -> i64 {
    if failure.is_none_or(CallFailure::was_refused) || usage.is_some() {
        return 0;
    }
    let Some(bytes) = request_bytes else {
        return 0;
    };
    let (ratio, _estimated) = crate::savings::chars_per_token(crate::savings::CalibrationInputs {
        request_bytes: Some(bytes),
        input_tokens: None,
    });
    (bytes as f64 / ratio).floor() as i64
}

/// Spawn the usage write so it never blocks the response.
pub fn spawn_log(
    db: PgPool,
    pricing: Arc<PricingEngine>,
    ledger: Arc<dyn UsageLedger>,
    record: UsageRecord,
) {
    tokio::spawn(async move {
        if let Err(e) = log_usage(db, pricing.as_ref(), ledger.as_ref(), record).await {
            tracing::warn!(error = %e, "llm_usage write failed (swallowed)");
        }
    });
}

/// Add one priced call to its `trace_usage` row, and hand it to `ledger` on the same
/// transaction.
pub async fn log_usage(
    db: PgPool,
    pricing: &PricingEngine,
    ledger: &dyn UsageLedger,
    record: UsageRecord,
) -> Result<(), String> {
    // trace_usage.user_id and token_savings.user_id are FKs to users(id); without a valid owner we
    // cannot write a row, so skip (best-effort logging must never surface an error).
    let Ok(owner) = Uuid::parse_str(&record.owner_id) else {
        tracing::debug!(owner_id = %record.owner_id, "skipping usage row: owner_id is not a uuid");
        return Ok(());
    };
    // A nil owner (system calls: MCP tool indexing, unattributed builds) has no `users` row, so
    // the FK insert would fail; skip it explicitly instead of failing the transaction.
    if owner.is_nil() {
        tracing::debug!(agent_id = %record.agent_id, "skipping usage row: system call has no owner");
        return Ok(());
    }
    let agent = Uuid::parse_str(&record.agent_id).ok();
    // A non-UUID agent id is a platform component label (`maf`, `titling`, ...): keep it as
    // the row's `agent_name` instead of collapsing every component into one empty name.
    let component_label: Option<String> = component_label(agent, &record.agent_id);
    let estimated_input_tokens =
        estimated_prompt_tokens(record.failure, record.usage.as_ref(), record.request_bytes);
    let reported_usage = record.usage.is_some();
    let cache_details = record.usage.as_ref().and_then(|u| u.cache_creation.clone());
    let (input, output, total, cache_read, cache_creation) = match record.usage {
        Some(mut u) => {
            // Lift OpenAI's nested prompt_tokens_details.cached_tokens into the
            // flat cache_read field (Anthropic already sets it directly).
            u.normalize_openai_details();
            (
                u.prompt_tokens,
                u.completion_tokens,
                u.total_tokens,
                u.cache_read_input_tokens,
                u.cache_creation_input_tokens,
            )
        }
        None => (None, None, None, None, None),
    };

    // `normalize_openai_details` above has already made `prompt_tokens` disjoint
    // from the cache counts (it subtracts for OpenAI's nested block; Anthropic
    // reports them disjoint already, and the Gemini adapter subtracts at its own
    // mapping site). So the prompt count reaching the engine is fresh, and
    // declaring the convention here keeps that decision in one place rather than
    // re-deriving the provider's semantics a second time.
    let priced = pricing
        .price_with_context(
            Some(&record.provider),
            &record.model,
            RawUsage {
                input: input.unwrap_or(0).max(0) as u64,
                output: output.unwrap_or(0).max(0) as u64,
                cache_read: cache_read.unwrap_or(0).max(0) as u64,
                cache_creation: cache_creation.unwrap_or(0).max(0) as u64,
                total: total.map(|t| t.max(0) as u64),
            },
            PromptConvention::Exclusive,
            Utc::now(),
            nasiko_pricing::PricingContext {
                cache_creation_5m: cache_details
                    .as_ref()
                    .and_then(|c| c.ephemeral_5m_input_tokens)
                    .and_then(|n| u64::try_from(n).ok()),
                cache_creation_1h: cache_details
                    .as_ref()
                    .and_then(|c| c.ephemeral_1h_input_tokens)
                    .and_then(|n| u64::try_from(n).ok()),
                ..Default::default()
            },
        )
        .await;

    // Captured before the insert below consumes the record's owned fields.
    let record_ids = SavingsIds {
        owner,
        agent,
        flow_id: record.flow_id.clone(),
        provider: record.provider.clone(),
        model: record.model.clone(),
        compress_bytes: record.compress_bytes,
        compact_tools_bytes: record.compact_tools_bytes,
        request_bytes: record.request_bytes,
    };

    let metadata: serde_json::Value = build_metadata(MetadataInputs {
        operation_type: record.operation_type,
        platform_paid: record.platform_paid,
        attribution_source: record.attribution_source,
        pricing: priced.provenance(),
        cache_creation: serde_json::to_value(&cache_details).unwrap_or(serde_json::Value::Null),
        compress: record.compress_metadata,
        brevity: record.brevity_metadata,
        compact_tools: record.compact_tools_metadata,
        routing: record.routing_metadata,
        failure: record.failure.map(|failure| FailureMetadata {
            failure,
            reported_usage,
            estimated_input_tokens,
        }),
    });

    let live: LiveSpend = LiveSpend {
        metadata,
        owner,
        agent,
        component_label,
        flow_id: record.flow_id.clone(),
        model: record.model.clone(),
        provider: record.provider.clone(),
        input_tokens: input.unwrap_or(0).max(0),
        output_tokens: output.unwrap_or(0).max(0),
        cache_read_tokens: cache_read.unwrap_or(0).max(0),
        cache_creation_tokens: cache_creation.unwrap_or(0).max(0),
        cost_usd: priced.cost.total_usd,
        prompt_cost_usd: priced.cost.input_usd
            + priced.cost.cache_read_usd
            + priced.cost.cache_creation_usd,
        completion_cost_usd: priced.cost.output_usd,
        latency_ms: record.latency_ms,
        error_count: i32::from(record.failure.is_some()),
        estimated_input_tokens,
        price_estimated: priced.cost.estimated,
    };

    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    write_trace_usage(&mut tx, &live)
        .await
        .map_err(|e| e.to_string())?;
    let entry = LedgerEntry {
        user_id: owner,
        agent_id: agent,
        operation_type: record.operation_type,
        provider: &live.provider,
        model: &live.model,
        input_tokens: live.input_tokens,
        output_tokens: live.output_tokens,
        total_tokens: total.unwrap_or(live.input_tokens + live.output_tokens),
        cache_read_tokens: live.cache_read_tokens,
        cache_creation_tokens: live.cache_creation_tokens,
        cost_usd: Some(live.cost_usd),
        finish_reason: record.finish_reason.as_deref(),
        metadata: &live.metadata,
    };
    ledger
        .record(&mut tx, &entry)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    write_savings(
        &db,
        &record_ids,
        &priced.cost,
        input,
        cache_read,
        cache_creation,
    )
    .await;
    Ok(())
}

/// One priced LLM call, added live to its `trace_usage` row so budgets read
/// spend with no materializer lag.
struct LiveSpend {
    /// Per-call metadata, merged into `trace_usage.metadata` (a later success wins per key,
    /// except `compact_tools`, whose counters add up; a failure adds only the keys the row
    /// lacks, plus its `failure`).
    metadata: serde_json::Value,
    owner: Uuid,
    agent: Option<Uuid>,
    /// `agent_name` for a platform component call (no agent uuid).
    component_label: Option<String>,
    flow_id: Option<String>,
    model: String,
    provider: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd: f64,
    prompt_cost_usd: f64,
    completion_cost_usd: f64,
    latency_ms: i64,
    /// `1` for a failed call, else `0`.
    error_count: i32,
    /// A failed call's prompt estimate when the provider reported no usage; kept out of the
    /// spend columns.
    estimated_input_tokens: i64,
    /// The call was priced at an inferred rate (a family, static, ratio or default price).
    price_estimated: bool,
}

/// `trace_usage.source` for an agent row: ACP containers, local coding agents
/// (`nasiko connect`), or regular agents.
fn source_for(workload_kind: Option<&str>, is_coding: bool) -> &'static str {
    match (workload_kind, is_coding) {
        (Some("acp_coding_agent"), _) => "acp",
        (_, true) => "coding",
        _ => "agent",
    }
}

/// The component label for a call whose agent id is not a uuid; `None` for agent calls
/// and blank ids.
fn component_label(agent: Option<Uuid>, agent_id: &str) -> Option<String> {
    let id: &str = agent_id.trim();
    (agent.is_none() && !id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod component_label_tests {
    use super::component_label;
    use uuid::Uuid;

    #[test]
    fn label_kept_for_component() {
        assert_eq!(component_label(None, "titling").as_deref(), Some("titling"));
    }

    #[test]
    fn no_label_for_agent_or_blank() {
        assert_eq!(component_label(Some(Uuid::new_v4()), "x"), None);
        assert_eq!(component_label(None, "  "), None);
    }
}

/// Key for a call: the flow's trace id, or `direct:<fresh uuid v4>` when the
/// call carried no `traceparent` (local coding agents) — one row per such call.
fn trace_key(flow_id: Option<&str>) -> String {
    match flow_id {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => format!("direct:{}", Uuid::new_v4()),
    }
}

/// The one statement every priced call is added to its trace's row with — built once, because
/// its two summed-block fragments are generated ([`summed_block`]).
static UPSERT: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"INSERT INTO trace_usage (
               trace_id, agent_name, session_id, agent_id, user_id, model, provider,
               input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
               cost_usd, prompt_cost_usd, completion_cost_usd,
               router_input_tokens, router_output_tokens, router_cache_read_tokens,
               router_cache_creation_tokens, router_cost_usd, router_prompt_cost_usd,
               router_completion_cost_usd,
               latency_ms, started_at, source, priced_by, materialized_at, metadata,
               error_count, router_error_count, router_estimated_input_tokens, cost_estimated
           ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,
                     $8,$9,$10,$11,$12,$13,$14,
                     $15,now(),$16,'router',now(),$17,
                     -- A success leaves error_count NULL: "not yet counted", which the
                     -- materializer fills; only a failure is evidence on its own.
                     NULLIF($18, 0), $18, $19,
                     -- An unreported prompt means the cost may be understated; an
                     -- inferred rate means it may be wrong either way.
                     CASE WHEN $19 > 0 OR $20 THEN true END)
           ON CONFLICT (trace_id, agent_name) DO UPDATE SET
               error_count                  = CASE WHEN EXCLUDED.router_error_count > 0
                                                   THEN COALESCE(trace_usage.error_count, 0) + EXCLUDED.router_error_count
                                                   ELSE trace_usage.error_count END,
               router_error_count           = trace_usage.router_error_count + EXCLUDED.router_error_count,
               router_estimated_input_tokens = trace_usage.router_estimated_input_tokens + EXCLUDED.router_estimated_input_tokens,
               cost_estimated               = CASE WHEN EXCLUDED.router_estimated_input_tokens > 0 OR $20
                                                   THEN true ELSE trace_usage.cost_estimated END,
               input_tokens                 = trace_usage.input_tokens + EXCLUDED.input_tokens,
               output_tokens                = trace_usage.output_tokens + EXCLUDED.output_tokens,
               cache_read_tokens            = trace_usage.cache_read_tokens + EXCLUDED.cache_read_tokens,
               cache_creation_tokens        = trace_usage.cache_creation_tokens + EXCLUDED.cache_creation_tokens,
               cost_usd                     = COALESCE(trace_usage.cost_usd, 0) + EXCLUDED.cost_usd,
               prompt_cost_usd              = COALESCE(trace_usage.prompt_cost_usd, 0) + EXCLUDED.prompt_cost_usd,
               completion_cost_usd          = COALESCE(trace_usage.completion_cost_usd, 0) + EXCLUDED.completion_cost_usd,
               router_input_tokens          = trace_usage.router_input_tokens + EXCLUDED.router_input_tokens,
               router_output_tokens         = trace_usage.router_output_tokens + EXCLUDED.router_output_tokens,
               router_cache_read_tokens     = trace_usage.router_cache_read_tokens + EXCLUDED.router_cache_read_tokens,
               router_cache_creation_tokens = trace_usage.router_cache_creation_tokens + EXCLUDED.router_cache_creation_tokens,
               router_cost_usd              = trace_usage.router_cost_usd + EXCLUDED.router_cost_usd,
               router_prompt_cost_usd       = trace_usage.router_prompt_cost_usd + EXCLUDED.router_prompt_cost_usd,
               router_completion_cost_usd   = trace_usage.router_completion_cost_usd + EXCLUDED.router_completion_cost_usd,
               -- A failed attempt never names the row: the model that answered keeps it,
               -- whichever of the two writes lands last.
               model                        = CASE WHEN EXCLUDED.router_error_count > 0
                                                   THEN COALESCE(trace_usage.model, EXCLUDED.model) ELSE EXCLUDED.model END,
               provider                     = CASE WHEN EXCLUDED.router_error_count > 0
                                                   THEN COALESCE(trace_usage.provider, EXCLUDED.provider) ELSE EXCLUDED.provider END,
               agent_id                     = COALESCE(trace_usage.agent_id, EXCLUDED.agent_id),
               user_id                      = COALESCE(trace_usage.user_id, EXCLUDED.user_id),
               latency_ms                   = COALESCE(trace_usage.latency_ms, 0) + EXCLUDED.latency_ms,
               source                       = CASE WHEN EXCLUDED.source = 'agent' THEN trace_usage.source ELSE EXCLUDED.source END,
               priced_by                    = 'router',
               materialized_at              = now(),
               -- A failure keeps the answering call's keys (its `pricing` above all) and
               -- records itself as the trace's latest `failure`; failures carry no
               -- `compact_tools` block. A success wins per key, except the summed blocks
               -- (`compact_tools`, `routing`): their flat counters are added key by key, so a flow
               -- that compacted its first call and skipped the rest keeps both, and one that
               -- classified its first call and cached the rest keeps both levels.
               metadata                     = CASE WHEN EXCLUDED.router_error_count > 0
                   THEN EXCLUDED.metadata || trace_usage.metadata
                        || jsonb_build_object('failure', EXCLUDED.metadata->'failure')
                   ELSE trace_usage.metadata || EXCLUDED.metadata
                   || {compact_tools}
                   || {routing}
                   END"#,
        compact_tools = summed_block("compact_tools"),
        routing = summed_block("routing"),
    )
});

/// Add two rows' copies of one flat-integer metadata block, key by key.
///
/// `||` merges jsonb later-value-wins, which is right for a block that describes the trace's latest
/// call (`pricing`, `compress`) and wrong for one that counts calls: `compact_tools` counts bytes
/// and applications, `routing` counts the precedence level and tier each call was served by, and
/// both must add up over a flow rather than be replaced by its last call's. Only sums where both
/// rows carry the block; otherwise `||` has already kept whichever one does.
fn summed_block(key: &str) -> String {
    format!(
        "CASE WHEN trace_usage.metadata ? '{key}' AND EXCLUDED.metadata ? '{key}'
             THEN jsonb_build_object('{key}', (
                 SELECT jsonb_object_agg(
                            k,
                            COALESCE((trace_usage.metadata->'{key}'->>k)::bigint, 0)
                            + COALESCE((EXCLUDED.metadata->'{key}'->>k)::bigint, 0))
                   FROM (SELECT jsonb_object_keys(trace_usage.metadata->'{key}')
                         UNION
                         SELECT jsonb_object_keys(EXCLUDED.metadata->'{key}')) AS keys(k)))
             ELSE '{{}}'::jsonb END"
    )
}

/// Add one call to its `(trace_id, agent_name)` row, marking it router-priced.
///
/// The call is added to both the visible spend columns and the row's `router_*`
/// share. A Tempo-priced row keeps what Tempo already counted (the agent's own-key
/// calls among it); the materializer later sets each visible column to
/// `GREATEST(tempo, router_*)`, so neither half of a mixed trace is lost.
async fn write_trace_usage(
    tx: &mut sqlx::PgConnection,
    live: &LiveSpend,
) -> Result<(), sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct AgentInfo {
        name: String,
        workload_kind: Option<String>,
        is_coding: bool,
    }
    let info: Option<AgentInfo> = match live.agent {
        Some(id) => {
            // `coding_agent_integration_id` is server-managed; `metadata` is
            // owner-editable, and a `coding` label exempts spend from routed
            // budgets, so it must not be self-assignable (resolver/mod.rs does the same).
            sqlx::query_as(
                "SELECT name, workload_kind, \
                        coding_agent_integration_id IS NOT NULL AS is_coding \
                 FROM agents WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
        }
        None => None,
    };
    let agent_name: String = match (&info, live.agent) {
        (Some(i), _) => i.name.clone(),
        (None, Some(id)) => id.to_string(),
        (None, None) => live.component_label.clone().unwrap_or_default(),
    };
    let source: &'static str = info
        .as_ref()
        .map(|i| source_for(i.workload_kind.as_deref(), i.is_coding))
        .unwrap_or("agent");
    let trace_id: String = trace_key(live.flow_id.as_deref());

    sqlx::query(&UPSERT)
        .bind(&trace_id)
        .bind(&agent_name)
        .bind(live.flow_id.as_deref())
        .bind(info.as_ref().and(live.agent))
        .bind(live.owner)
        .bind(&live.model)
        .bind(&live.provider)
        .bind(live.input_tokens)
        .bind(live.output_tokens)
        .bind(live.cache_read_tokens)
        .bind(live.cache_creation_tokens)
        .bind(live.cost_usd)
        .bind(live.prompt_cost_usd)
        .bind(live.completion_cost_usd)
        .bind(live.latency_ms)
        .bind(source)
        .bind(&live.metadata)
        .bind(live.error_count)
        .bind(live.estimated_input_tokens)
        .bind(live.price_estimated)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

/// Identity carried from the record into the savings write, so that `log_usage` does not have to
/// keep the whole `UsageRecord` alive past the insert that consumes its owned fields.
struct SavingsIds {
    owner: Uuid,
    agent: Option<Uuid>,
    flow_id: Option<String>,
    provider: String,
    model: String,
    compress_bytes: Option<(usize, usize)>,
    compact_tools_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
}

/// Write the per-layer savings rows for this call, one for each layer that saved anything.
///
/// Separate from the `trace_usage` upsert and best-effort within an already best-effort path: a
/// missing savings row costs a dashboard a data point, while a failed one must not cost a request
/// that has already succeeded.
async fn write_savings(
    db: &PgPool,
    ids: &SavingsIds,
    cost: &nasiko_pricing::CostBreakdown,
    input: Option<i64>,
    cache_read: Option<i64>,
    cache_creation: Option<i64>,
) {
    if ids.compress_bytes.is_none() && ids.compact_tools_bytes.is_none() {
        return;
    }
    // The denominator of the blended rate must match the numerator's scope: `input` is already
    // disjoint from the cache counts by this point (`normalize_openai_details`), so prompt-side
    // tokens is their sum, not `input` alone.
    let prompt_side_tokens =
        input.unwrap_or(0) + cache_read.unwrap_or(0) + cache_creation.unwrap_or(0);
    let ctx = crate::savings::CallContext {
        user_id: ids.owner,
        agent_id: ids.agent,
        flow_id: ids.flow_id.clone(),
        provider: ids.provider.clone(),
        model: ids.model.clone(),
        cost,
        prompt_side_tokens,
        calibration: crate::savings::CalibrationInputs {
            request_bytes: ids.request_bytes,
            input_tokens: Some(prompt_side_tokens).filter(|t| *t > 0),
        },
    };

    let compress = ids.compress_bytes.and_then(|(bytes_in, bytes_out)| {
        crate::savings::compress_payload_row(bytes_in, bytes_out, ctx.clone())
    });
    let compact = ids
        .compact_tools_bytes
        .and_then(|(native, compact)| crate::savings::compact_tools_row(native, compact, ctx));
    for row in compress.iter().chain(compact.iter()) {
        nasiko_savings::insert(db, row).await;
    }
}

/// What the row's `metadata` JSONB records about one call.
///
/// A struct rather than five parameters, and pre-serialized `Value`s rather than the pricing and
/// compression types, so this module stays a pure DB concern.
struct MetadataInputs {
    /// e.g. `"direct_llm"` (chat) or `"embedding"`; was `token_usage.operation_type`.
    operation_type: &'static str,
    platform_paid: bool,
    attribution_source: Option<AttributionSource>,
    /// Which rates priced the call, and whether each was matched or inferred.
    pricing: serde_json::Value,
    /// The provider's reported cache-creation split, or `Value::Null` when it reported none.
    cache_creation: serde_json::Value,
    /// `None` when compression did not run.
    compress: Option<serde_json::Value>,
    /// Always `Some` once the brevity layer exists: it records "skipped, and why" too.
    brevity: Option<serde_json::Value>,
    /// `Some` on every chat call, applied or skipped; summed across the flow on write.
    compact_tools: Option<serde_json::Value>,
    /// `Some` on every call that went through model routing; summed across the flow on write, like
    /// `compact_tools`. `None` on one that did not route (embeddings).
    routing: Option<serde_json::Value>,
    /// `None` for a call that succeeded.
    failure: Option<FailureMetadata>,
}

/// What `metadata.failure` records about a failed call. The row is per trace and merges keys
/// later-call-wins, so this is the trace's latest failure; the count is `error_count`.
struct FailureMetadata {
    failure: CallFailure,
    /// Whether the provider reported usage for the call (then its tokens were priced).
    reported_usage: bool,
    estimated_input_tokens: i64,
}

/// The row's `metadata` JSONB.
///
/// Extracted so the shape is assertable without a database — `trace_usage.metadata` is read back
/// by `platform_paid_agent_usage` (`oss/server/src/router/usage_meta.rs`), so a change to
/// `key_source` here silently breaks flow billing.
fn build_metadata(inputs: MetadataInputs) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "operation_type": inputs.operation_type,
        "key_source": if inputs.platform_paid { "platform" } else { "user_secret" },
        "attribution": inputs.attribution_source.map(|s| s.as_label()),
        "pricing": inputs.pricing,
        "cache_creation": inputs.cache_creation,
    });
    if let Some(compress) = inputs.compress {
        metadata["compress"] = compress;
    }
    if let Some(brevity) = inputs.brevity {
        metadata["brevity"] = brevity;
    }
    if let Some(compact_tools) = inputs.compact_tools {
        metadata["compact_tools"] = compact_tools;
    }
    if let Some(routing) = inputs.routing {
        metadata["routing"] = routing;
    }
    if let Some(failure) = inputs.failure {
        metadata["failure"] = serde_json::json!({
            "class": failure.failure.class,
            "http_status": failure.failure.http_status,
            "tokens_estimated": !failure.reported_usage,
            "estimated_input_tokens": failure.estimated_input_tokens,
        });
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::{
        CallFailure, CallScope, FailedAttempt, FailureMetadata, MetadataInputs, UsageRecord,
        build_metadata, estimated_prompt_tokens,
    };

    /// A call that was priced but neither compressed nor cache-split.
    fn inputs(platform_paid: bool) -> MetadataInputs {
        MetadataInputs {
            operation_type: "direct_llm",
            platform_paid,
            attribution_source: None,
            pricing: serde_json::json!({ "source": "Db", "estimated": false }),
            cache_creation: serde_json::Value::Null,
            compress: None,
            brevity: None,
            compact_tools: None,
            routing: None,
            failure: None,
        }
    }

    /// The zero-behaviour-change guard: with no compression, the row carries exactly the billing
    /// and pricing-provenance keys and nothing else.
    #[test]
    fn metadata_without_compression_carries_only_the_billing_keys() {
        assert_eq!(
            build_metadata(inputs(true)),
            serde_json::json!({
                "operation_type": "direct_llm",
                "key_source": "platform",
                "attribution": null,
                "pricing": { "source": "Db", "estimated": false },
                "cache_creation": null,
            })
        );
        assert_eq!(build_metadata(inputs(false))["key_source"], "user_secret");
    }

    #[test]
    fn a_chat_calls_compact_tools_block_reaches_the_row_metadata() {
        let block = serde_json::json!({ "calls": 1, "applied": 1, "tool_bytes_sent": 10 });
        let metadata = build_metadata(MetadataInputs {
            compact_tools: Some(block.clone()),
            ..inputs(true)
        });
        assert_eq!(metadata["compact_tools"], block);
    }

    /// NAS-1018: only a failed call with no reported usage gets a prompt estimate, at the
    /// fallback 4 chars per token; a success or a failure that reported usage gets none.
    #[test]
    fn only_a_failed_call_without_usage_is_estimated() {
        let failure = Some(CallFailure::new("status", Some(503)));
        let reported: crate::ir::Usage =
            serde_json::from_value(serde_json::json!({ "prompt_tokens": 10 })).unwrap();
        assert_eq!(estimated_prompt_tokens(failure, None, Some(4002)), 1000);
        assert_eq!(
            estimated_prompt_tokens(failure, Some(&reported), Some(4000)),
            0
        );
        assert_eq!(estimated_prompt_tokens(None, None, Some(4000)), 0);
        assert_eq!(estimated_prompt_tokens(failure, None, None), 0);
    }

    /// Q1-1: a provider that refuses a call (a 4xx such as 404) bills nothing, so the turn must
    /// not be flagged as an estimate. A timeout, a rate limit or a server error may have run the
    /// prompt, so those keep their estimate.
    #[test]
    fn a_refused_call_gets_no_prompt_estimate() {
        let status = |code| Some(CallFailure::new("status", Some(code)));
        for refused in [400, 401, 404, 422] {
            assert_eq!(
                estimated_prompt_tokens(status(refused), None, Some(4000)),
                0,
                "{refused}"
            );
        }
        for may_have_run in [408, 429, 500, 503] {
            assert_eq!(
                estimated_prompt_tokens(status(may_have_run), None, Some(4000)),
                1000,
                "{may_have_run}"
            );
        }
        let transport = Some(CallFailure::new("transport", None));
        assert_eq!(estimated_prompt_tokens(transport, None, Some(4000)), 1000);
    }

    #[test]
    fn a_failure_is_recorded_under_its_own_key_and_flagged_estimated() {
        let metadata = build_metadata(MetadataInputs {
            failure: Some(FailureMetadata {
                failure: CallFailure::new("status", Some(429)),
                reported_usage: false,
                estimated_input_tokens: 1000,
            }),
            ..inputs(true)
        });
        assert_eq!(
            metadata["failure"],
            serde_json::json!({
                "class": "status",
                "http_status": 429,
                "tokens_estimated": true,
                "estimated_input_tokens": 1000,
            })
        );
        assert!(build_metadata(inputs(true)).get("failure").is_none());
    }

    #[test]
    fn a_failed_attempt_row_carries_the_scope_and_no_usage() {
        let scope = CallScope {
            owner_id: "owner".into(),
            agent_id: "agent".into(),
            operation_type: "direct_llm",
            streaming: false,
            flow_id: Some("flow".into()),
            attribution_source: None,
            request_bytes: Some(800),
            routing_metadata: Some(serde_json::json!({ "calls": 1, "classified": 1 })),
        };
        let record = UsageRecord::failed_attempt(
            &scope,
            FailedAttempt {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                platform_paid: false,
                latency_ms: 29_000,
                failure: CallFailure::new("status", Some(503)),
            },
        );
        assert!(record.usage.is_none());
        assert_eq!(record.failure, Some(CallFailure::new("status", Some(503))));
        assert_eq!(record.finish_reason.as_deref(), Some("failed:status"));
        assert_eq!(
            (record.flow_id.as_deref(), record.request_bytes),
            (Some("flow"), Some(800))
        );
        assert!(!record.platform_paid);
        assert_eq!(record.latency_ms, 29_000);
        // A failed attempt was routed like the call that followed it, so it counts the same way
        // (NAS-1263) — a chain whose first model was classified and whose fallback answered must
        // not look like a call that never classified.
        assert_eq!(
            record.routing_metadata,
            Some(serde_json::json!({ "calls": 1, "classified": 1 }))
        );
    }

    /// NAS-1263: the routing block reaches the row's metadata under its own key, beside the
    /// billing keys rather than in place of any of them.
    #[test]
    fn a_routed_calls_routing_block_reaches_the_row_metadata() {
        let block = serde_json::json!({ "calls": 1, "small_talk": 1, "tier3": 1 });
        let metadata = build_metadata(MetadataInputs {
            routing: Some(block.clone()),
            ..inputs(true)
        });
        assert_eq!(metadata["routing"], block);
        assert_eq!(metadata["key_source"], "platform");
    }

    /// A call that never routed by model (embeddings) writes no block at all: its absence is what
    /// tells a reader the call had no routing decision, which zeros could not.
    #[test]
    fn a_call_that_did_not_route_writes_no_routing_block() {
        assert!(build_metadata(inputs(true)).get("routing").is_none());
    }

    #[test]
    fn compression_stats_are_added_under_their_own_key() {
        let stats = serde_json::json!({ "applied": true, "bytes_in": 100, "bytes_out": 40 });
        let metadata = build_metadata(MetadataInputs {
            compress: Some(stats.clone()),
            ..inputs(true)
        });

        assert_eq!(metadata["compress"], stats);
        // The keys flow billing and cost attribution read must survive alongside it.
        assert_eq!(metadata["key_source"], "platform");
        assert!(metadata.get("attribution").is_some());
        assert_eq!(metadata["pricing"]["source"], "Db");
    }
}
