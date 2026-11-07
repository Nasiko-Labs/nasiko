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
//! **Spend** is written live by the LLM router (`priced_by='router'`); this
//! pass only fills latency, errors and tool counts on those rows, and prices
//! rows the router never saw (`priced_by='tempo'`).
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

    // 3. Union with the server's `flows` rows (flow_id = trace_id; covers uninstrumented agents).
    let indexed: Vec<String> = sqlx::query_scalar(
        "SELECT flow_id FROM flows \
         WHERE created_at >= $1 AND created_at <= $2 \
         ORDER BY flow_id \
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

    upsert_rows(db, session_resolver, &rows).await?;
    let upserted = rows.len();

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
///
/// Rows are merged by the agent they resolve to before anything is written: the
/// gateway attributes a tool call to the agent's UUID while the agent's own spans
/// carry its name, and both resolve to the same `(trace_id, agent_name)` key,
/// where a plain upsert of each would overwrite the other's tokens or tool count.
pub(crate) async fn upsert_rows(
    db: &PgPool,
    session_resolver: &dyn SessionIdResolver,
    rows: &[TraceUsageRow],
) -> Result<(), String> {
    let mut merged: Vec<(Option<uuid::Uuid>, TraceUsageRow)> = Vec::new();
    for row in rows {
        let (agent_id, agent_name) = resolve_agent(db, &row.agent_name).await;
        let row = TraceUsageRow {
            agent_name,
            ..row.clone()
        };
        match merged
            .iter_mut()
            .find(|(_, m)| m.trace_id == row.trace_id && m.agent_name == row.agent_name)
        {
            Some((id, existing)) => {
                *id = id.or(agent_id);
                merge_usage_rows(existing, &row);
            }
            None => merged.push((agent_id, row)),
        }
    }
    for (agent_id, row) in &merged {
        write_resolved_row(db, session_resolver, row, *agent_id).await?;
    }
    Ok(())
}

/// Fold `other`, a row for the same agent in the same trace, into `into`.
fn merge_usage_rows(into: &mut TraceUsageRow, other: &TraceUsageRow) {
    into.input_tokens += other.input_tokens;
    into.output_tokens += other.output_tokens;
    into.cache_read_tokens += other.cache_read_tokens;
    into.cache_creation_tokens += other.cache_creation_tokens;
    into.tool_call_count += other.tool_call_count;
    into.error_count += other.error_count;
    into.cost_usd += other.cost_usd;
    into.prompt_cost_usd += other.prompt_cost_usd;
    into.completion_cost_usd += other.completion_cost_usd;
    into.cost_estimated |= other.cost_estimated;
    // A row with no tokens (tool calls only) has no model of its own to disagree with.
    let other_has_usage = other.input_tokens + other.output_tokens > 0;
    if into.model.is_none() && other_has_usage {
        into.model = other.model.clone();
    }
    if into.provider.is_none() && other_has_usage {
        into.provider = other.provider.clone();
    }
    into.session_id = into.session_id.take().or_else(|| other.session_id.clone());
}

/// Resolve what the provider called the agent (a name, or the UUID the router
/// stamps) to its id and name.
async fn resolve_agent(db: &PgPool, reported: &str) -> (Option<uuid::Uuid>, String) {
    let by_name: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM agents WHERE name = $1 AND deleted_at IS NULL LIMIT 1")
            .bind(reported)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    if let Some(id) = by_name {
        return (Some(id), reported.to_string());
    }
    let Ok(uuid) = uuid::Uuid::parse_str(reported) else {
        return (None, reported.to_string());
    };
    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM agents WHERE id = $1 AND deleted_at IS NULL LIMIT 1")
            .bind(uuid)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match name {
        Some(name) => (Some(uuid), name),
        None => (None, reported.to_string()),
    }
}

/// Write one merged row with its owners resolved.
async fn write_resolved_row(
    db: &PgPool,
    session_resolver: &dyn SessionIdResolver,
    row: &TraceUsageRow,
    agent_id: Option<uuid::Uuid>,
) -> Result<(), String> {
    // Backfill session_id from the DB index if the span didn't carry it.
    let session_id = match &row.session_id {
        Some(s) => Some(s.clone()),
        None => session_resolver.session_for_trace(&row.trace_id).await,
    };

    // Resolve user_id from the chat_session that owns this session_id.
    let session_user: Option<uuid::Uuid> = match &session_id {
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
    // A turn sent through the API or an A2A client has no UI chat session, but every entry
    // path opens a flow keyed by the trace id, and the flow knows who sent it.
    let user_id = match session_user {
        Some(user) => Some(user),
        None => sqlx::query_scalar("SELECT user_id FROM flows WHERE flow_id = $1")
            .bind(&row.trace_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten(),
    };
    // Tool calls carry bodies (arguments, output), so they are filed only under
    // the session the server's records tie this trace to, never under a span's
    // unverified `session.id` claim, and attributed to that session's owner.
    let tool_session: Option<String> = if row.tool_calls.is_empty() {
        None
    } else {
        super::session_resolver::canonical_session(db, &row.trace_id, session_id.as_deref())
            .await
            .map_err(|e| e.to_string())?
    };
    let tool_user: Option<uuid::Uuid> = if tool_session == session_id {
        user_id
    } else {
        match &tool_session {
            Some(sid) => sqlx::query_scalar(
                "SELECT user_id FROM chat_sessions WHERE session_id = $1 LIMIT 1",
            )
            .bind(sid)
            .fetch_optional(db)
            .await
            .map_err(|e| e.to_string())?,
            None => None,
        }
    };
    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    let written = write_usage_row(
        &mut tx,
        row,
        UsageSource::Agent,
        session_id.as_deref(),
        agent_id,
        user_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    // A tiered trace's tool calls were tiered with it: re-inserting them would
    // put a second copy in hot storage beside the cold one.
    let tool_calls: &[_] = if written { &row.tool_calls } else { &[] };
    for call in tool_calls {
        sqlx::query(
            "INSERT INTO tool_calls (trace_id, call_id, user_id, agent_id, agent_name, session_id,
                 tool_name, status, error, arguments, output, duration_ms, started_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,to_jsonb($10::text),to_jsonb($11::text),$12,$13)
             ON CONFLICT (trace_id, call_id) DO UPDATE SET
                 status = EXCLUDED.status,
                 -- A span may carry less than ingest stored (coding agents write
                 -- full bodies at ingest): never replace a body with nothing.
                 error = COALESCE(EXCLUDED.error, tool_calls.error),
                 arguments = COALESCE(EXCLUDED.arguments, tool_calls.arguments),
                 output = COALESCE(EXCLUDED.output, tool_calls.output),
                 duration_ms = COALESCE(EXCLUDED.duration_ms, tool_calls.duration_ms)",
        )
        .bind(&row.trace_id)
        .bind(&call.call_id)
        .bind(tool_user)
        .bind(agent_id)
        .bind(&row.agent_name)
        .bind(tool_session.as_deref())
        .bind(&call.tool_name)
        .bind(&call.status)
        .bind(&call.error)
        .bind(&call.arguments)
        .bind(&call.output)
        .bind(call.duration_ms)
        .bind(call.started_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Producer of a `trace_usage` row; stored in `trace_usage.source` so budgets
/// and dashboards can split spend by origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageSource {
    /// Materialized from Tempo spans of a deployed agent.
    Agent,
    /// Coding agent (Claude Code, Codex, OpenCode) via telemetry or router sync.
    Coding,
}

impl UsageSource {
    /// Value of the `trace_usage.source` column (CHECK-constrained in 0080).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            UsageSource::Agent => "agent",
            UsageSource::Coding => "coding",
        }
    }
}

/// Upsert one `trace_usage` row with its owners already resolved, inside the
/// caller's transaction. The single write path for every producer: the Tempo
/// window scan and receipt read-back above, and coding-agent ingest, which
/// writes a turn's row in the same transaction as its receipt. Keyed
/// `(trace_id, agent_name)`, so a second producer of the same trace replaces the
/// row rather than adding to it.
///
/// Returns `false`, writing nothing, for a key whose trace was already tiered
/// (rolled into `trace_totals`) and has no hot row: this row would be the full
/// trace total again, on top of the copy the rollups already hold. Router calls
/// still add to a tiered trace (as a new hot row holding only their increment);
/// only a producer that re-totals the trace is refused.
///
/// `started_at` is set on insert and never changed: the budget ledger keys each
/// hot row by it, so a row's start must not move under the copy already taken.
#[doc(hidden)]
pub async fn write_usage_row(
    conn: &mut sqlx::PgConnection,
    row: &TraceUsageRow,
    source: UsageSource,
    session_id: Option<&str>,
    agent_id: Option<uuid::Uuid>,
    user_id: Option<uuid::Uuid>,
) -> Result<bool, sqlx::Error> {
    let tiered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM trace_totals WHERE trace_id = $1)
            AND NOT EXISTS (SELECT 1 FROM trace_usage WHERE trace_id = $1 AND agent_name = $2)",
    )
    .bind(&row.trace_id)
    .bind(&row.agent_name)
    .fetch_one(&mut *conn)
    .await?;
    if tiered {
        tracing::debug!(
            trace_id = %row.trace_id,
            agent = %row.agent_name,
            "trace already tiered: not re-totalling it"
        );
        return Ok(false);
    }
    // A coding agent's turn row must not re-count calls the router already booked.
    // Here, not at ingest only, so a Tempo rescan of the turn's trace nets them too.
    let netted;
    let row = match agent_id {
        Some(agent) => {
            match crate::coding_agent_usage::routed_share_within(conn, agent, row).await? {
                Some(routed) => {
                    netted = crate::coding_agent_usage::net_of_routed(row, &routed);
                    &netted
                }
                None => row,
            }
        }
        None => row,
    };
    // A price-book match is not evidence of who served this request.
    let provider = &row.provider;
    let count = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    sqlx::query(
            r#"INSERT INTO trace_usage (
                   trace_id, agent_name, session_id, agent_id, user_id,
                   model, provider,
                   input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                   tool_call_count,
                   cost_usd, prompt_cost_usd, completion_cost_usd,
                   latency_ms, started_at, error_count, source, materialized_at
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, now())
               ON CONFLICT (trace_id, agent_name) DO UPDATE SET
                   -- Spend: Tempo's aggregate covers routed and own-key calls alike,
                   -- and the router's share (`router_*`, 0080) is the routed part
                   -- priced live. GREATEST keeps the router's figure while Tempo lags
                   -- and adds the own-key remainder once Tempo has seen more.
                   source = CASE WHEN EXCLUDED.source = 'agent' THEN trace_usage.source ELSE EXCLUDED.source END,
                   session_id = COALESCE(EXCLUDED.session_id, trace_usage.session_id),
                   agent_id = COALESCE(EXCLUDED.agent_id, trace_usage.agent_id),
                   user_id = COALESCE(EXCLUDED.user_id, trace_usage.user_id),
                   model = CASE WHEN trace_usage.priced_by = 'router' THEN trace_usage.model ELSE EXCLUDED.model END,
                   provider = CASE WHEN trace_usage.priced_by = 'router' THEN trace_usage.provider ELSE EXCLUDED.provider END,
                   input_tokens = GREATEST(EXCLUDED.input_tokens, trace_usage.router_input_tokens),
                   output_tokens = GREATEST(EXCLUDED.output_tokens, trace_usage.router_output_tokens),
                   cache_read_tokens = GREATEST(EXCLUDED.cache_read_tokens, trace_usage.router_cache_read_tokens),
                   cache_creation_tokens = GREATEST(EXCLUDED.cache_creation_tokens, trace_usage.router_cache_creation_tokens),
                   tool_call_count = EXCLUDED.tool_call_count,
                   cost_usd = GREATEST(EXCLUDED.cost_usd, trace_usage.router_cost_usd),
                   prompt_cost_usd = GREATEST(EXCLUDED.prompt_cost_usd, trace_usage.router_prompt_cost_usd),
                   completion_cost_usd = GREATEST(EXCLUDED.completion_cost_usd, trace_usage.router_completion_cost_usd),
                   latency_ms = EXCLUDED.latency_ms,
                   error_count = EXCLUDED.error_count,
                   materialized_at = now()"#,
        )
        .bind(&row.trace_id)
        .bind(&row.agent_name)
        .bind(session_id)
        .bind(agent_id)
        .bind(user_id)
        .bind(&row.model)
        .bind(provider)
        .bind(count(row.input_tokens))
        .bind(count(row.output_tokens))
        .bind(count(row.cache_read_tokens))
        .bind(count(row.cache_creation_tokens))
        .bind(row.tool_call_count as i64)
        .bind(row.cost_usd)
        .bind(row.prompt_cost_usd)
        .bind(row.completion_cost_usd)
        .bind(row.latency_ms)
        .bind(row.started_at)
        .bind(row.error_count as i32)
        .bind(source.as_str())
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE trace_usage SET cost_estimated=$3 WHERE trace_id=$1 AND agent_name=$2")
        .bind(&row.trace_id)
        .bind(&row.agent_name)
        .bind(row.cost_estimated)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn row(tokens: u64, tools: u32, model: Option<&str>) -> TraceUsageRow {
        TraceUsageRow {
            trace_id: "t".into(),
            agent_name: "calendar-agent-1".into(),
            session_id: None,
            model: model.map(str::to_owned),
            provider: None,
            input_tokens: tokens,
            output_tokens: tokens / 10,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            tool_call_count: tools,
            cost_estimated: false,
            cost_usd: tokens as f64,
            prompt_cost_usd: 0.0,
            completion_cost_usd: 0.0,
            latency_ms: Some(5),
            started_at: Utc::now(),
            error_count: 0,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn a_tool_only_row_adds_its_calls_to_the_token_row_without_touching_tokens() {
        let mut llm = row(1000, 0, Some("gpt-4o"));
        merge_usage_rows(&mut llm, &row(0, 6, None));
        assert_eq!(
            (llm.input_tokens, llm.tool_call_count, llm.model.as_deref()),
            (1000, 6, Some("gpt-4o"))
        );
    }

    #[test]
    fn the_merge_is_order_independent() {
        let mut tools_first = row(0, 6, None);
        merge_usage_rows(&mut tools_first, &row(1000, 0, Some("gpt-4o")));
        assert_eq!(
            (
                tools_first.input_tokens,
                tools_first.tool_call_count,
                tools_first.model.as_deref()
            ),
            (1000, 6, Some("gpt-4o"))
        );
    }
}
