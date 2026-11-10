//! Fire-and-forget usage logging to the `trace_usage` spend table.
//!
//! Adds each LLM call to its `(trace_id, agent_name)` row, priced here in Rust
//! through the platform's single cost engine. Failures are logged and swallowed — usage logging must
//! never break or delay the response.
//!
//! Missing exact prices fall back through the shared engine. Pricing provenance
//! records inferred rates so estimates are distinguishable from matched prices.

use std::sync::Arc;

use chrono::Utc;
use nasiko_pricing::{PricingEngine, PromptConvention, RawUsage};
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
    /// `(bytes_in, bytes_out)` from IP-1 when it actually reduced the payload, for the
    /// `token_savings` ledger. Two integers rather than the compression module's type, so this
    /// module stays a pure DB concern.
    ///
    /// The ledger write lives here, beside the `trace_usage` write, because pricing the saving
    /// needs the same `PricingEngine` result that prices the call — computing it anywhere else
    /// would price a saving at a different rate than the spend it is subtracted from, and the
    /// reduction percentage would stop being coherent.
    pub compress_bytes: Option<(usize, usize)>,
    /// Total text bytes actually sent, after compression and the brevity directive. Calibrates
    /// chars-per-token against this very call rather than a fixed divisor (see `savings.rs`).
    pub request_bytes: Option<usize>,
}

/// Spawn the usage write so it never blocks the response.
pub fn spawn_log(db: PgPool, pricing: Arc<PricingEngine>, record: UsageRecord) {
    tokio::spawn(async move {
        if let Err(e) = log_usage(db, pricing.as_ref(), record).await {
            tracing::warn!(error = %e, "llm_usage write failed (swallowed)");
        }
    });
}

/// Add one priced call to its `trace_usage` row.
pub async fn log_usage(
    db: PgPool,
    pricing: &PricingEngine,
    record: UsageRecord,
) -> Result<(), String> {
    // trace_usage.user_id and token_savings.user_id are FKs to users(id); without a valid owner we
    // cannot write a row, so skip (best-effort logging must never surface an error).
    let Ok(owner) = Uuid::parse_str(&record.owner_id) else {
        tracing::debug!(owner_id = %record.owner_id, "skipping usage row: owner_id is not a uuid");
        return Ok(());
    };
    let agent = Uuid::parse_str(&record.agent_id).ok();
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
    });

    let live: LiveSpend = LiveSpend {
        metadata,
        owner,
        agent,
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
    };

    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    write_trace_usage(&mut tx, &live)
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
    /// Per-call metadata, merged into `trace_usage.metadata` (later call wins per key).
    metadata: serde_json::Value,
    owner: Uuid,
    agent: Option<Uuid>,
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

/// Key for a call: the flow's trace id, or `direct:<fresh uuid v4>` when the
/// call carried no `traceparent` (local coding agents) — one row per such call.
fn trace_key(flow_id: Option<&str>) -> String {
    match flow_id {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => format!("direct:{}", Uuid::new_v4()),
    }
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
        (None, None) => String::new(),
    };
    let source: &'static str = info
        .as_ref()
        .map(|i| source_for(i.workload_kind.as_deref(), i.is_coding))
        .unwrap_or("agent");
    let trace_id: String = trace_key(live.flow_id.as_deref());

    sqlx::query(
        r#"INSERT INTO trace_usage (
               trace_id, agent_name, session_id, agent_id, user_id, model, provider,
               input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
               cost_usd, prompt_cost_usd, completion_cost_usd,
               router_input_tokens, router_output_tokens, router_cache_read_tokens,
               router_cache_creation_tokens, router_cost_usd, router_prompt_cost_usd,
               router_completion_cost_usd,
               latency_ms, started_at, source, priced_by, materialized_at, metadata
           ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,
                     $8,$9,$10,$11,$12,$13,$14,
                     $15,now(),$16,'router',now(),$17)
           ON CONFLICT (trace_id, agent_name) DO UPDATE SET
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
               model                        = EXCLUDED.model,
               provider                     = EXCLUDED.provider,
               agent_id                     = COALESCE(trace_usage.agent_id, EXCLUDED.agent_id),
               user_id                      = COALESCE(trace_usage.user_id, EXCLUDED.user_id),
               latency_ms                   = COALESCE(trace_usage.latency_ms, 0) + EXCLUDED.latency_ms,
               source                       = CASE WHEN EXCLUDED.source = 'agent' THEN trace_usage.source ELSE EXCLUDED.source END,
               priced_by                    = 'router',
               materialized_at              = now(),
               metadata                     = trace_usage.metadata || EXCLUDED.metadata"#,
    )
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
    request_bytes: Option<usize>,
}

/// Write the per-layer savings row for this call, if any layer saved anything.
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
    let Some((bytes_in, bytes_out)) = ids.compress_bytes else {
        return;
    };
    // The denominator of the blended rate must match the numerator's scope: `input` is already
    // disjoint from the cache counts by this point (`normalize_openai_details`), so prompt-side
    // tokens is their sum, not `input` alone.
    let prompt_side_tokens =
        input.unwrap_or(0) + cache_read.unwrap_or(0) + cache_creation.unwrap_or(0);

    let row = crate::savings::compress_payload_row(
        bytes_in,
        bytes_out,
        crate::savings::CallContext {
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
        },
    );

    if let Some(row) = row {
        nasiko_savings::insert(db, &row).await;
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
    metadata
}

#[cfg(test)]
mod tests {
    use super::{MetadataInputs, build_metadata};

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
