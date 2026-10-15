//! `session/list`, served entirely from Postgres.
//!
//! This replaces a per-row trace-store enrichment: `get_all_sessions` used to
//! call `provider.get_session` once per row, and each of those is two Tempo
//! searches (one of them unbounded) plus a full trace download and three Loki
//! queries *per trace*. A 100-row page cost on the order of 1,400 backend
//! requests to render five.
//!
//! Every field the response carries already had a SQL source:
//!
//! | Field | Source | Written by |
//! |---|---|---|
//! | `num_traces` | `session_traces` | `agent_proxy`, at forward time |
//! | tokens, cost, latency, window | `trace_usage` | `trace_materializer` |
//! | `first_input` / `last_output` | `chat_messages` | the chat path |
//! | `status`, `failed_flows` | `flows` | `agent_proxy`, `router::a2a_dispatch` |
//! | `error_count` | `trace_usage.error_count` | `trace_materializer` |
//!
//! `session_traces` is the authoritative session-to-trace index rather than
//! `trace_usage`, which only has rows for token-bearing traces: counting from
//! it would hide every session whose agent never made an LLM call.
//!
//! The shape is page-first: one CTE applies the caller's scope, window, status
//! filter and keyset cursor and takes `limit + 1` rows, and the aggregates hang
//! off it as lateral joins. They therefore run for the page, never for the
//! window, which is what keeps the cost flat as sessions accumulate.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use nasiko_observability::ObservabilityError;
use sqlx::PgPool;

use super::service::{
    CostEntry, Pagination, SessionListData, SessionListResponse, SessionSummary, SimpleCostSummary,
    TokenUsageSummary,
};

/// Ceiling on the unfiltered `matching_count`. An exact count over a window
/// holding 100,000 sessions is the one query in this design that would not stay
/// flat, and no caller needs the exact number — the filtered counts, which are
/// what "3 failed sessions" is built on, are always exact.
const MATCHING_COUNT_CAP: i64 = 10_000;

// ─── Request shape ───────────────────────────────────────────────────────────

/// `status=` filter. Mirrors the derived status on the response, so a filtered
/// list and the badge can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    Failed,
    Ok,
    Running,
}

impl StatusFilter {
    pub fn parse(raw: &str) -> Result<Self, ObservabilityError> {
        match raw {
            "failed" => Ok(Self::Failed),
            "ok" => Ok(Self::Ok),
            "running" => Ok(Self::Running),
            other => Err(ObservabilityError::BadRequest(format!(
                "status '{other}' is not one of: failed, ok, running"
            ))),
        }
    }
}

/// Everything `session/list` needs, already validated.
#[derive(Debug, Clone)]
pub struct SessionListQuery {
    pub caller: Option<uuid::Uuid>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub status: Option<StatusFilter>,
    pub limit: i64,
    pub cursor: Option<(DateTime<Utc>, String)>,
}

// ─── Cursor ──────────────────────────────────────────────────────────────────

/// Keyset anchor on `(created_at, session_id)`, the list's sort order. Same
/// encoding as the chat session cursor (`chat::routes`), so the two read alike.
pub fn encode_cursor(ts: DateTime<Utc>, id: &str) -> String {
    let nanos = ts.timestamp_nanos_opt().unwrap_or(0);
    URL_SAFE_NO_PAD.encode(format!("{nanos}:{id}"))
}

pub fn decode_cursor(s: &str) -> Option<(DateTime<Utc>, String)> {
    let raw = URL_SAFE_NO_PAD.decode(s).ok()?;
    let txt = std::str::from_utf8(&raw).ok()?;
    let (nanos_str, id) = txt.split_once(':')?;
    let nanos: i64 = nanos_str.parse().ok()?;
    Some((DateTime::from_timestamp_nanos(nanos), id.to_owned()))
}

// ─── Row ─────────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct Row {
    session_id: String,
    agent_name: Option<String>,
    created_at: DateTime<Utc>,
    trace_count: Option<i64>,
    total_tokens: Option<i64>,
    total_cost: Option<f64>,
    first_started: Option<DateTime<Utc>>,
    last_ended: Option<DateTime<Utc>>,
    p50: Option<f64>,
    p99: Option<f64>,
    first_input: Option<String>,
    last_output: Option<String>,
    error_count: Option<i64>,
    flow_count: Option<i64>,
    failed_flow_count: Option<i64>,
    running_flow_count: Option<i64>,
}

/// The derived session status. Request outcome, not span errors: a flow row is
/// written at the one chokepoint every request crosses, so it is exact and
/// immediate, where span errors depend on the agent being instrumented and on
/// the materializer having caught up. The span-error count is reported
/// separately as `error_count`, and the two are never mixed.
fn derive_status(row: &Row) -> Option<&'static str> {
    let failed = row.failed_flow_count.unwrap_or(0);
    let running = row.running_flow_count.unwrap_or(0);
    let total = row.flow_count.unwrap_or(0);
    if failed > 0 {
        Some("failed")
    } else if running > 0 {
        Some("running")
    } else if total > 0 {
        Some("ok")
    } else {
        // No recorded flows: nothing says this session succeeded or failed.
        None
    }
}

impl Row {
    fn into_summary(self) -> SessionSummary {
        let status = derive_status(&self);
        let started = self.first_started.unwrap_or(self.created_at);
        let ended = self.last_ended.unwrap_or(started);
        let duration_ms = (ended - started).num_milliseconds().max(0) as u64;
        // `num_traces` doubles as "this session has trace data"; keep it absent
        // rather than 0 so existing clients read it the way they always have.
        let num_traces = self.trace_count.filter(|&c| c > 0).map(|c| c as u32);
        SessionSummary {
            id: self.session_id.clone(),
            session_id: self.session_id,
            agent_id: self.agent_name.unwrap_or_default(),
            num_traces,
            start_time: Some(super::service::fmt_ts(started)),
            end_time: Some(super::service::fmt_ts(ended)),
            duration_ms: (duration_ms > 0).then_some(duration_ms),
            first_input: self.first_input,
            last_output: self.last_output,
            token_usage: TokenUsageSummary {
                total: self.total_tokens.filter(|&t| t > 0).map(|t| t as u64),
            },
            trace_latency_ms_p50: self.p50,
            trace_latency_ms_p99: self.p99,
            cost_summary: SimpleCostSummary {
                total: CostEntry {
                    cost: self.total_cost.filter(|&c| c > 0.0),
                },
            },
            status: status.map(str::to_owned),
            failed_flows: self.failed_flow_count.unwrap_or(0) as u32,
            // `null` while no trace in this session has been materialized yet:
            // "not yet known", never "zero errors". A materialized trace with no
            // unrecovered error span reports 0.
            error_count: self.error_count.map(|c| c.max(0) as u32),
            session_annotations: vec![],
            session_annotation_summaries: vec![],
        }
    }
}

// ─── Query ───────────────────────────────────────────────────────────────────

/// The status filter, as a predicate on `chat_sessions s` inside the page CTE.
///
/// It has to be a predicate rather than a filter over the joined result: the
/// cursor and `LIMIT` are applied in the same step, so filtering afterwards
/// would return short pages and skip rows.
fn status_predicate(status: Option<StatusFilter>) -> &'static str {
    match status {
        None => "TRUE",
        Some(StatusFilter::Failed) => {
            "EXISTS (SELECT 1 FROM flows f \
                      WHERE f.session_id = s.session_id AND f.status = 'failed')"
        }
        // 'failed' wins over 'running' in `derive_status`, so the filter has to
        // exclude it — otherwise a `status=running` page lists rows whose own
        // badge reads "failed".
        Some(StatusFilter::Running) => {
            "EXISTS (SELECT 1 FROM flows f \
                      WHERE f.session_id = s.session_id AND f.status IN ('running','paused')) \
             AND NOT EXISTS (SELECT 1 FROM flows f \
                      WHERE f.session_id = s.session_id AND f.status = 'failed')"
        }
        // "ok" is "has flows, none of them unfinished or failed".
        Some(StatusFilter::Ok) => {
            "EXISTS (SELECT 1 FROM flows f WHERE f.session_id = s.session_id) \
             AND NOT EXISTS (SELECT 1 FROM flows f \
                      WHERE f.session_id = s.session_id \
                        AND f.status IN ('failed','running','paused'))"
        }
    }
}

/// Shared WHERE clause for the page and for `matching_count`, so the two can
/// never drift apart.
fn scope_clause(status: Option<StatusFilter>) -> String {
    format!(
        "s.deleted_at IS NULL \
         AND ($1::uuid IS NULL OR s.user_id = $1) \
         AND s.created_at >= $2 \
         AND ($3::timestamptz IS NULL OR s.created_at < $3) \
         AND {}",
        status_predicate(status)
    )
}

pub async fn list_sessions(
    db: &PgPool,
    q: &SessionListQuery,
) -> Result<SessionListResponse, ObservabilityError> {
    let scope = scope_clause(q.status);
    // One extra row answers "is there a next page?" without a second query.
    let fetch = q.limit + 1;

    let sql = format!(
        r#"
WITH page AS (
    SELECT s.session_id, s.agent_id, s.created_at
      FROM chat_sessions s
     WHERE {scope}
       AND ($4::timestamptz IS NULL OR (s.created_at, s.session_id) < ($4, $5))
     ORDER BY s.created_at DESC, s.session_id DESC
     LIMIT $6
)
SELECT p.session_id,
       a.name AS agent_name,
       p.created_at,
       tr.trace_count,
       tu.total_tokens,
       tu.total_cost,
       tu.first_started,
       tu.last_ended,
       tu.p50,
       tu.p99,
       tu.error_count,
       fi.content AS first_input,
       lo.content AS last_output,
       fl.flow_count,
       fl.failed_flow_count,
       fl.running_flow_count
  FROM page p
  LEFT JOIN agents a ON a.id = p.agent_id
  LEFT JOIN LATERAL (
        SELECT count(*)::bigint AS trace_count
          FROM session_traces st
         WHERE st.session_id = p.session_id
  ) tr ON TRUE
  LEFT JOIN LATERAL (
        -- trace_usage is keyed (trace_id, agent_name), so a multi-agent trace
        -- has one row per agent: collapse to one row per trace first, or the
        -- percentiles count that trace once per agent.
        SELECT sum(t.tokens)::bigint AS total_tokens,
               sum(t.cost) AS total_cost,
               min(t.started) AS first_started,
               max(t.started + (coalesce(t.latency, 0) * interval '1 millisecond')) AS last_ended,
               percentile_cont(0.5)  WITHIN GROUP (ORDER BY t.latency) AS p50,
               percentile_cont(0.99) WITHIN GROUP (ORDER BY t.latency) AS p99,
               -- NULL only while no trace in this session has been materialized;
               -- summed because error_count is per (trace, agent).
               sum(t.errors)::bigint AS error_count
          FROM (
                SELECT u.trace_id,
                       sum(u.input_tokens + u.output_tokens
                           + u.cache_read_tokens + u.cache_creation_tokens)::bigint AS tokens,
                       sum(u.cost_usd) AS cost,
                       min(u.started_at) AS started,
                       max(u.latency_ms) AS latency,
                       sum(u.error_count)::bigint AS errors
                  FROM trace_usage u
                 WHERE u.session_id = p.session_id
                 GROUP BY u.trace_id
          ) t
  ) tu ON TRUE
  LEFT JOIN LATERAL (
        SELECT m.content
          FROM chat_messages m
         WHERE m.session_id = p.session_id AND m.role = 'user'
         ORDER BY m.timestamp ASC
         LIMIT 1
  ) fi ON TRUE
  LEFT JOIN LATERAL (
        SELECT m.content
          FROM chat_messages m
         WHERE m.session_id = p.session_id AND m.role <> 'user'
         ORDER BY m.timestamp DESC
         LIMIT 1
  ) lo ON TRUE
  LEFT JOIN LATERAL (
        SELECT count(*)::bigint AS flow_count,
               count(*) FILTER (WHERE f.status = 'failed')::bigint AS failed_flow_count,
               count(*) FILTER (WHERE f.status IN ('running','paused'))::bigint AS running_flow_count
          FROM flows f
         WHERE f.session_id = p.session_id
  ) fl ON TRUE
 ORDER BY p.created_at DESC, p.session_id DESC
"#
    );

    let (cursor_ts, cursor_sid) = match &q.cursor {
        Some((ts, sid)) => (Some(*ts), Some(sid.clone())),
        None => (None, None),
    };

    let mut rows: Vec<Row> = sqlx::query_as(&sql)
        .bind(q.caller)
        .bind(q.start)
        .bind(q.end)
        .bind(cursor_ts)
        .bind(cursor_sid)
        .bind(fetch)
        .fetch_all(db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

    let has_next_page = rows.len() > q.limit as usize;
    if has_next_page {
        rows.pop();
    }

    let end_cursor = has_next_page
        .then(|| {
            rows.last()
                .map(|r| encode_cursor(r.created_at, &r.session_id))
        })
        .flatten();

    let matching_count = matching_count(db, q, &scope).await?;
    let sessions: Vec<SessionSummary> = rows.into_iter().map(Row::into_summary).collect();
    let successful_agents = sessions.iter().filter(|s| s.num_traces.is_some()).count();
    let total_agents = total_agents(db).await;

    Ok(SessionListResponse {
        data: SessionListData {
            sessions,
            total_agents,
            successful_agents,
            matching_count,
            pagination: Pagination {
                end_cursor,
                has_next_page,
            },
        },
    })
}

/// Rows matching the filters, so "3 failed sessions" comes from one call rather
/// than from counting a sampled page.
///
/// Exact when a `status` filter narrows the set (the Needs-you case, always
/// small); capped otherwise, because an unfiltered count is the one part of
/// this query that grows with the window.
async fn matching_count(
    db: &PgPool,
    q: &SessionListQuery,
    scope: &str,
) -> Result<i64, ObservabilityError> {
    let sql = if q.status.is_some() {
        format!("SELECT count(*)::bigint FROM chat_sessions s WHERE {scope}")
    } else {
        format!(
            "SELECT count(*)::bigint FROM \
             (SELECT 1 FROM chat_sessions s WHERE {scope} LIMIT {MATCHING_COUNT_CAP}) capped"
        )
    };
    sqlx::query_scalar(&sql)
        .bind(q.caller)
        .bind(q.start)
        .bind(q.end)
        .fetch_one(db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))
}

/// Live agents, for the response's `total_agents`. Unfiltered by design — it is
/// the denominator the UI shows beside `successful_agents`, not a scoped count.
async fn total_agents(db: &PgPool) -> usize {
    sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM agents WHERE deleted_at IS NULL")
        .fetch_one(db)
        .await
        .unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(flow: i64, failed: i64, running: i64) -> Row {
        Row {
            session_id: "s".into(),
            agent_name: None,
            created_at: Utc::now(),
            trace_count: None,
            total_tokens: None,
            total_cost: None,
            first_started: None,
            last_ended: None,
            p50: None,
            p99: None,
            first_input: None,
            last_output: None,
            error_count: None,
            flow_count: Some(flow),
            failed_flow_count: Some(failed),
            running_flow_count: Some(running),
        }
    }

    #[test]
    fn status_is_failed_when_any_flow_failed() {
        // Failure wins over a concurrent running flow: a session with one failed
        // request has failed, whatever else is still in flight.
        assert_eq!(derive_status(&row(3, 1, 1)), Some("failed"));
    }

    #[test]
    fn status_is_running_only_without_a_failure() {
        assert_eq!(derive_status(&row(2, 0, 1)), Some("running"));
    }

    #[test]
    fn status_is_ok_when_every_flow_finished() {
        assert_eq!(derive_status(&row(2, 0, 0)), Some("ok"));
    }

    #[test]
    fn status_is_unknown_without_flows() {
        // Not "ok": no recorded flow means nothing says the session succeeded.
        assert_eq!(derive_status(&row(0, 0, 0)), None);
    }

    #[test]
    fn cursor_round_trips() {
        let ts = DateTime::from_timestamp_nanos(1_760_000_000_123_456_789);
        let encoded = encode_cursor(ts, "ses_abc");
        let (back_ts, back_id) = decode_cursor(&encoded).expect("decodes");
        assert_eq!(back_ts, ts);
        assert_eq!(back_id, "ses_abc");
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(decode_cursor("not-base64!!").is_none());
        assert!(decode_cursor(&URL_SAFE_NO_PAD.encode("no-separator")).is_none());
    }

    #[test]
    fn status_filter_parses_known_values_only() {
        assert_eq!(StatusFilter::parse("failed").unwrap(), StatusFilter::Failed);
        assert_eq!(StatusFilter::parse("ok").unwrap(), StatusFilter::Ok);
        assert_eq!(
            StatusFilter::parse("running").unwrap(),
            StatusFilter::Running
        );
        assert!(StatusFilter::parse("broken").is_err());
    }

    #[test]
    fn ok_filter_excludes_running_and_failed() {
        // The filter has to agree with `derive_status`, or a `status=ok` page
        // would list rows whose own badge reads "failed".
        let p = status_predicate(Some(StatusFilter::Ok));
        assert!(p.contains("NOT EXISTS"));
        assert!(p.contains("'failed','running','paused'"));
    }

    #[test]
    fn running_filter_excludes_failed() {
        let p = status_predicate(Some(StatusFilter::Running));
        assert!(p.contains("NOT EXISTS"));
        assert!(p.contains("AND f.status = 'failed'"));
    }

    #[test]
    fn unfiltered_scope_has_no_flow_predicate() {
        assert!(!scope_clause(None).contains("flows"));
    }
}
