#![recursion_limit = "256"]
//! Local lakehouse driver (docs/LAKEHOUSE_LOCAL_TESTING.md). Dev only.
//!
//! `lake migrate`:         apply the OSS migrations to `DATABASE_URL` (a scratch DB for a
//!                         benchmark; the EE ones need the EE server to start once).
//! `lake backdate <days>`: shift every hot row in the tiered tables `<days>` into the past.
//! `lake tier`:            one locked tiering run now (same code path as the daily loop,
//!                         without any reader hold the server wires in), timed per table.
//! `lake status`:          hot row counts, Postgres size per tiered table, cold dir size
//!                         and `tier_batches`.
//! `lake seed --users 5 --agents 3 --sessions 200 --turns 6 --days 200
//!            [--chunk 10000] [--big-bodies 0] [--legacy 0]`:
//!                         synthetic users, agents, chat sessions + messages and, per turn, one
//!                         trace_usage row, two tool_calls, one token_savings and one
//!                         context_reports row, spread over `days`; a few settings changes per
//!                         seed user; `--legacy N` rows in `token_usage_legacy` (if it exists).
//!                         Inserted `--chunk` sessions at a time with progress, idempotent, so a
//!                         stopped run resumes. `--big-bodies P`: ~P% of tool calls and assistant
//!                         messages get bodies over the 32 KiB inline limit (sidecar datasets).
//!                         User #1 is the first existing user (you), so the UI shows the data.
//!                         Also one `flows` row per turn (flow_id = trace id, session_id = the
//!                         chat session) plus its `flow_participants` row; ~1 in 10 failed, the
//!                         last turn of every 20th session left `running` (must never tier).
//! `lake unseed`:          delete everything tagged `lake-seed` (hot rows only).
//! `lake verify [--sessions N]`: for the N oldest seeded sessions, compare Postgres flow rows
//!                         with `EventStore::load_flows_for_session` (hot + cold merge): same
//!                         flow ids, same cost totals, no duplicates. Exits non-zero on mismatch.
//!
//! Env: `DATABASE_URL`, `NASIKO_COLD_PATH` (default `./data/cold`), `S3_*` for `s3://`.

use std::time::{Duration, Instant};

use chrono::Utc;
use nasiko_event_store::{EventStore, TieringConfig, cold};

/// Seeded rows per cold dataset: `unseed` removes them from Lance too, or a reseed
/// merges stale cold copies with new hot rows (double-counted usage).
const COLD_SEED_FILTERS: &[(&str, &str)] = &[
    ("trace_usage", "agent_name LIKE 'lake-seed%'"),
    ("tool_calls", "agent_name LIKE 'lake-seed%'"),
    ("token_savings", "flow_id LIKE 'lake-seed-t%'"),
    ("context_reports", "trace_id LIKE 'lake-seed-t%'"),
    ("flows", "flow_id LIKE 'lake-seed-t%'"),
    ("flow_participants", "flow_id LIKE 'lake-seed-t%'"),
    ("chat_messages", "session_id LIKE 'lake-seed-%'"),
    ("chat_sessions", "session_id LIKE 'lake-seed-%'"),
];
use sqlx::PgPool;

/// (table, timestamp column the tiering job keys on)
const TABLES: &[(&str, &str)] = &[
    ("trace_usage", "started_at"),
    ("tool_calls", "started_at"),
    ("user_setting_changes", "changed_at"),
    ("token_savings", "created_at"),
    ("context_reports", "created_at"),
    // Chat tiers whole sessions idle for `chat_idle_days`, keyed on the newest message.
    ("chat_messages", "timestamp"),
    // `updated_at` is reset by a trigger, so only `created_at` moves.
    ("chat_sessions", "created_at"),
];

/// Retired raw call log (0079): drained to cold on the first tiering run, any age.
const LEGACY: (&str, &str) = ("token_usage_legacy", "created_at");

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.into())
}

fn config() -> TieringConfig {
    let cold_path = env("NASIKO_COLD_PATH", "./data/cold");
    let mut storage_options = std::collections::HashMap::new();
    if cold_path.starts_with("s3://") {
        let endpoint = env("S3_ENDPOINT", "http://localhost:9000");
        for (k, v) in [
            ("aws_access_key_id", env("S3_ACCESS_KEY", "nasiko")),
            ("aws_secret_access_key", env("S3_SECRET_KEY", "nasiko123")),
            ("aws_region", env("S3_REGION", "us-east-1")),
            ("allow_http", endpoint.starts_with("http://").to_string()),
            ("aws_endpoint", endpoint),
            ("aws_virtual_hosted_style_request", "false".into()),
        ] {
            storage_options.insert(k.to_string(), v);
        }
    }
    TieringConfig {
        cold_path,
        storage_options,
        ..Default::default()
    }
}

/// Shape of one `lake seed` run.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SeedParams {
    users: i32,
    agents: i32,
    sessions: i32,
    turns: i32,
    days: i32,
    /// Sessions per insert statement (one transaction each).
    chunk: i32,
    /// Percent of tool calls / assistant messages with bodies over the inline limit.
    big_bodies: i32,
    /// Rows to put in `token_usage_legacy`.
    legacy: i32,
}

impl Default for SeedParams {
    fn default() -> Self {
        Self {
            users: 5,
            agents: 3,
            sessions: 200,
            turns: 6,
            days: 200,
            chunk: 10_000,
            big_bodies: 0,
            legacy: 0,
        }
    }
}

/// Parses `--users N --agents N --sessions N --turns N --days N --chunk N --big-bodies P
/// --legacy N` (any order, all optional).
fn parse_seed(args: &[String]) -> anyhow::Result<SeedParams> {
    let mut p = SeedParams::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let v: i32 = it
            .next()
            .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))?
            .parse()?;
        let min = if matches!(flag.as_str(), "--big-bodies" | "--legacy") {
            0
        } else {
            1
        };
        anyhow::ensure!(v >= min, "{flag} must be >= {min}");
        match flag.as_str() {
            "--users" => p.users = v,
            "--agents" => p.agents = v,
            "--sessions" => p.sessions = v,
            "--turns" => p.turns = v,
            "--days" => p.days = v,
            "--chunk" => p.chunk = v,
            "--big-bodies" => {
                anyhow::ensure!(v <= 100, "--big-bodies is a percentage");
                p.big_bodies = v
            }
            "--legacy" => p.legacy = v,
            _ => anyhow::bail!("unknown flag {flag}"),
        }
    }
    Ok(p)
}

/// One row per (session s, turn t) for sessions `[$7, $8]`. Users/agents round-robin over
/// sessions; sessions are spread evenly over `days`. Every id is derived from (s, t), so
/// re-seeding is idempotent and a stopped run resumes. `$9` is the big-body percentage.
const TURNS: &str =
    "WITH p AS (SELECT $1::uuid[] u, $2::uuid[] a, $3::text[] an, $4::int ns, $5::int nt,
                       $6::int nd, $7::int lo, $8::int hi, $9::int big),
x AS (SELECT s, t,
    p.u[1 + s % cardinality(p.u)] AS user_id,
    p.a[1 + s % cardinality(p.a)] AS agent_id,
    p.an[1 + s % cardinality(p.an)] AS agent_name,
    'lake-seed-s' || s AS session_id,
    'lake-seed-t' || s || '-' || t AS trace_id,
    date_trunc('day', now()) - make_interval(days => (s::bigint * p.nd / p.ns)::int)
      + make_interval(mins => (s * 37) % 1200 + t * 2) AS ts,
    (ARRAY['gpt-4o','claude-sonnet-4','gemini-2.5-pro'])[1 + (s + t) % 3] AS model,
    (ARRAY['openai','anthropic','google'])[1 + (s + t) % 3] AS provider,
    800 + (s * 7 + t * 13) % 1500 AS tin,
    150 + (s * 11 + t * 5) % 600 AS tout,
    300 + (s * 17 + t * 29) % 2500 AS lat,
    p.big
  FROM p, generate_series(p.lo, p.hi) s, generate_series(0, p.nt - 1) t)";

/// A body over the 32 KiB inline limit (`TieringConfig::*_body_inline_max`).
const BIG: &str = "repeat('lake-seed body ', 2600)";

/// Per-chunk statements, in FK order. Each is `(table, rows written per turn, sql)`.
fn chunk_statements() -> Vec<(&'static str, String)> {
    vec![
        ("chat_sessions", format!(
            "{TURNS} INSERT INTO chat_sessions (session_id, user_id, agent_id, title, created_at, updated_at)
             SELECT session_id, user_id, agent_id, 'Seed session ' || s, ts, ts FROM x WHERE t = 0
             ON CONFLICT DO NOTHING")),
        // One flow per turn; tiering keeps a slim hot row with `tiered_at` set.
        ("flows", format!(
            "{TURNS} INSERT INTO flows (flow_id, user_id, session_id, root_agent_id, root_agent_name, title,
                 status, total_invocations, total_tokens_used, total_cost_usd, duration_ms, error_message,
                 metadata, created_at, completed_at)
               SELECT trace_id, user_id, session_id, agent_id, agent_name, 'Seed turn ' || t,
                 CASE WHEN s % 20 = 0 AND t = p.nt - 1 THEN 'running'
                      WHEN (s + t) % 10 = 0 THEN 'failed' ELSE 'completed' END,
                 1, tin + tout, tin * 0.0000025 + tout * 0.00001, lat,
                 CASE WHEN (s + t) % 10 = 0 AND NOT (s % 20 = 0 AND t = p.nt - 1) THEN 'seeded failure' END,
                 jsonb_build_object('context_id', session_id, 'lake_seed', true),
                 ts, CASE WHEN s % 20 = 0 AND t = p.nt - 1 THEN NULL
                          ELSE ts + make_interval(secs => lat / 1000 + 1) END
               FROM x, p ON CONFLICT DO NOTHING")),
        ("flow_participants", format!(
            "{TURNS} INSERT INTO flow_participants (flow_id, agent_id, created_at)
               SELECT trace_id, agent_id, ts FROM x ON CONFLICT DO NOTHING")),
        ("chat_messages", format!(
            "{TURNS} INSERT INTO chat_messages (id, session_id, role, content, timestamp, input_tokens,
               output_tokens, model, duration_ms, cost_usd, trace_id)
             SELECT md5(trace_id || '-' || r)::uuid, session_id,
               CASE r WHEN 0 THEN 'user' ELSE 'assistant' END,
               CASE WHEN r = 0 THEN 'Question ' || t || ' in session ' || s || ': how do I tier old data?'
                    WHEN abs(hashtext(trace_id || '-m')) % 100 < big THEN {BIG}
                    ELSE 'Answer ' || t || ': move rows past the hot window to Lance, keep rollups in Postgres.' END,
               ts + make_interval(secs => r * 5),
               CASE r WHEN 1 THEN tin END, CASE r WHEN 1 THEN tout END, CASE r WHEN 1 THEN model END,
               CASE r WHEN 1 THEN lat END, CASE r WHEN 1 THEN tin * 0.0000025 + tout * 0.00001 END,
               CASE r WHEN 1 THEN trace_id END
             FROM x, generate_series(0, 1) r ON CONFLICT DO NOTHING")),
        ("trace_usage", format!(
            "{TURNS} INSERT INTO trace_usage (trace_id, agent_name, session_id, agent_id, user_id, model,
               provider, input_tokens, output_tokens, cache_read_tokens, cost_usd, prompt_cost_usd,
               completion_cost_usd, latency_ms, started_at, source, priced_by)
             SELECT trace_id, agent_name, session_id, agent_id, user_id, model, provider, tin, tout,
               tin / 5, tin * 0.0000025 + tout * 0.00001, tin * 0.0000025, tout * 0.00001, lat, ts,
                (ARRAY['agent', 'agent', 'coding', 'acp'])[1 + abs(hashtext(trace_id)) % 4],
                CASE WHEN abs(hashtext(trace_id)) % 8 = 0 THEN 'tempo' ELSE 'router' END
             FROM x ON CONFLICT DO NOTHING")),
        ("tool_calls", format!(
            "{TURNS} INSERT INTO tool_calls (trace_id, call_id, user_id, agent_id, agent_name, session_id,
               tool_name, tool_kind, status, error, arguments, output, duration_ms, started_at)
             SELECT trace_id, 'c' || c, user_id, agent_id, agent_name, session_id,
               (ARRAY['read_file','grep','shell'])[1 + (s + t + c) % 3], 'function',
               CASE WHEN (s + t + c) % 9 = 0 THEN 'error' ELSE 'ok' END,
               CASE WHEN (s + t + c) % 9 = 0 THEN 'seeded failure' END,
               jsonb_build_object('path', 'src/' || t || '.rs'),
               CASE WHEN abs(hashtext(trace_id || c)) % 100 < big
                    THEN jsonb_build_object('lines', {BIG})
                    ELSE jsonb_build_object('lines', 10 + c) END,
               20 + c * 15, ts + make_interval(secs => 1 + c)
             FROM x, generate_series(0, 1) c ON CONFLICT DO NOTHING")),
        // A quarter without a model: priced at tiering through the flow's first call.
        ("token_savings", format!(
            "{TURNS} INSERT INTO token_savings (id, user_id, agent_id, flow_id, session_id, provider,
               model, layer, program, bytes_before, bytes_after, saved_input_tokens, saved_cost_usd,
               method, created_at)
             SELECT md5(trace_id || '-sv')::uuid, user_id, agent_id, trace_id, session_id,
               CASE WHEN (s + t) % 4 <> 0 THEN provider END, CASE WHEN (s + t) % 4 <> 0 THEN model END,
               'compress_payload', 'caveman', tin * 4, tin * 3, tin / 4, tin / 4 * 0.0000025,
               'measured_bytes', ts + interval '1 second'
             FROM x ON CONFLICT DO NOTHING")),
        ("context_reports", format!(
            "{TURNS} INSERT INTO context_reports (id, user_id, agent_id, session_id, trace_id, strategy,
               level, budget, pool_messages, pool_tokens, kept_messages, kept_tokens, created_at)
             SELECT md5(trace_id || '-cr')::uuid, user_id, agent_id, session_id, trace_id, 'pacms',
               'medium', 1000, 20 + t, tin * 3, 8, tin, ts
             FROM x ON CONFLICT DO NOTHING")),
    ]
}

async fn table_exists(db: &PgPool, t: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(t)
        .fetch_one(db)
        .await?)
}

fn rate(rows: u64, d: Duration) -> u64 {
    (rows as f64 / d.as_secs_f64().max(1e-3)) as u64
}

async fn seed(db: &PgPool, p: SeedParams) -> anyhow::Result<()> {
    let me: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM users ORDER BY created_at LIMIT 1")
            .fetch_optional(db)
            .await?;
    let Some(me) = me else {
        anyhow::bail!("no users yet: start the server and sign in once");
    };
    sqlx::query(
        "INSERT INTO users (username, email, display_name)
         SELECT 'lake-seed-u' || n, 'lake-seed-u' || n || '@lake-seed.local', 'Seed User ' || n
         FROM generate_series(1, $1 - 1) n ON CONFLICT DO NOTHING",
    )
    .bind(p.users)
    .execute(db)
    .await?;
    let others: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE email = ANY(SELECT 'lake-seed-u' || n || '@lake-seed.local'
           FROM generate_series(1, $1 - 1) n) ORDER BY username",
    )
    .bind(p.users)
    .fetch_all(db)
    .await?;
    let users: Vec<uuid::Uuid> = std::iter::once(me).chain(others.iter().copied()).collect();

    sqlx::query(
        "INSERT INTO agents (name, display_name, description, owner_id, status, tags, is_public)
         SELECT 'lake-seed-a' || n, 'Seed Agent ' || n, 'Synthetic agent for lakehouse testing', $1,
                'running', '{lake-seed}', true
         FROM generate_series(1, $2) n
         WHERE NOT EXISTS (SELECT 1 FROM agents WHERE name = 'lake-seed-a' || n AND deleted_at IS NULL)",
    )
    .bind(me)
    .bind(p.agents)
    .execute(db)
    .await?;
    // Seed agents forward to simulated-agent (no container, so the proxy falls back to agents.url).
    let agent_url =
        std::env::var("LAKE_AGENT_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    sqlx::query("UPDATE agents SET url = $1 WHERE name LIKE 'lake-seed-a%' AND deleted_at IS NULL")
        .bind(&agent_url)
        .execute(db)
        .await?;
    println!("seed agents -> {agent_url}");
    let agents: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM agents WHERE deleted_at IS NULL
           AND name = ANY(SELECT 'lake-seed-a' || n FROM generate_series(1, $1) n) ORDER BY name",
    )
    .bind(p.agents)
    .fetch_all(db)
    .await?;
    let (agent_ids, agent_names): (Vec<_>, Vec<_>) = agents.into_iter().unzip();
    println!(
        "users={} agents={} sessions={} turns={} days={} chunk={} big_bodies={}% legacy={}",
        users.len(),
        agent_ids.len(),
        p.sessions,
        p.turns,
        p.days,
        p.chunk,
        p.big_bodies,
        p.legacy
    );

    // Prices are looked up at each row's own time, and a fresh DB's prices start at
    // migration time: without this, every backdated model-less saving freezes as
    // unpriced. Dev DB only, like the rest of the seed: stretches the earliest price
    // row of the three seeded models back over the seeded window.
    let stretched: u64 = sqlx::query(
        "UPDATE model_pricing mp SET effective_from = now() - make_interval(days => $1 + 1)
          WHERE (mp.provider, mp.model) IN (('openai','gpt-4o'), ('anthropic','claude-sonnet-4'),
                                            ('google','gemini-2.5-pro'))
            AND mp.effective_from > now() - make_interval(days => $1 + 1)
            AND NOT EXISTS (SELECT 1 FROM model_pricing e
                             WHERE e.provider = mp.provider AND e.model = mp.model
                               AND e.effective_from < mp.effective_from)",
    )
    .bind(p.days)
    .execute(db)
    .await?
    .rows_affected();
    if stretched > 0 {
        println!(
            "model_pricing: {stretched} seeded-model price rows now start {} days back",
            p.days + 1
        );
    }

    let mut stmts: Vec<(&str, String)> = Vec::new();
    for (t, sql) in chunk_statements() {
        if table_exists(db, t).await? {
            stmts.push((t, sql));
        } else {
            println!("{t:<22} missing (run `lake migrate` or start the server once)");
        }
    }
    let mut totals: Vec<(u64, Duration)> = vec![(0, Duration::ZERO); stmts.len()];
    let started = Instant::now();
    let chunks: i32 = (p.sessions + p.chunk - 1) / p.chunk;
    for i in 0..chunks {
        let lo: i32 = i * p.chunk;
        let hi: i32 = (lo + p.chunk).min(p.sessions) - 1;
        let mut tx = db.begin().await?;
        // Dev DB only: a crash loses at most this chunk, which a re-run re-inserts.
        sqlx::query("SET LOCAL synchronous_commit = off")
            .execute(&mut *tx)
            .await?;
        for (k, (_, sql)) in stmts.iter().enumerate() {
            let t0 = Instant::now();
            let n = sqlx::query(sql)
                .bind(&users)
                .bind(&agent_ids)
                .bind(&agent_names)
                .bind(p.sessions)
                .bind(p.turns)
                .bind(p.days)
                .bind(lo)
                .bind(hi)
                .bind(p.big_bodies)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            totals[k].0 += n;
            totals[k].1 += t0.elapsed();
        }
        tx.commit().await?;
        let done = i + 1;
        let elapsed = started.elapsed();
        let eta = elapsed.mul_f64(f64::from(chunks - done) / f64::from(done));
        let rows: u64 = totals.iter().map(|(n, _)| n).sum();
        println!(
            "chunk {done}/{chunks}  sessions {lo}..={hi}  rows={rows}  {} rows/s  elapsed={:.0?}  eta={:.0?}",
            rate(rows, elapsed),
            elapsed,
            eta
        );
    }
    for ((t, _), (n, d)) in stmts.iter().zip(&totals) {
        println!("{t:<22} +{n:<10} {:>8.1?}  {} rows/s", d, rate(*n, *d));
    }

    // A few settings changes per seed user (not you): deleted with the user by unseed.
    if !others.is_empty() && table_exists(db, "user_setting_changes").await? {
        let t0 = Instant::now();
        let n = sqlx::query(
            "INSERT INTO user_setting_changes (id, user_id, field, from_value, to_value, changed_at)
             SELECT md5('lake-seed-sc-' || u || '-' || k)::uuid, u, 'strategy',
                    (ARRAY['lastk','topk','pacms'])[1 + k % 3], (ARRAY['topk','pacms','lastk'])[1 + k % 3],
                    now() - make_interval(days => k * $2 / 6)
             FROM unnest($1::uuid[]) u, generate_series(0, 5) k ON CONFLICT DO NOTHING",
        )
        .bind(&others)
        .bind(p.days)
        .execute(db)
        .await?
        .rows_affected();
        println!(
            "{:<22} +{n:<10} {:>8.1?}",
            "user_setting_changes",
            t0.elapsed()
        );
    }

    if p.legacy > 0 {
        seed_legacy(db, &users, &agent_ids, p).await?;
    }
    println!("seed finished in {:.1?}", started.elapsed());
    Ok(())
}

/// Rows in the retired `token_usage_legacy`, to time the upgrade drain. Tagged in
/// `metadata` so `unseed` finds them.
async fn seed_legacy(
    db: &PgPool,
    users: &[uuid::Uuid],
    agents: &[uuid::Uuid],
    p: SeedParams,
) -> anyhow::Result<()> {
    if !table_exists(db, LEGACY.0).await? {
        println!(
            "{:<22} missing (already drained, or migrations not run)",
            LEGACY.0
        );
        return Ok(());
    }
    let t0 = Instant::now();
    let per: i32 = p.chunk.saturating_mul(10);
    let mut total: u64 = 0;
    let mut lo: i32 = 0;
    while lo < p.legacy {
        let hi: i32 = (lo.saturating_add(per)).min(p.legacy) - 1;
        let n = sqlx::query(
            "INSERT INTO token_usage_legacy (id, user_id, agent_id, operation_type, session_id,
               provider, model, input_tokens, output_tokens, total_tokens, cost_usd, latency_ms,
               metadata, created_at)
             SELECT md5('lake-seed-lg-' || i)::uuid, ($1::uuid[])[1 + i % cardinality($1::uuid[])],
               ($2::uuid[])[1 + i % cardinality($2::uuid[])], (ARRAY['chat','chat','mcp_tool_call'])[1 + i % 3],
               'lake-seed-s' || (i % 1000), 'openai', 'gpt-4o', 900 + i % 700, 200 + i % 300,
               1100 + i % 1000, (900 + i % 700) * 0.0000025, 400 + i % 900,
               '{\"lake_seed\": true}'::jsonb,
               now() - make_interval(days => (i::bigint * $5 / $6)::int)
             FROM generate_series($3::int, $4::int) i ON CONFLICT DO NOTHING",
        )
        .bind(users)
        .bind(agents)
        .bind(lo)
        .bind(hi)
        .bind(p.days)
        .bind(p.legacy)
        .execute(db)
        .await?
        .rows_affected();
        total += n;
        lo = hi + 1;
    }
    let d = t0.elapsed();
    println!(
        "{:<22} +{total:<10} {:>8.1?}  {} rows/s",
        LEGACY.0,
        d,
        rate(total, d)
    );
    Ok(())
}

/// Hot + cold flow reads agree with Postgres for the `n` oldest seeded sessions. Postgres
/// keeps every flow row (tiered ones slim), so it is the reference for ids and costs; the
/// merged read takes tiered rows from Lance, so a lost or duplicated cold row shows up here.
async fn verify(db: &PgPool, n: i64) -> anyhow::Result<()> {
    let store = EventStore::new(db.clone(), config());
    let sessions: Vec<String> = sqlx::query_scalar(
        "SELECT session_id FROM flows WHERE flow_id LIKE 'lake-seed-t%'
         GROUP BY session_id ORDER BY min(created_at) LIMIT $1",
    )
    .bind(n)
    .fetch_all(db)
    .await?;
    anyhow::ensure!(
        !sessions.is_empty(),
        "no seeded flows: run `lake seed` first"
    );
    let (mut bad, mut cold_flows, mut checked) = (0usize, 0usize, 0usize);
    for s in &sessions {
        let pg: Vec<(String, Option<f64>, bool)> = sqlx::query_as(
            "SELECT flow_id, total_cost_usd::float8, tiered_at IS NOT NULL FROM flows
             WHERE session_id = $1 ORDER BY flow_id",
        )
        .bind(s)
        .fetch_all(db)
        .await?;
        let merged = store.load_flows_for_session(s).await?;
        // Usage rows must be unique per (trace, agent) across hot + cold.
        let flow_ids: Vec<String> = pg.iter().map(|r| r.0.clone()).collect();
        let usage = store.load_trace_usage_for_session(s, &flow_ids).await?;
        let mut keys: Vec<(String, String)> = usage
            .iter()
            .map(|u| (u.trace_id.clone(), u.agent_name.clone()))
            .collect();
        keys.sort();
        let usage_dupes = keys.windows(2).filter(|w| w[0] == w[1]).count();
        let mut ids: Vec<&str> = merged.iter().map(|f| f.flow_id.as_str()).collect();
        ids.sort();
        let dupes = ids.windows(2).filter(|w| w[0] == w[1]).count();
        ids.dedup();
        let want: Vec<&str> = pg.iter().map(|r| r.0.as_str()).collect();
        let pg_cost: f64 = pg.iter().filter_map(|r| r.1).sum();
        let merged_cost: f64 = merged.iter().filter_map(|f| f.total_cost_usd).sum();
        let tiered = pg.iter().filter(|r| r.2).count();
        cold_flows += tiered;
        checked += pg.len();
        // A tiered flow must also load alone, with its full metadata back from Lance.
        let mut single_ok = true;
        if let Some((fid, _, _)) = pg.iter().find(|r| r.2) {
            single_ok = store
                .load_flow(fid)
                .await?
                .is_some_and(|f| f.metadata != "{}");
        }
        if usage_dupes > 0 {
            println!("MISMATCH {s}: {usage_dupes} duplicate trace_usage rows (trace, agent)");
        }
        if usage_dupes > 0
            || ids != want
            || dupes > 0
            || (pg_cost - merged_cost).abs() > 1e-6
            || !single_ok
        {
            bad += 1;
            println!(
                "MISMATCH {s}: pg={} merged={} dupes={dupes} tiered={tiered} cost pg={pg_cost:.6} merged={merged_cost:.6} single_cold_ok={single_ok}",
                want.len(),
                ids.len()
            );
        }
    }
    println!(
        "verify: sessions={} flows={checked} tiered={cold_flows} mismatches={bad}",
        sessions.len()
    );
    anyhow::ensure!(
        bad == 0,
        "{bad} sessions disagree between Postgres and hot+cold reads"
    );
    if cold_flows == 0 {
        println!("note: no tiered flows checked; run `just lake-cycle 120` first");
    }
    Ok(())
}

/// Bytes under a local cold root; `None` for `s3://` (check the bucket instead).
fn dir_size(path: &std::path::Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return Some(meta.len());
    }
    let mut total: u64 = 0;
    for entry in std::fs::read_dir(path).ok()? {
        total += dir_size(&entry.ok()?.path()).unwrap_or(0);
    }
    Some(total)
}

fn mib(bytes: i64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

/// Hot row count per tiered table (the legacy log included while it exists).
async fn hot_counts(db: &PgPool) -> anyhow::Result<Vec<(&'static str, i64)>> {
    let mut out = Vec::new();
    for (t, _) in TABLES.iter().chain(std::iter::once(&LEGACY)) {
        if table_exists(db, t).await? {
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {t}"))
                .fetch_one(db)
                .await?;
            out.push((*t, n));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_seed_defaults_and_flags() {
        assert_eq!(parse_seed(&[]).unwrap(), SeedParams::default());
        let p = parse_seed(&a(&["--turns", "3", "--users", "10"])).unwrap();
        assert_eq!((p.users, p.turns, p.agents), (10, 3, 3));
        let p = parse_seed(&a(&[
            "--chunk",
            "500",
            "--big-bodies",
            "0",
            "--legacy",
            "7",
        ]))
        .unwrap();
        assert_eq!((p.chunk, p.big_bodies, p.legacy), (500, 0, 7));
        assert!(parse_seed(&a(&["--users"])).is_err());
        assert!(parse_seed(&a(&["--users", "0"])).is_err());
        assert!(parse_seed(&a(&["--big-bodies", "101"])).is_err());
        assert!(parse_seed(&a(&["--bogus", "1"])).is_err());
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let db = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    if args.first().map(String::as_str) == Some("migrate") {
        // What the server does at boot (extensions first), OSS migrations only.
        for ext in ["pgcrypto", "pg_trgm", "vector"] {
            sqlx::query(&format!("CREATE EXTENSION IF NOT EXISTS \"{ext}\""))
                .execute(&db)
                .await?;
        }
        sqlx::migrate!("../migrations")
            .set_ignore_missing(true)
            .run(&db)
            .await?;
        println!("OSS migrations applied");
        return Ok(());
    }
    let (name, port, users): (String, Option<i32>, i64) = sqlx::query_as(
        "SELECT current_database()::text, inet_server_port(), (SELECT count(*) FROM users)",
    )
    .fetch_one(&db)
    .await?;
    let first: Option<String> =
        sqlx::query_scalar("SELECT email::text FROM users ORDER BY created_at LIMIT 1")
            .fetch_optional(&db)
            .await
            .unwrap_or(None);
    eprintln!(
        "db={name} port={port:?} users={users} seed_user={}",
        first.unwrap_or("-".into())
    );
    match args.first().map(String::as_str) {
        Some("backdate") => {
            let days: i64 = args.get(1).map(|d| d.parse()).transpose()?.unwrap_or(120);
            if table_exists(&db, "flows").await? {
                let n = sqlx::query(
                    "UPDATE flows SET created_at = created_at - make_interval(days => $1::int),
                       completed_at = completed_at - make_interval(days => $1::int)",
                )
                .bind(days as i32)
                .execute(&db)
                .await?
                .rows_affected();
                println!("flows: {n} rows moved back {days} days");
            }
            for (t, col) in TABLES {
                let sql = format!("UPDATE {t} SET {col} = {col} - make_interval(days => $1::int)");
                let n = sqlx::query(&sql)
                    .bind(days as i32)
                    .execute(&db)
                    .await?
                    .rows_affected();
                println!("{t}: {n} rows moved back {days} days");
            }
        }
        Some("seed") => {
            seed(&db, parse_seed(&args[1..])?).await?;
        }
        Some("verify") => {
            let n: i64 = match args.get(1).map(String::as_str) {
                Some("--sessions") => args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(50),
                _ => 50,
            };
            verify(&db, n).await?;
        }
        Some("unseed") => {
            let mut stmts: Vec<&str> = vec![
                "DELETE FROM trace_usage WHERE agent_name LIKE 'lake-seed%'",
                "DELETE FROM tool_calls WHERE agent_name LIKE 'lake-seed%'",
                "DELETE FROM token_savings WHERE flow_id LIKE 'lake-seed-t%'",
                "DELETE FROM context_reports WHERE trace_id LIKE 'lake-seed-t%'",
                // Cascades flow_participants and flow_steps.
                "DELETE FROM flows WHERE flow_id LIKE 'lake-seed-t%'",
                "DELETE FROM chat_sessions WHERE session_id LIKE 'lake-seed-%'",
                "DELETE FROM agents WHERE name LIKE 'lake-seed-a%'",
                // Cascades the seed users' settings changes.
                "DELETE FROM users WHERE email LIKE '%@lake-seed.local'",
            ];
            if table_exists(&db, LEGACY.0).await? {
                stmts.insert(
                    0,
                    "DELETE FROM token_usage_legacy WHERE metadata->>'lake_seed' = 'true'",
                );
            }
            for sql in stmts {
                let n = sqlx::query(sql).execute(&db).await?.rows_affected();
                println!("{n:<8} {sql}");
            }
            let cfg = config();
            for (t, filter) in COLD_SEED_FILTERS {
                let loc = cold::Loc {
                    uri: cold::dataset_uri(&cfg.cold_path, t),
                    opts: cfg.storage_options.clone(),
                };
                cold::delete(loc, filter).await?;
                println!("cold     {t}: deleted where {filter}");
            }
        }
        Some("tier") => {
            // TierReport counts rows over all tables; per-table figures come from the
            // hot counts on either side of the run.
            let before = hot_counts(&db).await?;
            let t0 = Instant::now();
            let report = EventStore::new(db.clone(), config())
                .run_once(Utc::now())
                .await?;
            let elapsed = t0.elapsed();
            let after = hot_counts(&db).await?;
            match report {
                Some(r) => {
                    println!("tiered: {r:?}");
                    for (t, b) in &before {
                        let a: i64 = after.iter().find(|(n, _)| n == t).map_or(0, |(_, a)| *a);
                        println!("  {t:<22} hot {b:>10} -> {a:<10} moved {}", b - a);
                    }
                    println!(
                        "tier finished in {elapsed:.1?}  {} rows/s",
                        rate(r.rows, elapsed)
                    );
                    anyhow::ensure!(r.failed.is_empty(), "tables failed: {:?}", r.failed);
                }
                None => println!("tiering lock held by another process (is a server running?)"),
            }
        }
        Some("status") | None => {
            for (t, col) in TABLES.iter().chain(std::iter::once(&LEGACY)) {
                if !table_exists(&db, t).await? {
                    let why = if *t == LEGACY.0 {
                        "gone (drained, or a fresh install's first run dropped it)"
                    } else {
                        "missing (run `lake migrate` or start the server once)"
                    };
                    println!("{t:<22} {why}");
                    continue;
                }
                let (n, oldest, bytes): (i64, Option<chrono::DateTime<Utc>>, i64) = sqlx::query_as(
                    &format!("SELECT count(*), min({col}), pg_total_relation_size('{t}') FROM {t}"),
                )
                .fetch_one(&db)
                .await?;
                println!(
                    "{t:<22} hot={n:<10} pg={:<12} oldest={}",
                    mib(bytes),
                    oldest.map(|o| o.to_rfc3339()).unwrap_or("-".into())
                );
            }
            if table_exists(&db, "flows").await? {
                let (all, tiered, running, oldest): (i64, i64, i64, Option<chrono::DateTime<Utc>>) =
                    sqlx::query_as(
                        "SELECT count(*), count(tiered_at), count(*) FILTER (WHERE status = 'running'),
                                min(completed_at) FILTER (WHERE tiered_at IS NULL) FROM flows",
                    )
                    .fetch_one(&db)
                    .await?;
                println!(
                    "{:<22} rows={all:<10} tiered(slim)={tiered:<8} untiered={:<8} running={running} oldest_untiered={}",
                    "flows",
                    all - tiered,
                    oldest.map(|o| o.to_rfc3339()).unwrap_or("-".into())
                );
            }
            let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
                "SELECT table_name::text, state::text, count(*), COALESCE(sum(row_count), 0)::bigint
                 FROM tier_batches GROUP BY 1, 2 ORDER BY 1, 2",
            )
            .fetch_all(&db)
            .await
            .unwrap_or_default();
            for (t, s, n, r) in rows {
                println!("tier_batches {t:<22} {s:<8} batches={n:<6} rows={r}");
            }
            let cold = config().cold_path;
            match dir_size(std::path::Path::new(cold.trim_start_matches("file://"))) {
                Some(b) => println!("cold {cold}: {}", mib(b as i64)),
                None => println!("cold {cold}: size not read (s3 or missing)"),
            }
        }
        Some(other) => anyhow::bail!(
            "unknown command {other}: use migrate | seed [--users N --agents N --sessions N --turns N --days N \
             --chunk N --big-bodies P --legacy N] | unseed | backdate <days> | tier | status | verify [--sessions N]"
        ),
    }
    Ok(())
}
