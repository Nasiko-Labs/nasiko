//! Recording what IP-3 removed from one turn's tool results.
//!
//! The ReAct loop compresses every tool result as it stores it
//! (`oss/react-agent/src/context.rs::push_tool_result`). That has run in production since IP-3
//! shipped and reported nothing: `UNINSTRUMENTED_LAYERS` carried `compress_tool_result` precisely
//! so the dashboard said "not measured" rather than claiming a zero. This is the emitter that
//! retires that entry.
//!
//! Two sinks, the same pair `context_report` writes to and for the same reasons:
//!
//! * **The dispatch span**, as `nasiko.tool_results.*` — the trace page reads Tempo, not Postgres.
//! * **`token_savings`**, as one `compress_tool_result` row — the only shape `/finops/savings` can
//!   aggregate by window, agent and program.
//!
//! Counts only. Never the tool output itself.
//!
//! # Why this is a turn-level row, not a per-result one
//!
//! A ReAct turn calls several tools, and `token_savings` is keyed per (call, layer). One row per
//! *tool result* would multiply the layer's row count by the loop's fan-out while measuring the
//! same turn, and the page aggregates by summing — so the figure would be right and the row count
//! misleading. The tally accumulates across the turn and lands once, which is also the only grain
//! at which a streaming turn's spawned task can be observed at all.

use nasiko_react_agent::CompressionTally;
use sqlx::PgPool;
use uuid::Uuid;

/// Same divisor the rest of the context pipeline estimates with
/// (`ContextConfig::chars_per_token`, and `session_history::est_tokens` on the history side).
///
/// Duplicated as a constant rather than imported because both owners keep it private, and a third
/// crate reaching into either for it would couple them to this emitter. If calibration per §2.1 of
/// `docs/TOKENOPS_SAVINGS_ATTRIBUTION_PLAN.md` ever lands, this constant is what it replaces.
const CHARS_PER_TOKEN: usize = 4;

/// Owned attribution, so the write can outlive the request.
pub struct ToolResultRecord {
    pub user_id: Uuid,
    /// The orchestrator turn has no single agent — it calls whichever ones it routes to — so this
    /// is `None` on the chat path and the per-agent breakdown resolves it through `flows`,
    /// exactly as the context report does.
    pub agent_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub flow_id: Option<String>,
}

/// Attach the tally to `span` and persist it.
///
/// Spawned, best-effort, and silent when nothing was compressed: a turn whose tool results were
/// already small (or whose agent has IP-3 off) has no saving to report, and a row of zeros would
/// dilute every average the Optimization page computes — the same rule
/// `context_report::record` applies to an empty pool.
pub fn record(
    db: &PgPool,
    span: &tracing::Span,
    tally: &CompressionTally,
    record: ToolResultRecord,
) {
    let saved_bytes = tally.saved_bytes();
    if saved_bytes == 0 {
        return;
    }

    let (bytes_in, bytes_out) = (tally.bytes_in(), tally.bytes_out());
    set_span_attributes(span, tally);

    // Truncate, never round — the same bias every figure in this ledger carries: under-claim.
    let saved_tokens = (saved_bytes / CHARS_PER_TOKEN) as i64;
    if saved_tokens == 0 {
        return;
    }

    let db = db.clone();
    tokio::spawn(async move {
        nasiko_savings::insert(
            &db,
            &nasiko_savings::SavingsRow {
                user_id: record.user_id,
                agent_id: record.agent_id,
                flow_id: record.flow_id,
                session_id: record.session_id,
                // No provider or model: the tool results are compressed as they are *stored*, which
                // happens between calls rather than inside one, so there is no response to read a
                // rate from. Matches the two history-side rows, which carry the token figure only.
                provider: None,
                model: None,
                layer: nasiko_savings::Layer::CompressToolResult,
                // Unlike the history rows, this layer has real bytes on both sides — it is a
                // straight before/after on text held in memory — so the ledger's ground-truth
                // columns are populated rather than left NULL.
                bytes_before: Some(bytes_in as i64),
                bytes_after: Some(bytes_out as i64),
                saved_input_tokens: saved_tokens,
                // IP-3 shrinks what the loop re-reads on later turns; it cannot shorten a reply.
                saved_output_tokens: 0,
                saved_cost_usd: 0.0,
                method: nasiko_savings::Method::MeasuredBytes,
                token_estimated: true,
                // Tier is a history-selection concept; this layer has none.
                context_tier: None,
            },
        )
        .await;
    });
}

/// The `nasiko.tool_results.*` attribute set.
///
/// `results_compressed` is carried separately from the byte counts because equal byte counts are
/// ambiguous on their own: a turn where IP-3 was off and a turn where it ran and found nothing to
/// elide both report `bytes_in == bytes_out`, and only this tells them apart.
fn set_span_attributes(span: &tracing::Span, tally: &CompressionTally) {
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;

    span.set_attribute("nasiko.tool_results.bytes_in", tally.bytes_in() as i64);
    span.set_attribute("nasiko.tool_results.bytes_out", tally.bytes_out() as i64);
    span.set_attribute(
        "nasiko.tool_results.compressed",
        tally.results_compressed() as i64,
    );
}
