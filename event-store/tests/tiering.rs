#![recursion_limit = "256"]
//! Tiering job against a real Postgres (fresh DB per test, all oss migrations).
//! Needs Postgres at `TEST_PG_URL` (default the infra compose one).

use chrono::Utc;
use nasiko_event_store::{EventStore, TieringConfig, cold};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

async fn fresh_db() -> PgPool {
    let admin = admin_url();
    let name = format!("tiering_{}", Uuid::new_v4().simple());
    let mut c = PgConnection::connect(&admin).await.expect("postgres up?");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&mut c)
        .await
        .unwrap();
    let base = admin.rsplit_once('/').unwrap().0;
    let pool = PgPool::connect(&format!("{base}/{name}")).await.unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();
    pool
}

async fn user(db: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(id.to_string())
        .bind(format!("{id}@t"))
        .execute(db)
        .await
        .unwrap();
    id
}

fn store(db: &PgPool, dir: &tempfile::TempDir) -> EventStore {
    let cfg = TieringConfig {
        cold_path: dir.path().to_str().unwrap().into(),
        batch_rows: 3, // force several batches
        ..Default::default()
    };
    EventStore::new(db.clone(), cfg)
}

async fn scalar(db: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
}

async fn trace(db: &PgPool, user: Uuid, trace_id: &str, days_ago: i64, tools: i64) {
    trace_src(db, user, trace_id, days_ago, tools, "agent").await
}

async fn trace_src(
    db: &PgPool,
    user: Uuid,
    trace_id: &str,
    days_ago: i64,
    tools: i64,
    source: &str,
) {
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, model, input_tokens,
             output_tokens, cost_usd, tool_call_count, started_at, source)
         VALUES ($1, 'agent-a', $2, 'gpt-4o-mini', 10, 5, 0.01, $3,
                 now() - make_interval(days => $4::int), $5)",
    )
    .bind(trace_id)
    .bind(user)
    .bind(tools)
    .bind(days_ago as i32)
    .bind(source)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn tiers_trace_usage_and_keeps_tool_counts() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    for i in 0..4 {
        trace(&db, u, &format!("old{i}"), 100, 2).await;
    }
    trace_src(&db, u, "old4", 100, 2, "coding").await;
    trace(&db, u, "new0", 1, 3).await;
    // Inside the 90-day trace window: stays hot even though it is past 30 days.
    trace(&db, u, "mid0", 40, 1).await;
    let totals = "SELECT (sum(traces) * 1000 + sum(tool_call_count))::bigint FROM trace_spend";
    let before = scalar(&db, totals).await;

    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_usage").await, 2);
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*) FROM trace_usage WHERE trace_id = 'mid0'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(tool_call_count)::bigint FROM trace_usage_daily"
        )
        .await,
        10
    );
    assert_eq!(scalar(&db, totals).await, before);
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "trace_usage");
    assert_eq!(
        cold::count(loc.as_str(), "row_key LIKE 'old%'")
            .await
            .unwrap(),
        5
    );
    assert_eq!(
        cold::count(loc.as_str(), "source = 'coding'")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(tool_call_count)::bigint FROM trace_usage_daily WHERE source = 'coding'"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*) FROM trace_totals WHERE trace_id = 'old4' AND source = 'coding'"
        )
        .await,
        1
    );

    store(&db, &dir).erase_user(u).await.unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM trace_usage_daily").await,
        0
    );
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 0);
}

async fn saving(db: &PgPool, user: Uuid, days_ago: i64, tokens: i64, model: Option<&str>) {
    sqlx::query(
        "INSERT INTO token_savings (user_id, flow_id, provider, model, layer, program,
             saved_input_tokens, saved_cost_usd, method, created_at)
         VALUES ($1, 'f1', $2, $3, 'compress_payload', 'caveman', $4, $5, 'measured_bytes',
                 now() - make_interval(days => $6::int))",
    )
    .bind(user)
    .bind(model.map(|_| "openai"))
    .bind(model)
    .bind(tokens)
    .bind(if model.is_some() { 0.25 } else { 0.0 })
    .bind(days_ago as i32)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn tiers_token_savings_with_frozen_cost() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    for _ in 0..3 {
        saving(&db, u, 100, 10, Some("gpt-4o")).await;
    }
    saving(&db, u, 100, 7, None).await; // unpriced: no flow call
    saving(&db, u, 40, 5, None).await; // inside 90-day window
    let totals = "SELECT (sum(rows) * 1000000 + sum(saved_input_tokens) * 1000
                  + sum(unpriced_tokens) + round(sum(priced_cost_usd) * 100))::bigint
                  FROM savings_spend";
    let before = scalar(&db, totals).await;

    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(scalar(&db, "SELECT count(*) FROM token_savings").await, 1);
    assert_eq!(
        scalar(&db, "SELECT sum(rows)::bigint FROM token_savings_daily").await,
        4
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(unpriced_tokens)::bigint FROM token_savings_daily"
        )
        .await,
        7
    );
    assert_eq!(scalar(&db, totals).await, before);
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "token_savings");
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 4);

    store(&db, &dir).erase_user(u).await.unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM token_savings_daily").await,
        0
    );
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 0);
}

async fn report(db: &PgPool, user: Uuid, days_ago: i64, trace: &str) {
    sqlx::query(
        "INSERT INTO context_reports (user_id, trace_id, strategy, pool_messages, pool_tokens,
             kept_messages, kept_tokens, compressed_bytes_saved, created_at)
         VALUES ($1, $2, 'pacms', 10, 1000, 4, 400, 5, now() - make_interval(days => $3::int))",
    )
    .bind(user)
    .bind(trace)
    .bind(days_ago as i32)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tiers_context_reports_with_frozen_rollup() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    for i in 0..4 {
        report(&db, u, 100, &format!("t{i}")).await;
    }
    report(&db, u, 40, "recent").await; // inside 90-day window

    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(scalar(&db, "SELECT count(*) FROM context_reports").await, 1);
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(reports)::bigint FROM context_reports_daily"
        )
        .await,
        4
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(pool_tokens)::bigint FROM context_reports_daily"
        )
        .await,
        4000
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(messages_dropped)::bigint FROM context_reports_daily"
        )
        .await,
        24
    );
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "context_reports");
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 4);

    store(&db, &dir).erase_user(u).await.unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM context_reports_daily").await,
        0
    );
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn tiering_fills_savings_flows() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    for _ in 0..3 {
        saving(&db, u, 100, 10, Some("gpt-4o")).await;
    }
    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM token_savings").await, 0);
    assert_eq!(scalar(&db, "SELECT count(*) FROM savings_flows").await, 1);
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(saved_input_tokens)::bigint FROM savings_flows"
        )
        .await,
        30
    );
    store(&db, &dir).erase_user(u).await.unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM savings_flows").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn tiering_fills_trace_totals() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, session_id, user_id, model, provider,
             input_tokens, output_tokens, cost_usd, started_at)
         VALUES ('tr1', 'a', 's1', $1, 'm', 'p', 10, 5, 0.5, now() - interval '100 days'),
                ('tr1', 'b', 's1', $1, 'm', 'p', 1, 2, 0.1, now() - interval '100 days')",
    )
    .bind(u)
    .execute(&db)
    .await
    .unwrap();
    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_usage").await, 0);
    assert_eq!(
        scalar(
            &db,
            "SELECT tokens FROM trace_totals WHERE session_id = 's1'"
        )
        .await,
        18
    );
    // One per agent row, as session turns counted them while hot.
    assert_eq!(
        scalar(
            &db,
            "SELECT row_count FROM trace_totals WHERE session_id = 's1'"
        )
        .await,
        2
    );
    store(&db, &dir).erase_user(u).await.unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_totals").await, 0);
}

async fn tool_call(db: &PgPool, user: Uuid, call: &str, days_ago: i64) {
    sqlx::query(
        "INSERT INTO tool_calls (trace_id, call_id, user_id, agent_name, session_id, tool_name,
             status, arguments, duration_ms, started_at)
         VALUES ('t1', $1, $2, 'claude', 's1', 'Bash', 'succeeded', '{\"cmd\":\"ls\"}'::jsonb, 5,
                 now() - make_interval(days => $3::int))",
    )
    .bind(call)
    .bind(user)
    .bind(days_ago as i32)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn tiers_tool_calls_and_reads_them_back() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    for (c, d) in [("a", 100), ("b/x", 120), ("c", 95), ("d", 10)] {
        tool_call(&db, u, c, d).await;
    }
    let s = store(&db, &dir);
    s.run_once(Utc::now()).await.unwrap().unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM tool_calls").await, 1);
    let cold = s.session_tool_calls("s1").await.unwrap();
    let ids: Vec<&str> = cold.iter().map(|r| r.call_id.as_str()).collect();
    assert_eq!(ids, ["b/x", "a", "c"]);
    assert_eq!(cold[0].arguments.as_deref(), Some("{\"cmd\": \"ls\"}"));
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*) FROM tier_batches WHERE table_name='tool_calls' AND state<>'live'"
        )
        .await,
        0
    );
    s.erase_user(u).await.unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM tool_calls").await, 0);
    assert!(s.session_tool_calls("s1").await.unwrap().is_empty());
}

#[tokio::test]
async fn tiers_large_tool_call_bodies_to_sidecar() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    tool_call(&db, u, "small", 100).await;
    tool_call(&db, u, "big", 100).await;
    let big_out: String = "y".repeat(4096);
    sqlx::query("UPDATE tool_calls SET output = to_jsonb($1::text) WHERE call_id = 'big'")
        .bind(&big_out)
        .execute(&db)
        .await
        .unwrap();
    let cfg = TieringConfig {
        cold_path: dir.path().to_str().unwrap().into(),
        tool_body_inline_max: 1024,
        ..Default::default()
    };
    let s = EventStore::new(db.clone(), cfg);
    s.run_once(Utc::now()).await.unwrap().unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM tool_calls").await, 0);
    let bodies = cold::Loc {
        uri: cold::dataset_uri(dir.path().to_str().unwrap(), "tool_call_bodies"),
        opts: Default::default(),
    };
    let body_rows: usize = cold::count(bodies, "row_key IS NOT NULL").await.unwrap();
    assert_eq!(body_rows, 1);
    let rows = s.session_tool_calls_all("s1").await.unwrap();
    let big = rows.iter().find(|r| r.call_id == "big").unwrap();
    assert_eq!(
        big.output.as_deref(),
        Some(format!("\"{big_out}\"").as_str())
    );
    assert_eq!(big.arguments.as_deref(), Some("{\"cmd\": \"ls\"}"));
    let small = rows.iter().find(|r| r.call_id == "small").unwrap();
    assert_eq!(small.output, None);
    s.erase_user(u).await.unwrap();
    assert!(s.session_tool_calls("s1").await.unwrap().is_empty());
}

/// A price row only these tests use, valid since long before any test row.
async fn test_price(db: &PgPool) {
    sqlx::query(
        "INSERT INTO model_pricing (provider, model, input_price_per_1m, output_price_per_1m,
             effective_from)
         VALUES ('testprov', 'testmodel', 2.0, 8.0, '2000-01-01')",
    )
    .execute(db)
    .await
    .unwrap();
}

/// Savings and context reports that tier after their trace: price, session and agent
/// still resolve through `trace_totals` (live view first, then the frozen rollups).
#[tokio::test(flavor = "multi_thread")]
async fn dependents_tiered_after_their_trace_keep_price_and_session() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    test_price(&db).await;
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, session_id, user_id, model, provider,
             input_tokens, output_tokens, cost_usd, started_at)
         VALUES ('f1', 'a', 's1', $1, 'testmodel', 'testprov', 10, 5, 0.5,
                 now() - interval '100 days')",
    )
    .bind(u)
    .execute(&db)
    .await
    .unwrap();
    // Model-less saving and a report on that flow, still inside the 90-day window.
    saving(&db, u, 80, 1_000_000, None).await;
    report(&db, u, 80, "f1").await;

    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_usage").await, 0);
    assert_eq!(scalar(&db, "SELECT count(*) FROM token_savings").await, 1);
    // Live view: priced at the tiered trace's first call (1M tokens x $2 / 1M).
    let live = "SELECT round(sum(priced_cost_usd) * 100)::bigint FROM savings_spend";
    assert_eq!(scalar(&db, live).await, 200);

    sqlx::query("UPDATE token_savings SET created_at = created_at - interval '20 days'")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE context_reports SET created_at = created_at - interval '20 days'")
        .execute(&db)
        .await
        .unwrap();
    store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scalar(&db, "SELECT count(*) FROM token_savings").await, 0);
    assert_eq!(scalar(&db, live).await, 200);
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(unpriced_tokens)::bigint FROM token_savings_daily"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*) FROM savings_flows WHERE session_id = 's1'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT sum(priced_reports)::bigint FROM context_reports_daily"
        )
        .await,
        1
    );
}

/// A live write that lands while a batch is being tiered waits for it and is then in
/// the Lance copy and the rollups alike, never only in one or lost between them.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_racing_a_trace_batch_lands_in_both_copies() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    trace(&db, u, "busy", 100, 0).await; // cost 0.01

    // An uncommitted live write holds the row.
    let mut writer = db.begin().await.unwrap();
    sqlx::query("UPDATE trace_usage SET cost_usd = cost_usd + 1 WHERE trace_id = 'busy'")
        .execute(&mut *writer)
        .await
        .unwrap();
    let tiering = {
        let s = store(&db, &dir);
        tokio::spawn(async move { s.run_once(Utc::now()).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!tiering.is_finished(), "tiering must wait for the row lock");
    writer.commit().await.unwrap();
    tiering.await.unwrap().unwrap().unwrap();

    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_usage").await, 0);
    let rolled = "SELECT round(sum(cost_usd) * 100)::bigint FROM trace_usage_daily";
    assert_eq!(scalar(&db, rolled).await, 101);
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "trace_usage");
    assert_eq!(
        cold::count(loc.as_str(), "cost_usd > 1.0").await.unwrap(),
        1
    );
}

/// A batch that cannot be settled holds back only its own table: the other tables
/// tier, the run reports the failure, and an erasure clears everything else while
/// staying pending.
#[tokio::test(flavor = "multi_thread")]
async fn a_stuck_batch_holds_back_only_its_table() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let u = user(&db).await;
    trace(&db, u, "old0", 100, 0).await;
    tool_call(&db, u, "c1", 100).await;
    // A `writing` tool_calls batch whose dataset cannot be read.
    sqlx::query(
        "INSERT INTO tier_batches (table_name, batch_id, row_count, min_ts, max_ts, state)
         VALUES ('tool_calls', $1, 1, now(), now(), 'writing')",
    )
    .bind(Uuid::new_v4())
    .execute(&db)
    .await
    .unwrap();
    std::fs::write(dir.path().join("tool_calls.lance"), b"not a dataset").unwrap();

    let r = store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.failed, vec!["tool_calls".to_string()]);
    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_usage").await, 0);
    assert_eq!(scalar(&db, "SELECT count(*) FROM tool_calls").await, 1);

    assert!(store(&db, &dir).erase_user(u).await.is_err());
    assert_eq!(scalar(&db, "SELECT count(*) FROM trace_totals").await, 0);
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "trace_usage");
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 0);
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM erasure_requests").await,
        1
    );
}

async fn legacy_call(db: &PgPool, user: Uuid, days_ago: i64, op: &str) {
    sqlx::query(
        "INSERT INTO token_usage_legacy (id, user_id, operation_type, provider, model,
             input_tokens, output_tokens, total_tokens, cost_usd, created_at)
         VALUES ($1, $2, $3, 'openai', 'gpt-4o', 10, 5, 15, 0.001, now() - make_interval(days => $4))",
    )
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(op)
    .bind(days_ago as i32)
    .execute(db)
    .await
    .unwrap();
}

async fn legacy_exists(db: &PgPool) -> bool {
    sqlx::query_scalar("SELECT to_regclass('token_usage_legacy') IS NOT NULL")
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn legacy_token_usage_drains_to_cold_and_is_dropped() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (user(&db).await, user(&db).await);
    // Any age: the drain moves recent history too (batch_rows = 3 → several batches).
    for (u, days) in [(a, 400), (a, 2), (a, 0), (b, 30), (b, 1)] {
        legacy_call(&db, u, days, "chat").await;
    }
    legacy_call(&db, a, 5, "mcp_tool_call").await;

    let r = store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert!(!legacy_exists(&db).await, "drained legacy table is dropped");
    let loc = cold::dataset_uri(dir.path().to_str().unwrap(), "token_usage");
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 6);
    let mcp = "operation_type = 'mcp_tool_call'";
    assert_eq!(cold::count(loc.as_str(), mcp).await.unwrap(), 1);

    // A second run is a no-op on the missing table.
    let again = store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert!(again.failed.is_empty(), "{:?}", again.failed);

    // Erasure reaches the drained rows.
    assert!(store(&db, &dir).erase_user(a).await.unwrap());
    let of_a = format!("user_id = '{a}'");
    assert_eq!(cold::count(loc.as_str(), &of_a).await.unwrap(), 0);
    assert_eq!(cold::count(loc.as_str(), "true").await.unwrap(), 2);
}

#[tokio::test]
async fn erasure_reaches_legacy_rows_not_yet_drained() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (user(&db).await, user(&db).await);
    legacy_call(&db, a, 3, "chat").await;
    legacy_call(&db, b, 3, "chat").await;
    store(&db, &dir).erase_hot(a).await.unwrap();
    assert_eq!(
        scalar(&db, "SELECT count(*) FROM token_usage_legacy").await,
        1
    );
}

#[tokio::test]
async fn fresh_install_drops_the_empty_legacy_table() {
    let db = fresh_db().await;
    let dir = tempfile::tempdir().unwrap();
    assert!(legacy_exists(&db).await, "0079 renames token_usage");
    let r = store(&db, &dir)
        .run_once(Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(r.rows, 0);
    assert!(!legacy_exists(&db).await);
    // No dataset is created for an empty drain.
    assert!(!dir.path().join("token_usage.lance").exists());
}
