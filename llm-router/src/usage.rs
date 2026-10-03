//! Fire-and-forget usage logging to the `token_usage` table.
//!
//! Writes one row per LLM call, priced here in Rust through the platform's
//! single cost engine. Failures are logged and swallowed — usage logging must
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
    /// `token_usage.operation_type`, e.g. `"direct_llm"` (chat) or `"embedding"`.
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
    /// calls always carry it). Written to `token_usage.session_id` — the same
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
    /// The ledger write lives here, beside the `token_usage` write, because pricing the saving
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
    spawn_log_with(db, pricing, record, None);
}

/// [`spawn_log`] plus the pre-serialized `metadata.compact_tools` block (`compact_tools.rs`).
///
/// The block travels beside the record rather than on it so `UsageRecord`'s shape stays what
/// every other writer (and every test that builds one) already knows. `None` leaves the row
/// byte-identical to one written through [`spawn_log`]; once the fleet flag is on the chat
/// handler always passes `Some`, recording "applied" or "bypassed, and why" like the brevity block.
pub fn spawn_log_with(
    db: PgPool,
    pricing: Arc<PricingEngine>,
    record: UsageRecord,
    compact_tools: Option<serde_json::Value>,
) {
    tokio::spawn(async move {
        if let Err(e) = log_usage_with(db, pricing.as_ref(), record, compact_tools).await {
            tracing::warn!(error = %e, "llm_usage write failed (swallowed)");
        }
    });
}

/// Insert one priced `token_usage` row.
pub async fn log_usage(
    db: PgPool,
    pricing: &PricingEngine,
    record: UsageRecord,
) -> Result<(), String> {
    log_usage_with(db, pricing, record, None).await
}

/// [`log_usage`] with the optional `metadata.compact_tools` block (see [`spawn_log_with`]).
pub async fn log_usage_with(
    db: PgPool,
    pricing: &PricingEngine,
    record: UsageRecord,
    compact_tools: Option<serde_json::Value>,
) -> Result<(), String> {
    // token_usage.user_id is NOT NULL + FK to users(id); without a valid owner we
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

    let metadata = build_metadata(MetadataInputs {
        platform_paid: record.platform_paid,
        attribution_source: record.attribution_source,
        pricing: priced.provenance(),
        cache_creation: serde_json::to_value(&cache_details).unwrap_or(serde_json::Value::Null),
        compress: record.compress_metadata,
        brevity: record.brevity_metadata,
        compact_tools,
    });

    sqlx::query(
        r#"INSERT INTO token_usage
               (user_id, agent_id, operation_type, provider, model,
                input_tokens, output_tokens, total_tokens,
                cache_read_input_tokens, cache_creation_input_tokens,
                cached_tokens, reasoning_tokens,
                latency_ms, streaming, finish_reason, session_id, metadata,
                cost_usd)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)"#,
    )
    .bind(owner)
    .bind(agent)
    .bind(record.operation_type)
    .bind(&record.provider)
    .bind(&record.model)
    .bind(saturating_i32(input.unwrap_or(0)))
    .bind(saturating_i32(output.unwrap_or(0)))
    .bind(saturating_i32(total.unwrap_or(0)))
    .bind(saturating_i32(cache_read.unwrap_or(0)))
    .bind(saturating_i32(cache_creation.unwrap_or(0)))
    .bind(saturating_i32(
        record.cached_tokens.unwrap_or(cache_read.unwrap_or(0)),
    ))
    .bind(saturating_i32(record.reasoning_tokens.unwrap_or(0)))
    .bind(saturating_i32(record.latency_ms))
    .bind(record.streaming)
    .bind(record.finish_reason)
    .bind(record.flow_id)
    .bind(metadata)
    .bind(priced.cost.total_usd)
    .execute(&db)
    .await
    .map_err(|e| e.to_string())?;

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
/// Separate from the `token_usage` insert and best-effort within an already best-effort path: a
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
    /// `None` while compact tool definitions are off for the deployment.
    compact_tools: Option<serde_json::Value>,
}

/// The row's `metadata` JSONB.
///
/// Extracted so the shape is assertable without a database — `token_usage.metadata` is read back
/// by `platform_paid_agent_usage` (`oss/server/src/router/usage_meta.rs`), so a change to
/// `key_source` here silently breaks flow billing.
fn build_metadata(inputs: MetadataInputs) -> serde_json::Value {
    let mut metadata = serde_json::json!({
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
    metadata
}

fn saturating_i32(value: i64) -> i32 {
    value.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::{MetadataInputs, build_metadata, saturating_i32};

    /// A call that was priced but neither compressed nor cache-split.
    fn inputs(platform_paid: bool) -> MetadataInputs {
        MetadataInputs {
            platform_paid,
            attribution_source: None,
            pricing: serde_json::json!({ "source": "Db", "estimated": false }),
            cache_creation: serde_json::Value::Null,
            compress: None,
            brevity: None,
            compact_tools: None,
        }
    }

    #[test]
    fn usage_values_saturate_without_wrapping() {
        assert_eq!(saturating_i32(i64::MAX), i32::MAX);
        assert_eq!(saturating_i32(i64::MIN), i32::MIN);
        assert_eq!(saturating_i32(42), 42);
    }

    /// The zero-behaviour-change guard: with no compression, the row carries exactly the billing
    /// and pricing-provenance keys and nothing else.
    #[test]
    fn metadata_without_compression_carries_only_the_billing_keys() {
        assert_eq!(
            build_metadata(inputs(true)),
            serde_json::json!({
                "key_source": "platform",
                "attribution": null,
                "pricing": { "source": "Db", "estimated": false },
                "cache_creation": null,
            })
        );
        assert_eq!(build_metadata(inputs(false))["key_source"], "user_secret");
    }

    #[test]
    fn compact_tools_block_is_added_under_its_own_key_only_when_present() {
        let block = serde_json::json!({ "applied": true, "bypass": null, "calls": 1 });
        let metadata = build_metadata(MetadataInputs {
            compact_tools: Some(block.clone()),
            ..inputs(true)
        });
        assert_eq!(metadata["compact_tools"], block);
        assert_eq!(metadata["key_source"], "platform");
        assert!(build_metadata(inputs(true)).get("compact_tools").is_none());
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
