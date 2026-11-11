//! The router's live `trace_usage` write: every priced call adds to its
//! `(trace_id, agent_name)` row in the same transaction as `token_usage`, so
//! budgets read spend with no materializer lag.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test live_trace_spend -- --test-threads=1

#[allow(dead_code)]
mod common;

use nasiko_llm_router::usage::{CallFailure, UsageRecord, log_usage};
use nasiko_pricing::PricingEngine;
use serde_json::json;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const TRACE: &str = "0af7651916cd43dd8448eb211c80319c";

fn record(owner: Uuid, agent: Uuid, flow_id: Option<&str>, input: i64, output: i64) -> UsageRecord {
    UsageRecord {
        owner_id: owner.to_string(),
        agent_id: agent.to_string(),
        operation_type: "direct_llm",
        provider: "openai".into(),
        model: "gpt-4o".into(),
        usage: Some(
            serde_json::from_value(json!({
                "prompt_tokens": input,
                "completion_tokens": output,
                "total_tokens": input + output
            }))
            .unwrap(),
        ),
        cached_tokens: None,
        reasoning_tokens: None,
        latency_ms: 10,
        streaming: false,
        finish_reason: Some("stop".into()),
        flow_id: flow_id.map(str::to_string),
        attribution_source: None,
        platform_paid: true,
        compress_metadata: None,
        brevity_metadata: None,
        compress_bytes: None,
        request_bytes: None,
        failure: None,
    }
}

/// A call that reached the provider and failed: no usage reported, `request_bytes` sent.
fn failed(owner: Uuid, agent: Uuid, request_bytes: usize) -> UsageRecord {
    UsageRecord {
        usage: None,
        finish_reason: Some("failed:status".into()),
        request_bytes: Some(request_bytes),
        failure: Some(CallFailure::new("status", Some(503))),
        ..record(owner, agent, Some(TRACE), 0, 0)
    }
}

/// A user and an agent. `workload_kind` / `metadata` pick the row's `source`.
async fn seed(
    db: &PgPool,
    workload_kind: Option<&str>,
    metadata: serde_json::Value,
) -> (Uuid, Uuid, String) {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash, role)
         VALUES ($1, $1 || '@test.local', 'x', 'admin') RETURNING id",
    )
    .bind(format!("live-spend-{}", Uuid::new_v4()))
    .fetch_one(db)
    .await
    .unwrap();
    let name = format!("live-spend-agent-{}", Uuid::new_v4());
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, display_name, owner_id, status, version, description, url, metadata)
         VALUES ($1, $1, $2, 'running', '1.0.0', 'test', 'http://localhost:1', $3) RETURNING id",
    )
    .bind(&name)
    .bind(user)
    .bind(metadata)
    .fetch_one(db)
    .await
    .unwrap();
    if let Some(kind) = workload_kind {
        sqlx::query("UPDATE agents SET workload_kind = $1 WHERE id = $2")
            .bind(kind)
            .bind(agent)
            .execute(db)
            .await
            .unwrap();
    }
    (user, agent, name)
}

#[derive(sqlx::FromRow, Debug)]
struct Row {
    trace_id: String,
    agent_name: String,
    source: String,
    priced_by: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cost_usd: Option<f64>,
}

async fn rows(db: &PgPool, agent: Uuid) -> Vec<Row> {
    sqlx::query_as(
        "SELECT trace_id, agent_name, source, priced_by,
                input_tokens::int8 AS input_tokens, output_tokens::int8 AS output_tokens,
                cost_usd::float8 AS cost_usd
         FROM trace_usage WHERE agent_id = $1 ORDER BY trace_id",
    )
    .bind(agent)
    .fetch_all(db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn router_calls_accumulate_on_one_trace_row() {
    let server = common::TestServer::start().await;
    let (user, agent, name) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();
    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 200, 20),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 1, "two calls on one trace make one row: {r:?}");
    assert_eq!(r[0].trace_id, TRACE);
    assert_eq!(r[0].agent_name, name);
    assert_eq!(
        (r[0].source.as_str(), r[0].priced_by.as_str()),
        ("agent", "router")
    );
    assert_eq!(
        (r[0].input_tokens, r[0].output_tokens),
        (Some(300), Some(30))
    );

    let billed: f64 = sqlx::query_scalar(
        // `token_usage` is gone (0079); the row's router share is the sum of its calls.
        "SELECT COALESCE(SUM(router_cost_usd), 0)::float8 FROM trace_usage WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(
        (r[0].cost_usd.unwrap_or(0.0) - billed).abs() < 1e-9,
        "row cost equals the sum of calls"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn router_spend_adds_to_tempo_spend_and_keeps_source() {
    let server = common::TestServer::start().await;
    let (user, agent, name) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    // The Tempo materializer got there first with its own (estimated) spend.
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, agent_id, user_id, model, provider,
                                  input_tokens, output_tokens, cost_usd, started_at,
                                  source, priced_by, materialized_at)
         VALUES ($1, $2, $3, $4, 'gpt-4o', 'openai', 999, 999, 9.99, now(), 'agent', 'tempo', now())",
    )
    .bind(TRACE)
    .bind(&name)
    .bind(agent)
    .bind(user)
    .execute(&server.db)
    .await
    .unwrap();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].priced_by, "router");
    assert_eq!(
        (r[0].input_tokens, r[0].output_tokens),
        (Some(1099), Some(1009)),
        "the agent's own-key calls Tempo already counted are kept; the routed call is added"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn tempo_pass_keeps_router_spend() {
    use nasiko_server::observability::trace_materializer::{UsageSource, write_usage_row};

    let server = common::TestServer::start().await;
    let (user, agent, name) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();
    let before = rows(&server.db, agent).await;

    // Tempo reads the same trace later with its own (different) estimate.
    let tempo = nasiko_observability::TraceUsageRow {
        trace_id: TRACE.into(),
        agent_name: name.clone(),
        session_id: None,
        model: Some("other-model".into()),
        provider: Some("other".into()),
        input_tokens: 999,
        output_tokens: 999,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        tool_call_count: 3,
        cost_estimated: true,
        cost_usd: 9.99,
        prompt_cost_usd: 5.0,
        completion_cost_usd: 4.99,
        latency_ms: Some(1234),
        started_at: chrono::Utc::now(),
        error_count: 1,
        tool_calls: vec![],
    };
    let mut conn = server.db.acquire().await.unwrap();
    write_usage_row(
        &mut conn,
        &tempo,
        UsageSource::Agent,
        None,
        Some(agent),
        Some(user),
    )
    .await
    .unwrap();
    drop(conn);

    let after = rows(&server.db, agent).await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].priced_by, "router");
    assert_eq!(
        (after[0].input_tokens, after[0].output_tokens),
        (Some(999), Some(999)),
        "Tempo saw more than the router: the remainder is own-key spend and is kept"
    );
    assert_eq!(after[0].cost_usd, Some(9.99));

    // A lagging Tempo pass that saw less than the router never lowers its figure.
    let lagging = nasiko_observability::TraceUsageRow {
        input_tokens: 1,
        output_tokens: 1,
        cost_usd: 0.0,
        prompt_cost_usd: 0.0,
        completion_cost_usd: 0.0,
        ..tempo
    };
    let mut conn = server.db.acquire().await.unwrap();
    write_usage_row(
        &mut conn,
        &lagging,
        UsageSource::Agent,
        None,
        Some(agent),
        Some(user),
    )
    .await
    .unwrap();
    drop(conn);
    let lagged = rows(&server.db, agent).await;
    assert_eq!(
        (lagged[0].input_tokens, lagged[0].output_tokens),
        (Some(100), Some(10)),
        "tempo must not lower router spend"
    );
    assert_eq!(lagged[0].cost_usd, before[0].cost_usd);

    let (latency, errors): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT latency_ms::int8, error_count::int8 FROM trace_usage WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        (latency, errors),
        (Some(1234), Some(1)),
        "tempo still fills latency and errors"
    );
    server.cleanup().await;
}

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Failures {
    error_count: Option<i64>,
    router_error_count: i64,
    router_estimated_input_tokens: i64,
    cost_estimated: Option<bool>,
}

async fn failures(db: &PgPool, agent: Uuid) -> Failures {
    sqlx::query_as(
        "SELECT error_count::int8 AS error_count, router_error_count::int8 AS router_error_count,
                router_estimated_input_tokens, cost_estimated
         FROM trace_usage WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(db)
    .await
    .unwrap()
}

/// NAS-1018: a failed provider call is counted on its trace row. Its unreported prompt is
/// estimated apart from the spend columns and never priced; a success beside it is unchanged.
#[tokio::test]
#[serial]
async fn failed_calls_count_as_errors_without_inventing_spend() {
    use nasiko_server::observability::trace_materializer::{UsageSource, write_usage_row};

    let server = common::TestServer::start().await;
    let (user, agent, name) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    // Two failed attempts (4000 bytes ≈ 1000 tokens each), then the fallback that succeeded.
    for _ in 0..2 {
        log_usage(server.db.clone(), &pricing, failed(user, agent, 4000))
            .await
            .unwrap();
    }
    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(
        (r[0].input_tokens, r[0].output_tokens),
        (Some(100), Some(10)),
        "only reported tokens are spend"
    );
    let success_only: f64 =
        sqlx::query_scalar("SELECT router_cost_usd::float8 FROM trace_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        r[0].cost_usd,
        Some(success_only),
        "failed calls add no cost"
    );
    assert_eq!(
        failures(&server.db, agent).await,
        Failures {
            error_count: Some(2),
            router_error_count: 2,
            router_estimated_input_tokens: 2000,
            cost_estimated: Some(true),
        }
    );
    let failure: serde_json::Value =
        sqlx::query_scalar("SELECT metadata->'failure' FROM trace_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(failure["class"], "status");
    assert_eq!(failure["http_status"], 503);
    assert_eq!(failure["tokens_estimated"], true);

    // A later Tempo pass that saw no error span must not erase the router's count.
    let tempo = nasiko_observability::TraceUsageRow {
        trace_id: TRACE.into(),
        agent_name: name,
        session_id: None,
        model: Some("gpt-4o".into()),
        provider: Some("openai".into()),
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        tool_call_count: 0,
        cost_estimated: false,
        cost_usd: 0.0,
        prompt_cost_usd: 0.0,
        completion_cost_usd: 0.0,
        latency_ms: Some(5),
        started_at: chrono::Utc::now(),
        error_count: 0,
        tool_calls: vec![],
    };
    let mut conn = server.db.acquire().await.unwrap();
    write_usage_row(
        &mut conn,
        &tempo,
        UsageSource::Agent,
        None,
        Some(agent),
        Some(user),
    )
    .await
    .unwrap();
    drop(conn);
    let after = failures(&server.db, agent).await;
    assert_eq!(after.error_count, Some(2), "router-booked failures survive");
    assert_eq!(
        after.cost_estimated,
        Some(true),
        "the estimate stays flagged"
    );
    server.cleanup().await;
}

/// A success alone leaves `error_count` NULL ("not yet counted"), as before.
#[tokio::test]
#[serial]
async fn a_successful_call_leaves_error_count_uncounted() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    log_usage(
        server.db.clone(),
        &PricingEngine::offline(),
        record(user, agent, Some(TRACE), 10, 1),
    )
    .await
    .unwrap();
    assert_eq!(
        failures(&server.db, agent).await,
        Failures {
            error_count: None,
            router_error_count: 0,
            router_estimated_input_tokens: 0,
            cost_estimated: None,
        }
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn acp_agent_call_writes_one_acp_row() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, Some("acp_coding_agent"), json!({})).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 50, 5),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].source.as_str(), r[0].priced_by.as_str()),
        ("acp", "router")
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn local_coding_agent_without_traceparent_gets_direct_row() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, None, 40, 4),
    )
    .await
    .unwrap();
    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, None, 40, 4),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 2, "each untraced call is its own direct: row");
    assert!(
        r.iter()
            .all(|x| x.trace_id.starts_with("direct:") && x.source == "coding"),
        "{r:?}"
    );
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn owner_editable_metadata_cannot_label_spend_coding() {
    let server = common::TestServer::start().await;
    // `metadata` is editable through the catalog; only the server-managed
    // `coding_agent_integration_id` may exempt spend as `coding`.
    let (user, agent, _) = seed(&server.db, None, json!({"integration_id": "claude"})).await;
    let pricing = PricingEngine::offline();

    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, Some(TRACE), 40, 4),
    )
    .await
    .unwrap();

    let r = rows(&server.db, agent).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].source, "agent");
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn coding_turn_row_nets_out_calls_the_router_booked() {
    use nasiko_server::observability::trace_materializer::{UsageSource, write_usage_row};

    let server = common::TestServer::start().await;
    let (user, agent, name) = seed(&server.db, None, json!({})).await;
    sqlx::query("UPDATE agents SET coding_agent_integration_id = 'claude' WHERE id = $1")
        .bind(agent)
        .execute(&server.db)
        .await
        .unwrap();
    let pricing = PricingEngine::offline();
    // The turn's one routed call, booked live on a `direct:` row.
    log_usage(
        server.db.clone(),
        &pricing,
        record(user, agent, None, 40, 4),
    )
    .await
    .unwrap();

    // The turn's receipt reports that call plus one the router never saw.
    let turn = nasiko_observability::TraceUsageRow {
        trace_id: "turn-trace".into(),
        agent_name: name.clone(),
        session_id: None,
        model: None,
        provider: None,
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        tool_call_count: 0,
        cost_estimated: true,
        cost_usd: 1.0,
        prompt_cost_usd: 0.5,
        completion_cost_usd: 0.5,
        latency_ms: Some(2_000),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(1),
        error_count: 0,
        tool_calls: vec![],
    };
    let mut conn = server.db.acquire().await.unwrap();
    write_usage_row(
        &mut conn,
        &turn,
        UsageSource::Coding,
        None,
        Some(agent),
        Some(user),
    )
    .await
    .unwrap();
    drop(conn);

    let r = rows(&server.db, agent).await;
    let turn_row = r.iter().find(|x| x.trace_id == "turn-trace").unwrap();
    assert_eq!(
        (turn_row.input_tokens, turn_row.output_tokens),
        (Some(60), Some(6)),
        "only the unrouted remainder of the turn is counted again"
    );
    server.cleanup().await;
}
