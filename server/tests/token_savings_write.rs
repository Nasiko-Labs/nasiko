//! The savings-ledger write path, exercised through the real `log_usage`.
//!
//! This is the test the whole feature rests on. Everything else — the aggregate, the three scopes,
//! the dashboard — reads `token_savings`, and until this ran, nothing had ever observed a row being
//! *written* by the code that writes them in production. The savings API's own tests seed the table
//! directly, so they would pass just as happily against a writer that never fired.
//!
//! It lives in `oss/server/tests` rather than in the llm-router because that is where a Postgres
//! fixture already exists; the code under test is `nasiko_llm_router::usage::log_usage`.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test token_savings_write -- --test-threads=1

#[allow(dead_code)] // this file uses the fixture's pool, not its HTTP client
mod common;

use std::sync::Arc;

use nasiko_llm_router::usage::{UsageRecord, log_usage};
use nasiko_pricing::PricingEngine;
use serde_json::json;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

/// A usage record for a call that reported real token counts.
///
/// `compress_bytes` and `request_bytes` are what the chat handler threads through after both
/// seams; everything else mirrors a plain non-streaming completion.
fn record(
    owner: Uuid,
    agent: Uuid,
    compress_bytes: Option<(usize, usize)>,
    request_bytes: Option<usize>,
) -> UsageRecord {
    UsageRecord {
        owner_id: owner.to_string(),
        agent_id: agent.to_string(),
        operation_type: "direct_llm",
        provider: "openai".into(),
        model: "gpt-4o".into(),
        usage: Some(
            serde_json::from_value(json!({
                "prompt_tokens": 2000,
                "completion_tokens": 100,
                "total_tokens": 2100
            }))
            .unwrap(),
        ),
        cached_tokens: None,
        reasoning_tokens: None,
        latency_ms: 42,
        streaming: false,
        finish_reason: Some("stop".into()),
        flow_id: Some("0af7651916cd43dd8448eb211c80319c".into()),
        attribution_source: None,
        platform_paid: true,
        compress_metadata: None,
        brevity_metadata: None,
        compress_bytes,
        request_bytes,
        // `nasiko-llm-router::UsageRecord` gained these fields for the compact-tools/classifier
        // tracks; this test only exercises the pre-existing savings-ledger write, so both are None.
        compact_metadata: None,
        routing_metadata: None,
    }
}

async fn seed_user_and_agent(db: &PgPool) -> (Uuid, Uuid) {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role)
         VALUES ($1, $1 || '@test.local', 'x', 'admin') RETURNING id",
    )
    .bind(format!("savings-writer-{}", Uuid::new_v4()))
    .fetch_one(db)
    .await
    .unwrap();

    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, display_name, owner_id, status, version, description, url)
         VALUES ($1, $1, $2, 'running', '1.0.0', 'test', 'http://localhost:1') RETURNING id",
    )
    .bind(format!("savings-writer-agent-{}", Uuid::new_v4()))
    .bind(user)
    .fetch_one(db)
    .await
    .unwrap();

    (user, agent)
}

#[derive(sqlx::FromRow)]
struct SavedRow {
    layer: String,
    program: String,
    bytes_before: Option<i64>,
    bytes_after: Option<i64>,
    saved_input_tokens: i64,
    saved_output_tokens: i64,
    saved_cost_usd: f64,
    method: String,
    token_estimated: bool,
    flow_id: Option<String>,
    session_id: Option<String>,
}

async fn saved_rows(db: &PgPool, agent: Uuid) -> Vec<SavedRow> {
    sqlx::query_as("SELECT * FROM token_savings WHERE agent_id = $1")
        .bind(agent)
        .fetch_all(db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_compressed_call_writes_both_the_usage_row_and_its_savings_row() {
    let server = common::TestServer::start().await;
    let (user, agent) = seed_user_and_agent(&server.db).await;
    let pricing = PricingEngine::offline();

    // 40 000 bytes of tool results compressed to 10 000, on a request that went out at 8 000 bytes
    // and was billed 2 000 prompt tokens — so the calibrated ratio is exactly 4 chars/token.
    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some((40_000, 10_000)), Some(8_000)),
    )
    .await
    .unwrap();

    let usage_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM token_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(usage_rows, 1, "the usage row must still be written");

    let rows = saved_rows(&server.db, agent).await;
    assert_eq!(rows.len(), 1, "exactly one savings row per compressed call");
    let r = &rows[0];

    assert_eq!(r.layer, "compress_payload");
    assert_eq!(r.program, "caveman");
    assert_eq!(r.bytes_before, Some(40_000));
    assert_eq!(r.bytes_after, Some(10_000));
    // 30 000 bytes saved at the calibrated 4.0 chars/token.
    assert_eq!(r.saved_input_tokens, 7_500);
    assert_eq!(
        r.saved_output_tokens, 0,
        "payload compression cannot shorten what the model writes"
    );
    assert_eq!(r.method, "measured_bytes");
    assert!(
        !r.token_estimated,
        "the ratio came from this call's own reported usage, so it is not an estimate"
    );
    // The llm-router holds the flow id, not the contextId; the read path resolves the latter.
    assert_eq!(
        r.flow_id.as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    assert_eq!(r.session_id, None);
}

#[tokio::test]
#[serial]
async fn a_call_with_no_compression_writes_usage_but_no_savings_row() {
    // The zero-behaviour guard: with the layer off, the ledger must stay empty rather than fill
    // with zero rows that would dilute every average computed over it.
    let server = common::TestServer::start().await;
    let (user, agent) = seed_user_and_agent(&server.db).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, None, Some(8_000)),
    )
    .await
    .unwrap();

    let usage_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM token_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(usage_rows, 1);
    assert!(saved_rows(&server.db, agent).await.is_empty());
}

#[tokio::test]
#[serial]
async fn a_call_that_reported_no_request_size_still_saves_but_says_it_was_estimated() {
    // Dropping the saving because the handler could not measure the request would under-report the
    // window; reporting it unflagged would overstate the confidence. It must do both honestly.
    let server = common::TestServer::start().await;
    let (user, agent) = seed_user_and_agent(&server.db).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some((40_000, 10_000)), None),
    )
    .await
    .unwrap();

    let rows = saved_rows(&server.db, agent).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].saved_input_tokens, 7_500); // 30 000 / the 4.0 fallback
    assert!(rows[0].token_estimated, "an assumed ratio must say so");
}

#[tokio::test]
#[serial]
async fn the_saving_is_priced_through_the_same_engine_that_priced_the_call() {
    // A saving priced by a different rate than the spend it is subtracted from makes the reduction
    // percentage incoherent — which is the whole reason this write lives beside the usage write.
    let server = common::TestServer::start().await;
    let (user, agent) = seed_user_and_agent(&server.db).await;
    let pricing = Arc::new(PricingEngine::offline());

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some((40_000, 10_000)), Some(8_000)),
    )
    .await
    .unwrap();

    let rows = saved_rows(&server.db, agent).await;
    let cost: Option<f64> =
        sqlx::query_scalar("SELECT cost_usd::FLOAT8 FROM token_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();

    // Both are finite and non-negative, and the saving is priced on the prompt side only, so it
    // cannot exceed what the whole call cost.
    let saved = rows[0].saved_cost_usd;
    assert!(
        saved.is_finite() && saved >= 0.0,
        "saved_cost_usd = {saved}"
    );
    if let Some(c) = cost.filter(|c| *c > 0.0) {
        assert!(
            saved <= c * 1000.0,
            "a saving {saved} wildly out of scale with the call's own cost {c} means the rate came \
             from somewhere else"
        );
    }
}
