//! One-off: re-price existing `trace_usage` rows with the sibling-span dedup
//! fix (`link_router_spans` / `find_sibling_duplicate`, `types.rs`).
//!
//! Before that fix, an agent whose own instrumentation records a call as a
//! *sibling* of the LLM router's span (rather than nesting it) had both spans
//! counted: the router's correct, cache-aware figure was dropped as
//! "unattributable", and the agent's own, typically bigger and cache-blind
//! figure stood in its place. Rows materialized before the fix still carry
//! that inflated cost — this script re-reads each trace from Tempo with the
//! fixed logic and corrects the stored row.
//!
//! It touches only the cost/token columns of rows that already exist; it never
//! inserts a new row, and never changes `agent_id`/`user_id`/`session_id`
//! (those were resolved correctly already and this bug never affected them).
//!
//! A trace outside Tempo's retention window can't be re-read and is reported
//! as unrecoverable, not silently left wrong — there is no better number to
//! put there.
//!
//! Defaults to a dry run: it prints what would change and writes nothing.
//! Pass `--apply` to actually update the database.
//!
//! ```sh
//! cargo run -p nasiko-observability --example backfill_trace_usage_dedup
//! cargo run -p nasiko-observability --example backfill_trace_usage_dedup -- --apply
//! ```

use std::sync::Arc;

use nasiko_observability::{ObservabilityProvider, TempoLokiProvider};
use sqlx::{PgPool, Row};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let apply = std::env::args().any(|a| a == "--apply");

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (same value the server uses)")?;
    let tempo_url = std::env::var("TEMPO_URL").unwrap_or_else(|_| "http://localhost:3200".into());
    let loki_url = std::env::var("LOKI_URL").unwrap_or_else(|_| "http://localhost:3100".into());

    let pool = PgPool::connect(&database_url).await?;
    let pricing = Arc::new(nasiko_pricing::PricingEngine::new(pool.clone()));
    let provider = TempoLokiProvider::new(tempo_url, loki_url, pricing);

    // Synthetic `direct:<id>` keys (coding-agent direct calls, no Tempo trace
    // behind them — `trace_materializer.rs::sync_direct_token_usage`) were
    // never built from spans, so this bug can't have touched them.
    let trace_ids: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT trace_id FROM trace_usage WHERE trace_id NOT LIKE 'direct:%' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await?;

    println!(
        "{} real traces in trace_usage; mode = {}",
        trace_ids.len(),
        if apply {
            "APPLY"
        } else {
            "dry run (pass --apply to write)"
        }
    );

    let mut unreadable = 0usize;
    let mut unchanged = 0usize;
    let mut corrected = 0usize;
    let mut total_delta = 0f64;

    for trace_id in &trace_ids {
        let rows = match provider.extract_trace_usage(trace_id).await {
            Ok(rows) => rows,
            Err(e) => {
                unreadable += 1;
                println!("  SKIP  {trace_id}  (unreadable: {e})");
                continue;
            }
        };

        for row in rows {
            let existing = sqlx::query(
                "SELECT cost_usd, input_tokens, output_tokens FROM trace_usage \
                 WHERE trace_id = $1 AND agent_name = $2",
            )
            .bind(&row.trace_id)
            .bind(&row.agent_name)
            .fetch_optional(&pool)
            .await?;

            let Some(existing) = existing else {
                // extract_trace_usage can find an agent that wasn't recorded
                // at all last time (e.g. it had no usage before a span-level
                // fix made it attributable) -- nothing to correct, and this
                // script only corrects, never inserts.
                continue;
            };

            let old_cost: f64 = existing.try_get("cost_usd")?;
            let old_input: i64 = existing.try_get("input_tokens")?;
            let old_output: i64 = existing.try_get("output_tokens")?;
            let delta = row.cost_usd - old_cost;

            if delta.abs() < 0.000_05
                && old_input == row.input_tokens as i64
                && old_output == row.output_tokens as i64
            {
                unchanged += 1;
                continue;
            }

            corrected += 1;
            total_delta += delta;
            println!(
                "  {} / {:<32} cost ${old_cost:.6} -> ${:.6} ({delta:+.6})  tokens {old_input}/{old_output} -> {}/{}",
                &row.trace_id[..row.trace_id.len().min(12)],
                row.agent_name,
                row.cost_usd,
                row.input_tokens,
                row.output_tokens,
            );

            if apply {
                sqlx::query(
                    "UPDATE trace_usage SET \
                        model = $3, provider = $4, \
                        input_tokens = $5, output_tokens = $6, \
                        cache_read_tokens = $7, cache_creation_tokens = $8, \
                        tool_call_count = $9, \
                        cost_usd = $10, prompt_cost_usd = $11, completion_cost_usd = $12, \
                        cost_estimated = $13, materialized_at = now() \
                     WHERE trace_id = $1 AND agent_name = $2",
                )
                .bind(&row.trace_id)
                .bind(&row.agent_name)
                .bind(&row.model)
                .bind(&row.provider)
                .bind(row.input_tokens as i64)
                .bind(row.output_tokens as i64)
                .bind(row.cache_read_tokens as i64)
                .bind(row.cache_creation_tokens as i64)
                .bind(row.tool_call_count as i64)
                .bind(row.cost_usd)
                .bind(row.prompt_cost_usd)
                .bind(row.completion_cost_usd)
                .bind(row.cost_estimated)
                .execute(&pool)
                .await?;
            }
        }
    }

    println!(
        "\n{corrected} row(s) corrected (net cost change {total_delta:+.4}), \
         {unchanged} already correct, {unreadable} trace(s) no longer in Tempo (left as-is)."
    );
    if corrected > 0 && !apply {
        println!("Dry run only — re-run with --apply to write these changes.");
    }
    Ok(())
}
