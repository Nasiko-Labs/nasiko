//! The router's live `trace_usage` write: every priced call adds to its
//! `(trace_id, agent_name)` row in the same transaction as `token_usage`, so
//! budgets read spend with no materializer lag.
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test live_trace_spend -- --test-threads=1

#[allow(dead_code)]
mod common;

use chrono::{Duration, Utc};
use nasiko_llm_router::usage::{CallFailure, UsageRecord, log_usage};
use nasiko_pricing::{NoUsageLedger, PricingEngine};
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
        compact_tools_metadata: None,
        routing_metadata: None,
        compress_bytes: None,
        compact_tools_bytes: None,
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

/// A flow's `metadata.compact_tools` adds its calls' counters together, where every other block
/// keeps the last call's: a flow that compacted its first call and skipped the rest must show
/// both, or the last skip would read as "compact schemas never ran".
#[tokio::test]
#[serial]
async fn compact_tools_counters_add_up_across_a_flows_calls() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    // The block `compact_tools::to_metadata` writes for one call.
    let call = |outcome: &str, sent: i64, brevity: &str| UsageRecord {
        compact_tools_metadata: Some(json!({
            "calls": 1, outcome: 1, "tool_bytes_sent": sent, "tool_bytes_native": 10_270,
        })),
        brevity_metadata: Some(json!({ "skipped": brevity })),
        ..record(user, agent, Some(TRACE), 100, 10)
    };
    // A call that is not a chat call (an embedding, a `/v1/responses` call) carries no block, and
    // must leave the counters the chat calls before and after it wrote.
    let not_chat = UsageRecord {
        brevity_metadata: Some(json!({ "skipped": "not_chat" })),
        ..record(user, agent, Some(TRACE), 100, 10)
    };
    for record in [
        call("applied", 7_430, "first"),
        call("skipped_tool_history", 10_270, "second"),
        not_chat,
        call("skipped_tool_history", 10_270, "third"),
    ] {
        log_usage(server.db.clone(), &pricing, &NoUsageLedger, record)
            .await
            .unwrap();
    }

    let metadata: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM trace_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        metadata["compact_tools"],
        json!({
            "calls": 3,
            "applied": 1,
            "skipped_tool_history": 2,
            "tool_bytes_sent": 27_970,
            "tool_bytes_native": 30_810,
        })
    );
    assert_eq!(
        metadata["brevity"],
        json!({ "skipped": "third" }),
        "every other block still keeps the last call's"
    );
}

/// NAS-1263: a flow's `metadata.routing` adds its calls' counters together too, so a turn that
/// classified its first call and served the rest from the decision cache reports both levels.
/// Keeping the last call's block instead would erase every classification behind a cache hit.
#[tokio::test]
#[serial]
async fn routing_counters_add_up_across_a_flows_calls() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    let pricing = PricingEngine::offline();

    // The block `RouteDecision::to_metadata` writes for one call, at the two levels a tiered turn
    // goes through: the classifier once, then the decision cache for the rest of the turn.
    let call = |level: &str, tier: &str, overridden: i64| UsageRecord {
        routing_metadata: Some(json!({
            "calls": 1, level: 1, tier: 1, "model_overridden": overridden,
        })),
        ..record(user, agent, Some(TRACE), 100, 10)
    };
    // An embedding routes no model, carries no block, and must leave the counters alone.
    let embedding = UsageRecord {
        operation_type: "embedding",
        ..record(user, agent, Some(TRACE), 100, 0)
    };
    for record in [
        call("classified", "tier2", 1),
        call("cache_hit", "tier2", 1),
        embedding,
        call("cache_hit", "tier2", 0),
    ] {
        log_usage(server.db.clone(), &pricing, &NoUsageLedger, record)
            .await
            .unwrap();
    }

    let metadata: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM trace_usage WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        metadata["routing"],
        json!({
            "calls": 3,
            "classified": 1,
            "cache_hit": 2,
            "tier2": 3,
            "model_overridden": 2,
        })
    );

    // Read the counters back the way `/finops/model-routing` does. The endpoint's own SQL only
    // ever ran here: a unit test sees the laid-out rows, never the statement, so a query that
    // Postgres refuses to parse reached a running server once (a `FILTER` inside `SUM(...)`'s
    // argument list) and was found by hand.
    let data = nasiko_server::observability::model_routing::get_model_routing(
        &server.db,
        &nasiko_server::observability::model_routing::ModelRoutingQuery {
            start: Utc::now() - Duration::hours(1),
            end: Utc::now() + Duration::hours(1),
            agent_id: Some(agent),
            provider: None,
            model: None,
            accessible_agent_ids: None,
            user_ids: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(data.calls, 3);
    assert_eq!(data.model_overridden, 2);
    assert_eq!(data.traces, 1);
    let level = |key: &str| {
        data.by_level
            .iter()
            .find(|b| b.key == key)
            .unwrap_or_else(|| panic!("no {key} row"))
    };
    assert_eq!(level("cache_hit").calls, 2);
    assert_eq!(level("classified").calls, 1);
    // A level that served nothing is still a row, so the card's order never shifts.
    assert_eq!(level("small_talk").calls, 0);
    // Both levels shared the one trace, so neither can claim its cost.
    assert_eq!(level("cache_hit").attributed_calls, 0);
    assert_eq!(level("classified").attributed_calls, 0);
    // The embedding's row carries no routing block, so it is counted as unrecorded, never as a
    // level's call. It shares the trace row here, so the window's single row is recorded.
    assert_eq!(data.unrecorded_traces, 0);
    let tier2 = data.by_tier.iter().find(|b| b.key == "tier2").unwrap();
    assert_eq!(tier2.calls, 3);
    // Every call of the trace drew tier 2, so the tier does own the trace's tokens and cost.
    assert_eq!(tier2.attributed_calls, 3);
    assert!(tier2.tokens > 0, "tier2 tokens: {}", tier2.tokens);
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
        &NoUsageLedger,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();
    log_usage(
        server.db.clone(),
        &pricing,
        &NoUsageLedger,
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
        &NoUsageLedger,
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
        &NoUsageLedger,
        record(user, agent, Some(TRACE), 100, 10),
    )
    .await
    .unwrap();
    let before = rows(&server.db, agent).await;

    // Tempo reads the same trace later with its own (different) estimate.
    let tempo = nasiko_observability::TraceUsageRow {
        trace_id: TRACE.into(),
        agent_name: name.clone(),
        agent_uuid: None,
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
        log_usage(
            server.db.clone(),
            &pricing,
            &NoUsageLedger,
            failed(user, agent, 4000),
        )
        .await
        .unwrap();
    }
    log_usage(
        server.db.clone(),
        &pricing,
        &NoUsageLedger,
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
        agent_uuid: None,
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

/// A success alone leaves `error_count` NULL ("not yet counted"), as before, and an exactly
/// priced one leaves `cost_estimated` NULL too.
#[tokio::test]
#[serial]
async fn a_successful_call_leaves_error_count_uncounted() {
    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    let model = exactly_priced_model(&server.db).await;
    log_usage(
        server.db.clone(),
        &PricingEngine::new(server.db.clone()),
        &NoUsageLedger,
        answered(user, agent, &model),
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
        &NoUsageLedger,
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
        &NoUsageLedger,
        record(user, agent, None, 40, 4),
    )
    .await
    .unwrap();
    log_usage(
        server.db.clone(),
        &pricing,
        &NoUsageLedger,
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
        &NoUsageLedger,
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
        &NoUsageLedger,
        record(user, agent, None, 40, 4),
    )
    .await
    .unwrap();

    // The turn's receipt reports that call plus one the router never saw.
    let turn = nasiko_observability::TraceUsageRow {
        trace_id: "turn-trace".into(),
        agent_name: name.clone(),
        agent_uuid: None,
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

/// A model with a synced price row, so the router prices it exactly rather than from the offline
/// list, which is itself an estimate.
async fn exactly_priced_model(db: &PgPool) -> String {
    let model = format!("exact-model-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m,
             cache_read_price_per_1m, cache_creation_price_per_1m, effective_from)
         VALUES ('test-provider', $1, 2, 10, 0.2, 2.5, '2020-01-01')",
    )
    .bind(&model)
    .execute(db)
    .await
    .unwrap();
    model
}

/// The fallback call that answered, on `model` (see [`exactly_priced_model`]).
fn answered(owner: Uuid, agent: Uuid, model: &str) -> UsageRecord {
    UsageRecord {
        provider: "test-provider".into(),
        model: model.into(),
        ..record(owner, agent, Some(TRACE), 100, 10)
    }
}

/// An attempt the provider refused with a 404 (the model does not exist there).
fn refused(owner: Uuid, agent: Uuid) -> UsageRecord {
    UsageRecord {
        provider: "openai".into(),
        model: "gpt-4o-mini".into(),
        failure: Some(CallFailure::new("status", Some(404))),
        ..failed(owner, agent, 4000)
    }
}

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Named {
    model: Option<String>,
    provider: Option<String>,
    pricing_source: Option<String>,
    failure_status: Option<i64>,
}

async fn named(db: &PgPool, agent: Uuid) -> Named {
    sqlx::query_as(
        "SELECT model, provider, metadata->'pricing'->>'source' AS pricing_source,
                (metadata->'failure'->>'http_status')::int8 AS failure_status
         FROM trace_usage WHERE agent_id = $1",
    )
    .bind(agent)
    .fetch_one(db)
    .await
    .unwrap()
}

/// Q1-1: the success and the failed attempt of one fallback chain are written by separate
/// tasks, so either can land last. Both orders must leave the row naming the model that
/// answered, priced as it was, with the refused attempt counted but not flagged as an estimate.
#[tokio::test]
#[serial]
async fn a_failed_attempt_never_replaces_the_model_that_answered() {
    for failure_lands_last in [true, false] {
        let server = common::TestServer::start().await;
        let (user, agent, _) = seed(&server.db, None, json!({})).await;
        let model = exactly_priced_model(&server.db).await;
        let pricing = PricingEngine::new(server.db.clone());
        let writes = if failure_lands_last {
            [answered(user, agent, &model), refused(user, agent)]
        } else {
            [refused(user, agent), answered(user, agent, &model)]
        };
        for write in writes {
            log_usage(server.db.clone(), &pricing, &NoUsageLedger, write)
                .await
                .unwrap();
        }

        assert_eq!(
            named(&server.db, agent).await,
            Named {
                model: Some(model.clone()),
                provider: Some("test-provider".into()),
                pricing_source: Some("Exact".into()),
                failure_status: Some(404),
            },
            "failure lands last: {failure_lands_last}"
        );
        assert_eq!(
            failures(&server.db, agent).await,
            Failures {
                error_count: Some(1),
                router_error_count: 1,
                router_estimated_input_tokens: 0,
                cost_estimated: None,
            },
            "a refused call bills nothing, so nothing is estimated (failure lands last: {failure_lands_last})"
        );
        server.cleanup().await;
    }
}

/// Q1-2: a call priced at the default rate (no price for the model anywhere) marks its row
/// estimated, and the chat summary of the turn says so.
#[tokio::test]
#[serial]
async fn a_default_priced_call_is_marked_estimated() {
    use nasiko_server::router::usage_meta::{TurnUsage, summarize_flow_usage};

    let server = common::TestServer::start().await;
    let (user, agent, _) = seed(&server.db, None, json!({})).await;
    let pricing = std::sync::Arc::new(PricingEngine::offline());
    log_usage(
        server.db.clone(),
        &pricing,
        &NoUsageLedger,
        UsageRecord {
            model: format!("unpriced-model-{}", Uuid::new_v4()),
            ..record(user, agent, Some(TRACE), 100, 10)
        },
    )
    .await
    .unwrap();

    assert_eq!(
        named(&server.db, agent).await.pricing_source.as_deref(),
        Some("Default")
    );
    assert_eq!(failures(&server.db, agent).await.cost_estimated, Some(true));

    let provider = nasiko_observability::TempoLokiProvider::new(
        "http://unused.invalid".into(),
        "http://unused.invalid".into(),
        pricing,
    );
    let summary =
        summarize_flow_usage(&server.db, &provider, TRACE, &TurnUsage::default(), 100).await;
    assert!(summary.estimated, "the chat summary must say estimated");
    server.cleanup().await;
}
