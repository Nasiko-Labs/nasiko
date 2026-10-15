//! Background worker that materializes Tempo trace data into `trace_usage`.
//!
//! Periodically queries Tempo for recent user-query traces, extracts FinOps
//! metrics (tokens, model, cost, latency), and upserts them into Postgres.
//! The FinOps dashboard and spend endpoints then query `trace_usage` instead
//! of hitting Tempo live on every request.
//!
//! Each trace produces one row **per agent** that has token-bearing spans,
//! keyed by `(trace_id, agent_name)`. Multi-agent traces are correctly
//! split so each agent's spend is attributed independently.
//!
//! **Coding-agent direct calls** (Claude Code, Codex, OpenCode) bypass the
//! A2A proxy entirely and have no OTel traces. They are picked up by a
//! second pass (`sync_direct_token_usage`) that reads `token_usage` rows for
//! coding agents and upserts them with a synthetic `direct:<id>` trace key.
//!
//! Pattern: same as `hours_meter` / catalog-sync — a polling loop with
//! `MissedTickBehavior::Skip`, fail-soft per tick, never panics.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nasiko_observability::provider::SessionIdResolver;
use nasiko_observability::{ObservabilityProvider, TraceUsageRow};
use sqlx::PgPool;
use tokio::time::MissedTickBehavior;

/// Prefix applied to the synthetic `trace_id` for direct coding-agent calls.
/// Kept short and unambiguous: real Tempo trace ids are 32 lowercase hex
/// characters and never contain a colon.
const DIRECT_TRACE_PREFIX: &str = "direct:";

/// Run the trace-usage materializer loop. Exits only on shutdown (task drop).
pub async fn run(
    db: PgPool,
    provider: Arc<dyn ObservabilityProvider>,
    session_resolver: Arc<dyn SessionIdResolver>,
    interval: Duration,
    overlap: Duration,
    batch_size: usize,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Skip the immediate first tick to avoid startup load (same pattern as
    // the materialized-view refresh in state.rs).
    tick.tick().await;

    loop {
        tick.tick().await;
        if let Err(error) = super::receipt_materializer::materialize_once(
            &db,
            provider.as_ref(),
            session_resolver.as_ref(),
        )
        .await
        {
            tracing::warn!(%error, "receipt materialization failed");
        }
        if let Err(e) = materialize_once(
            &db,
            provider.as_ref(),
            session_resolver.as_ref(),
            overlap,
            batch_size,
        )
        .await
        {
            tracing::warn!(error = %e, "trace_materializer: pass failed");
        }
        if let Err(e) = sync_direct_token_usage(&db).await {
            tracing::warn!(error = %e, "trace_materializer: direct-usage sync failed");
        }
    }
}

async fn materialize_once(
    db: &PgPool,
    provider: &dyn ObservabilityProvider,
    session_resolver: &dyn SessionIdResolver,
    overlap: Duration,
    batch_size: usize,
) -> Result<(), String> {
    let start = std::time::Instant::now();

    // 1. Read high-water mark.
    let high_water: DateTime<Utc> =
        sqlx::query_scalar("SELECT high_water FROM trace_usage_cursor WHERE id = 1")
            .fetch_one(db)
            .await
            .map_err(|e| format!("cursor read: {e}"))?;

    let now = Utc::now();
    let query_start =
        high_water - chrono::Duration::from_std(overlap).unwrap_or(chrono::Duration::seconds(600));

    // 2. Search Tempo for user-query traces in [query_start, now].
    let mut trace_ids: Vec<String> = provider
        .search_user_traces(query_start, now, 1000)
        .await
        .map_err(|e| format!("search: {e}"))?;

    // 3. Union with session_traces DB index (uninstrumented agents).
    let indexed: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT trace_id FROM session_traces \
         WHERE created_at >= $1 AND created_at <= $2 \
         ORDER BY trace_id \
         LIMIT 1000",
    )
    .bind(query_start)
    .bind(now)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut seen: HashSet<String> = trace_ids.iter().cloned().collect();
    for id in indexed {
        if seen.insert(id.clone()) {
            trace_ids.push(id);
        }
    }

    if trace_ids.is_empty() {
        tracing::debug!("trace_materializer: no traces in window");
        return Ok(());
    }

    // 4. Fetch + extract each trace (8-in-flight bounded concurrency).
    //    Each trace may produce multiple rows (one per agent).
    let mut rows: Vec<TraceUsageRow> = Vec::new();
    let mut errors = 0usize;

    for chunk in trace_ids.chunks(batch_size.max(1)) {
        let fetches = chunk.iter().map(|id| provider.extract_trace_usage(id));
        for result in futures::future::join_all(fetches).await {
            match result {
                Ok(agent_rows) => rows.extend(agent_rows),
                Err(e) => {
                    errors += 1;
                    tracing::debug!(error = %e, "trace_materializer: trace fetch failed");
                }
            }
        }
    }

    if rows.is_empty() {
        tracing::debug!(
            traces_found = trace_ids.len(),
            errors,
            "trace_materializer: no token-bearing traces"
        );
        return Ok(());
    }

    let mut upserted = 0usize;
    for row in &rows {
        upsert_row(db, session_resolver, row).await?;
        upserted += 1;
    }

    // Advance only when every fetch and write succeeded.
    if errors == 0
        && let Some(max_ts) = rows.iter().map(|r| r.started_at).max()
    {
        sqlx::query("UPDATE trace_usage_cursor SET high_water = GREATEST(high_water, $1), updated_at = now() WHERE id = 1")
            .bind(max_ts).execute(db).await.map_err(|e| e.to_string())?;
    }
    tracing::info!(
        traces_found = trace_ids.len(),
        agent_rows = rows.len(),
        upserted,
        errors,
        elapsed_ms = start.elapsed().as_millis() as u64,
        "trace_materializer: pass complete"
    );
    Ok(())
}

/// Shared idempotent write path for window scans and receipt-driven recovery.
pub(crate) async fn upsert_row(
    db: &PgPool,
    session_resolver: &dyn SessionIdResolver,
    row: &TraceUsageRow,
) -> Result<(), String> {
    // Resolve agent_id from agent_name.
    let agent_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM agents WHERE name = $1 AND deleted_at IS NULL LIMIT 1")
            .bind(&row.agent_name)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    // Backfill session_id from the DB index if the span didn't carry it.
    let session_id = match &row.session_id {
        Some(s) => Some(s.clone()),
        None => session_resolver.session_for_trace(&row.trace_id).await,
    };

    // Resolve user_id from the chat_session that owns this session_id.
    let user_id: Option<uuid::Uuid> = match &session_id {
        Some(sid) => {
            sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE session_id = $1 LIMIT 1")
                .bind(sid)
                .fetch_optional(db)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    // A price-book match is not evidence of who served this request.
    let provider = &row.provider;

    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    // Upsert into trace_usage (composite PK: trace_id, agent_name).
    let result = sqlx::query(
            r#"INSERT INTO trace_usage (
                   trace_id, agent_name, session_id, agent_id, user_id,
                   model, provider,
                   input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                   tool_call_count,
                   cost_usd, prompt_cost_usd, completion_cost_usd,
                   latency_ms, started_at, materialized_at
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, now())
               ON CONFLICT (trace_id, agent_name) DO UPDATE SET
                   session_id = COALESCE(EXCLUDED.session_id, trace_usage.session_id),
                   agent_id = COALESCE(EXCLUDED.agent_id, trace_usage.agent_id),
                   user_id = COALESCE(EXCLUDED.user_id, trace_usage.user_id),
                   model = EXCLUDED.model,
                   provider = EXCLUDED.provider,
                   input_tokens = EXCLUDED.input_tokens,
                   output_tokens = EXCLUDED.output_tokens,
                   cache_read_tokens = EXCLUDED.cache_read_tokens,
                   cache_creation_tokens = EXCLUDED.cache_creation_tokens,
                   tool_call_count = EXCLUDED.tool_call_count,
                   cost_usd = EXCLUDED.cost_usd,
                   prompt_cost_usd = EXCLUDED.prompt_cost_usd,
                   completion_cost_usd = EXCLUDED.completion_cost_usd,
                   latency_ms = EXCLUDED.latency_ms,
                   started_at = EXCLUDED.started_at,
                   materialized_at = now()"#,
        )
        .bind(&row.trace_id)
        .bind(&row.agent_name)
        .bind(&session_id)
        .bind(agent_id)
        .bind(user_id)
        .bind(&row.model)
        .bind(provider)
        .bind(row.input_tokens as i64)
        .bind(row.output_tokens as i64)
        .bind(row.cache_read_tokens as i64)
        .bind(row.cache_creation_tokens as i64)
        .bind(row.tool_call_count as i64)
        .bind(row.cost_usd)
        .bind(row.prompt_cost_usd)
        .bind(row.completion_cost_usd)
        .bind(row.latency_ms)
        .bind(row.started_at)
        .execute(&mut *tx)
        .await;

    result.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE trace_usage SET cost_estimated=$3 WHERE trace_id=$1 AND agent_name=$2")
        .bind(&row.trace_id)
        .bind(&row.agent_name)
        .bind(row.cost_estimated)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Sync `token_usage` rows for coding agents (Claude Code, Codex, OpenCode)
/// that have no OTel trace and therefore no `trace_usage` entry.
///
/// These agents call `/v1/responses` or `/v1/chat/completions` directly,
/// bypassing the A2A proxy entirely, so `session.id` is never set on any span
/// and `session_traces` never receives a mapping. The Tempo pass misses them
/// entirely. This pass bridges the gap: it reads every unseen coding-agent
/// `token_usage` row and upserts a `trace_usage` row with a synthetic
/// `direct:<token_usage.id>` trace key.
///
/// The synthetic key is stable across reruns (it is the UUID primary key of
/// the source row), so the `ON CONFLICT … DO UPDATE` in `upsert_row` makes
/// this idempotent: running it twice produces one row, not two.
async fn sync_direct_token_usage(db: &PgPool) -> Result<(), String> {
    // Read coding-agent token_usage rows not yet in trace_usage.
    // `is_coding_agent` is not a column on `token_usage`; we join on
    // `agents.metadata->>'integration_id' IS NOT NULL` — every coding-agent
    // row is registered with an `integration_id` by `nasiko connect`.
    #[derive(sqlx::FromRow)]
    struct Row {
        id: uuid::Uuid,
        agent_id: uuid::Uuid,
        agent_name: String,
        user_id: uuid::Uuid,
        model: String,
        provider: String,
        input_tokens: i64,
        output_tokens: i64,
        cache_read: i64,
        cache_creation: i64,
        cost_usd: f64,
        latency_ms: Option<i64>,
        created_at: DateTime<Utc>,
    }

    let rows: Vec<Row> = sqlx::query_as(
        r#"SELECT
               tu.id,
               tu.agent_id,
               a.name AS agent_name,
               tu.user_id,
               tu.model,
               tu.provider,
               tu.input_tokens::BIGINT  AS input_tokens,
               tu.output_tokens::BIGINT AS output_tokens,
               COALESCE(tu.cache_read_input_tokens, 0)::BIGINT AS cache_read,
               COALESCE(tu.cache_creation_input_tokens, 0)::BIGINT AS cache_creation,
               COALESCE(tu.cost_usd, 0)::FLOAT8 AS cost_usd,
               tu.latency_ms::BIGINT AS latency_ms,
               tu.created_at
           FROM token_usage tu
           JOIN agents a ON a.id = tu.agent_id
                        AND a.deleted_at IS NULL
                        AND a.metadata->>'integration_id' IS NOT NULL
           WHERE NOT EXISTS (
               SELECT 1 FROM trace_usage
               WHERE trace_id = $1 || tu.id::TEXT
                 AND agent_name = a.name
           )
           ORDER BY tu.created_at
           LIMIT 500"#,
    )
    .bind(DIRECT_TRACE_PREFIX)
    .fetch_all(db)
    .await
    .map_err(|e| format!("direct-usage read: {e}"))?;

    if rows.is_empty() {
        return Ok(());
    }

    let count = rows.len();
    for row in rows {
        let trace_id = format!("{DIRECT_TRACE_PREFIX}{}", row.id);
        // Prompt vs completion split: `token_usage` stores `cost_usd` as the
        // total; prompt/completion breakdown requires the pricing engine.
        // We store zeros for the split and the full total in `cost_usd`,
        // matching what the dashboard actually uses (it sums `cost_usd`).
        let result = sqlx::query(
            r#"INSERT INTO trace_usage (
                   trace_id, agent_name, agent_id, user_id,
                   model, provider,
                   input_tokens, output_tokens,
                   cache_read_tokens, cache_creation_tokens,
                   cost_usd, prompt_cost_usd, completion_cost_usd,
                   latency_ms, started_at, materialized_at
               ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0,$12,$13,now())
               ON CONFLICT (trace_id, agent_name) DO UPDATE SET
                   model       = EXCLUDED.model,
                   provider    = EXCLUDED.provider,
                   input_tokens  = EXCLUDED.input_tokens,
                   output_tokens = EXCLUDED.output_tokens,
                   cache_read_tokens    = EXCLUDED.cache_read_tokens,
                   cache_creation_tokens = EXCLUDED.cache_creation_tokens,
                   cost_usd    = EXCLUDED.cost_usd,
                   latency_ms  = EXCLUDED.latency_ms,
                   started_at  = EXCLUDED.started_at,
                   materialized_at = now()"#,
        )
        .bind(&trace_id)
        .bind(&row.agent_name)
        .bind(row.agent_id)
        .bind(row.user_id)
        .bind(&row.model)
        .bind(&row.provider)
        .bind(row.input_tokens)
        .bind(row.output_tokens)
        .bind(row.cache_read)
        .bind(row.cache_creation)
        .bind(row.cost_usd)
        .bind(row.latency_ms)
        .bind(row.created_at)
        .execute(db)
        .await;

        if let Err(e) = result {
            tracing::warn!(
                %trace_id,
                agent = %row.agent_name,
                error = %e,
                "direct-usage upsert failed"
            );
        }
    }

    tracing::info!(count, "trace_materializer: direct-usage sync complete");
    Ok(())
}
