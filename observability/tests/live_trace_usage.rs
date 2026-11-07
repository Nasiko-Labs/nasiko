//! Reconciles `extract_trace_usage` against a live Tempo, without writing anything.
//!
//! This is the check that would otherwise only be possible after a deploy: the trace
//! materializer's output is what backfills `trace_usage`, and its correctness depends on
//! reading the cached-token attribute and splitting the prompt correctly. Getting that wrong
//! silently inflates every figure on the TokenOps tab.
//!
//! Ignored by default — it needs a reachable Tempo and real traces:
//!   TEMPO_URL=http://localhost:3200 TRACE_IDS=abc,def \
//!     cargo test -p nasiko-observability --test live_trace_usage -- --ignored --nocapture

use nasiko_observability::{ObservabilityProvider, TempoLokiProvider};
use nasiko_pricing::PricingEngine;
use std::sync::Arc;

/// Uses the real `model_pricing` table when `DATABASE_URL` is set. That matters: with the
/// static fallback every model looks like it has no cache rate, cached tokens get billed at
/// the full input rate, and the cost column looks broken even when the split is right.
async fn provider() -> TempoLokiProvider {
    let tempo = std::env::var("TEMPO_URL").unwrap_or_else(|_| "http://localhost:3200".into());
    let loki = std::env::var("LOKI_URL").unwrap_or_else(|_| "http://localhost:3100".into());
    // The engine needs a pool for the synced price book; without one it still
    // resolves through the offline table, which is enough for the shape checks
    // here (the rate assertions live in nasiko-pricing's own live tests).
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://invalid:invalid@127.0.0.1:1/none".into());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy(&url)
        .expect("build a lazy pool");
    TempoLokiProvider::new(tempo, loki, Arc::new(PricingEngine::new(pool)))
}

#[tokio::test]
#[ignore = "needs a live Tempo with real traces and DATABASE_URL"]
async fn extracted_usage_reconciles_with_token_usage() {
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");

    // Every trace the llm-router billed. Its `router_*` share is written from the
    // provider's own usage object, so it is the reference the trace-derived figures have
    // to agree with. `direct:` rows carry no trace to read back.
    let billed: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT trace_id, sum(router_input_tokens)::int8, sum(router_cache_read_tokens)::int8,
                sum(router_output_tokens)::int8
           FROM trace_usage
          WHERE priced_by = 'router' AND trace_id NOT LIKE 'direct:%'
          GROUP BY trace_id",
    )
    .fetch_all(&pool)
    .await
    .expect("read trace_usage");
    assert!(!billed.is_empty(), "no billed traces to reconcile against");

    let p = provider().await;
    let (mut ok, mut skipped) = (0usize, 0usize);

    println!(
        "\n{:<34}{:>9}{:>9}{:>9}{:>9}  verdict",
        "trace", "in(t)", "in(b)", "cache(t)", "cache(b)"
    );
    for (trace_id, b_in, b_cache, _b_out) in &billed {
        let rows = match p.extract_trace_usage(trace_id).await {
            Ok(r) if !r.is_empty() => r,
            // Aged out of Tempo retention, or never instrumented — not a mismatch.
            _ => {
                skipped += 1;
                continue;
            }
        };
        let t_in: u64 = rows.iter().map(|r| r.input_tokens).sum();
        let t_cache: u64 = rows.iter().map(|r| r.cache_read_tokens).sum();

        println!(
            "{:<34}{:>9}{:>9}{:>9}{:>9}  {}",
            &trace_id[..trace_id.len().min(32)],
            t_in,
            b_in,
            t_cache,
            b_cache,
            if t_in as i64 == *b_in && t_cache as i64 == *b_cache {
                "ok"
            } else {
                "MISMATCH"
            }
        );

        // The bug this guards: cached tokens left inside `input` inflate it by exactly the
        // cache amount, and `cache_read` reads as zero.
        assert_eq!(
            t_in as i64, *b_in,
            "{trace_id}: trace-derived input disagrees with what was billed"
        );
        assert_eq!(
            t_cache as i64, *b_cache,
            "{trace_id}: trace-derived cache reads disagree with what was billed"
        );
        ok += 1;
    }

    println!("\n{ok} trace(s) reconciled, {skipped} not in Tempo\n");
    assert!(
        ok > 0,
        "every trace was skipped — nothing was actually checked"
    );
}
