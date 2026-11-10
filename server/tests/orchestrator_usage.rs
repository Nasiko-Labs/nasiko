//! D6 accounting regressions using an isolated migrated database.
mod common;

use chrono::{TimeZone, Utc};
use nasiko_observability::TempoLokiProvider;
use nasiko_pricing::PricingEngine;
use nasiko_react_agent::CallUsage;
use nasiko_server::{
    router::usage_meta::{KeySource, PricedTurn, TurnUsage, summarize_flow_usage},
    usage::UsageTracker,
};
use serde_json::{Value, json};
use serial_test::serial;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
#[serial]
async fn priced_calls_persist_cache_split_and_match_the_chat_summary() {
    let server = common::TestServer::start().await;
    assert!(
        server
            .client
            .get(server.url("/health"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, 'usage-test', 'usage@test.local')",
    )
    .bind(user_id)
    .execute(&server.db)
    .await
    .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id) VALUES ('usage-agent', $1) RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    // Historical windows prove pricing uses call time, not insertion time.
    sqlx::query("INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m, cache_read_price_per_1m, cache_creation_price_per_1m, effective_from, effective_until) VALUES
        ('test-provider', 'model-a', 2, 10, 0.2, 2.5, '2020-01-01', '2021-01-01'),
        ('test-provider', 'model-a', 4, 20, 0.4, 5, '2021-01-01', NULL)")
        .execute(&server.db).await.unwrap();
    let engine = Arc::new(PricingEngine::new(server.db.clone()));
    let tracker = UsageTracker::new(server.db.clone());
    let provider = TempoLokiProvider::new(
        "http://unused.invalid".into(),
        "http://unused.invalid".into(),
        engine.clone(),
    );
    let mut turns = TurnUsage::default();
    let mut expected_cost = 0.0;
    for (year, attributed, streaming) in [(2020, Some(agent), true), (2022, None, false)] {
        let call = CallUsage {
            input_tokens: 764,
            output_tokens: 110,
            cache_read_tokens: 3968,
            cache_creation_tokens: 100,
            total_tokens: 4942,
            model: "model-a".into(),
            provider: Some("test-provider".into()),
            started_at: Utc.with_ymd_and_hms(year, 6, 1, 0, 0, 0).unwrap(),
            streaming,
            estimated: false,
        };
        let priced = PricedTurn::price(&engine, call).await;
        let expected = if year == 2020 { 0.003672 } else { 0.007343 };
        assert!(
            (priced.cost_usd - expected).abs() < 1e-9,
            "{}",
            priced.cost_usd
        );
        assert!(!priced.estimated);
        expected_cost += priced.cost_usd;
        turns.add(&priced);
        let key = priced
            .persist(
                &tracker,
                user_id,
                "usage-flow",
                attributed,
                KeySource::Platform,
            )
            .await
            .unwrap();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(t) FROM trace_usage t WHERE trace_id = $1 AND agent_name = $2",
        )
        .bind(&key.trace_id)
        .bind(&key.agent_name)
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(row["input_tokens"], 764);
        assert_eq!(row["cache_read_tokens"], 3968);
        assert_eq!(row["cache_creation_tokens"], 100);
        assert_eq!(row["output_tokens"], 110);
        assert_eq!(row["priced_by"], "router");
        assert_eq!(
            row["agent_id"],
            attributed.map(|id| json!(id)).unwrap_or(Value::Null)
        );
        assert_eq!(row["metadata"]["key_source"], "platform");
        assert!(row["metadata"]["pricing"].is_object());
        assert!((row["cost_usd"].as_f64().unwrap() - expected).abs() < 1e-9);
    }
    // Only the in-memory orchestrator totals belong in this summary: the two
    // orchestrator rows must not also be counted as gateway agent rows.
    let summary = summarize_flow_usage(&server.db, &provider, "usage-flow", &turns, 100).await;
    assert_eq!(summary.input_tokens, 1528);
    assert_eq!(summary.cache_read_tokens, 7936);
    assert_eq!(summary.cache_creation_tokens, 200);
    assert_eq!(summary.output_tokens, 220);
    assert_eq!(summary.to_data_part("trace")["total_tokens"], 9884);
    assert!((summary.cost_usd - expected_cost).abs() < 1e-9);
    assert!(!summary.estimated);
    // One row per (flow, agent): the attributed call and the unattributed one.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trace_usage WHERE trace_id = 'usage-flow'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(count, 2);
    server.cleanup().await;
}
