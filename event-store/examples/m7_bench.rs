//! TRD Phase 1 exit check M7: per-user cold `token_usage` read p50 < 50 ms.
//! Builds a Lance dataset of N rows (daily-style appends, BTREE indexes), then
//! times `user_id` + time-range scans.
//!
//! Run: `cargo run --release -p nasiko-event-store --example m7_bench -- [rows] [users] [dir]`
//! Defaults: 1_000_000 rows, 10_000 users, /tmp/m7_bench.

use std::time::Instant;

use chrono::{TimeDelta, TimeZone, Utc};
use nasiko_event_store::{cold, token_usage};
use rust_decimal::Decimal;
use uuid::Uuid;

const BATCH: u64 = 250_000;

fn row(i: u64, users: u64) -> token_usage::TokenUsageRow {
    let user = Uuid::from_u128((i * 2_654_435_761 % users) as u128 + 1);
    token_usage::TokenUsageRow {
        id: Uuid::new_v4(),
        user_id: user,
        agent_id: Some(Uuid::from_u128((i % 50) as u128 + 1_000)),
        operation_type: "chat".into(),
        request_id: None,
        session_id: Some(format!("{:032x}", i / 4)),
        provider: "openai".into(),
        model: ["gpt-4o-mini", "gpt-4o", "claude-sonnet"][(i % 3) as usize].into(),
        input_tokens: (i % 900) as i32,
        output_tokens: (i % 300) as i32,
        total_tokens: (i % 1200) as i32,
        cache_creation_input_tokens: Some(0),
        cache_read_input_tokens: None,
        cached_tokens: Some(0),
        audio_tokens: None,
        reasoning_tokens: None,
        accepted_prediction_tokens: None,
        rejected_prediction_tokens: None,
        completion_tokens_details: None,
        prompt_tokens_details: None,
        cost_usd: Some(Decimal::new((i % 10_000) as i64, 8)),
        latency_ms: Some((i % 2000) as i32),
        ttft_ms: None,
        streaming: Some(true),
        finish_reason: Some("stop".into()),
        metadata: Some(serde_json::json!({})),
        // spread over ~1 year
        created_at: Utc
            .timestamp_opt(
                1_700_000_000 + (i * 31_536_000 / 100_000_000) as i64 * 100,
                0,
            )
            .unwrap(),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let rows: u64 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let users: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10_000);
    let dir = args.next().unwrap_or_else(|| "/tmp/m7_bench".into());
    let _ = std::fs::remove_dir_all(&dir);
    let uri = cold::dataset_uri(&dir, token_usage::TABLE);

    let t = Instant::now();
    let mut start = 0;
    while start < rows {
        let n = BATCH.min(rows - start);
        let mut b: Vec<_> = (start..start + n).map(|i| row(i, users)).collect();
        token_usage::sort_rows(&mut b);
        cold::append(
            &uri,
            token_usage::to_record_batch(&b, Uuid::new_v4())?,
            &["user_id", "agent_id", "session_id"],
        )
        .await?;
        start += n;
    }
    println!("load {rows} rows in {:.1}s", t.elapsed().as_secs_f64());

    let to = Utc
        .timestamp_opt(
            1_700_000_000 + 31_536_000 * rows as i64 / 100_000_000 * 100 + 1,
            0,
        )
        .unwrap();
    let from = to - TimeDelta::days(90);
    let mut lat = Vec::new();
    let mut hits = 0;
    for q in 0..200u128 {
        let user = cold::lit(&Uuid::from_u128(q * 37 % users as u128 + 1).to_string());
        let filter = format!(
            "user_id = {user} AND created_at >= TIMESTAMP '{}' AND created_at < TIMESTAMP '{}'",
            from.format("%Y-%m-%d %H:%M:%S"),
            to.format("%Y-%m-%d %H:%M:%S")
        );
        let t = Instant::now();
        let b = cold::scan(&uri, &filter).await?;
        lat.push(t.elapsed().as_secs_f64() * 1e3);
        hits += b.iter().map(|b| b.num_rows()).sum::<usize>();
    }
    lat.sort_by(f64::total_cmp);
    let p = |q: f64| lat[((lat.len() as f64 - 1.0) * q) as usize];
    println!(
        "M7 per-user 90d scan: p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms ({hits} rows / 200 queries)",
        p(0.5),
        p(0.95),
        p(0.99)
    );
    println!(
        "{}",
        if p(0.5) < 50.0 {
            "PASS (p50 < 50 ms)"
        } else {
            "FAIL (p50 >= 50 ms)"
        }
    );
    Ok(())
}
