//! Subagent sessions in `session/list` and `session/{id}`.
//!
//! A subagent session is an ordinary coding-agent session whose `chat_sessions`
//! row names the session that spawned it (`parent_session_id`, set at ingest).
//! Its turns are its own receipts and its own `trace_usage` rows, so its spend
//! is counted once, on the child, by every total that reads those sources.
//! Nothing here adds to that: these queries only present a parent's children
//! beside it, and their totals are reported apart from the parent's own.

use chrono::{DateTime, Utc};
use nasiko_observability::ObservabilityError;
use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;

/// Children listed in one session's detail. A Claude Code session that fans
/// out runs tens of subagents, not thousands; the cap only bounds the payload.
const MAX_SUBAGENTS_IN_DETAIL: i64 = 500;

/// One session's coding-agent totals, as a lateral subquery over `session`
/// (an SQL expression naming the session id). The same sources `sessions_sql`
/// totals a coding session from, so a child's figures here match its own row:
/// turns, tokens and window from the receipts (one per turn), cost from the
/// turns priced at ingest, else from `trace_usage` while they are unpriced.
///
/// Yields `turns`, `tokens`, `cost`, `started` and `ended`; always one row.
/// The token caps are guarded: `LEAST` ignores a `NULL`, so an empty sum would
/// otherwise read as the cap rather than as unknown.
pub(crate) fn coding_totals(session: &str) -> String {
    format!(
        r#"SELECT r.turns, r.tokens, r.started, r.ended,
                  COALESCE(priced.cost, traced.cost) AS cost
             FROM (SELECT count(*)::bigint AS turns,
                          CASE WHEN count(d.turn_tokens) > 0
                               THEN LEAST(sum(d.turn_tokens), 9223372036854775807) END::bigint AS tokens,
                          min(d.turn_started_at) AS started,
                          max(d.turn_ended_at) AS ended
                     FROM (SELECT DISTINCT ON (e.turn_id)
                                  e.turn_tokens, e.turn_started_at, e.turn_ended_at
                             FROM coding_agent_telemetry_events e
                            WHERE e.session_id = {session}
                            ORDER BY e.turn_id, e.received_at DESC) d) r
            CROSS JOIN LATERAL (
                  SELECT sum(m.cost_usd)::float8 AS cost
                    FROM chat_messages m
                   WHERE m.session_id = {session}
                     AND m.role = 'assistant'
                     AND m.external_turn_id IS NOT NULL) priced
            CROSS JOIN LATERAL (
                  SELECT sum(u.cost_usd)::float8 AS cost
                    FROM trace_usage u
                   WHERE u.session_id = {session}) traced"#
    )
}

/// The `session/list` lateral that totals a page row's children. `page` is the
/// alias of the page CTE, which must carry `session_id` and `user_id`. Yields
/// `subagent_count`, `subagent_turns`, `subagent_tokens` and `subagent_cost`.
pub(crate) fn subagent_totals_lateral(page: &str) -> String {
    format!(
        r#"LEFT JOIN LATERAL (
        SELECT count(*)::bigint AS subagent_count,
               sum(k.turns)::bigint AS subagent_turns,
               CASE WHEN count(k.tokens) > 0
                    THEN LEAST(sum(k.tokens), 9223372036854775807) END::bigint AS subagent_tokens,
               sum(k.cost)::float8 AS subagent_cost
          FROM chat_sessions c
         CROSS JOIN LATERAL ({totals}) k
         WHERE c.parent_session_id = {page}.session_id
           AND c.user_id = {page}.user_id
           AND c.deleted_at IS NULL
  ) sa ON TRUE"#,
        totals = coding_totals("c.session_id")
    )
}

/// What a session's children add up to, beside (never inside) its own totals.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SubagentTotals {
    pub turns: u32,
    /// `null` while no child turn reported token counts.
    pub tokens: Option<u64>,
    /// `null` while no child turn is priced.
    pub cost: Option<f64>,
}

impl SubagentTotals {
    pub(crate) fn from_sums(turns: Option<i64>, tokens: Option<i64>, cost: Option<f64>) -> Self {
        Self {
            turns: clamp_u32(turns),
            tokens: tokens.map(|t| t.max(0) as u64),
            cost,
        }
    }
}

/// One child of a session, as its detail lists it.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SubagentRun {
    pub session_id: String,
    /// The subagent's kind as the coding agent names it (e.g. "Explore").
    pub subagent_type: Option<String>,
    /// The parent's tool call that spawned it.
    pub spawn_tool_call_id: Option<String>,
    pub turns: u32,
    pub tokens: Option<u64>,
    pub cost: Option<f64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
}

/// A session's place in its subagent tree: its own parent link and its children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionLineage {
    pub parent_session_id: Option<String>,
    pub spawn_tool_call_id: Option<String>,
    pub subagent_type: Option<String>,
    pub subagents: Vec<SubagentRun>,
}

#[derive(sqlx::FromRow)]
struct ChildRow {
    session_id: String,
    subagent_type: Option<String>,
    spawn_tool_call_id: Option<String>,
    created_at: DateTime<Utc>,
    turns: Option<i64>,
    tokens: Option<i64>,
    cost: Option<f64>,
    started: Option<DateTime<Utc>>,
    ended: Option<DateTime<Utc>>,
}

impl ChildRow {
    fn into_run(self) -> SubagentRun {
        let started = self.started.unwrap_or(self.created_at);
        SubagentRun {
            session_id: self.session_id,
            subagent_type: self.subagent_type,
            spawn_tool_call_id: self.spawn_tool_call_id,
            turns: clamp_u32(self.turns),
            tokens: self.tokens.map(|t| t.max(0) as u64),
            cost: self.cost,
            start_time: Some(super::service::fmt_ts(started)),
            end_time: Some(super::service::fmt_ts(self.ended.unwrap_or(started))),
        }
    }
}

/// `session_id`'s parent link and its children, oldest first. The caller has
/// already authorized `session_id`; its children are the same user's (a
/// child's scoped id derives from the same agent), and the join says so too.
pub async fn session_lineage(
    db: &PgPool,
    session_id: &str,
) -> Result<SessionLineage, ObservabilityError> {
    let internal = |e: sqlx::Error| ObservabilityError::Internal(e.to_string());
    let link: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT parent_session_id, spawn_tool_call_id, subagent_type \
           FROM chat_sessions WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await
    .map_err(internal)?;
    let Some((parent_session_id, spawn_tool_call_id, subagent_type)) = link else {
        return Ok(SessionLineage::default());
    };
    let sql = format!(
        r#"SELECT c.session_id, c.subagent_type, c.spawn_tool_call_id, c.created_at,
                  k.turns, k.tokens, k.cost, k.started, k.ended
             FROM chat_sessions parent
             JOIN chat_sessions c
               ON c.parent_session_id = parent.session_id
              AND c.user_id = parent.user_id
              AND c.deleted_at IS NULL
            CROSS JOIN LATERAL ({totals}) k
            WHERE parent.session_id = $1
            ORDER BY COALESCE(k.started, c.created_at), c.session_id
            LIMIT $2"#,
        totals = coding_totals("c.session_id")
    );
    let children: Vec<ChildRow> = sqlx::query_as(&sql)
        .bind(session_id)
        .bind(MAX_SUBAGENTS_IN_DETAIL)
        .fetch_all(db)
        .await
        .map_err(internal)?;
    Ok(SessionLineage {
        parent_session_id,
        spawn_tool_call_id,
        subagent_type,
        subagents: children.into_iter().map(ChildRow::into_run).collect(),
    })
}

fn clamp_u32(count: Option<i64>) -> u32 {
    u32::try_from(count.unwrap_or(0).max(0)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_without_children_are_zero_turns_and_unknown_spend() {
        assert_eq!(
            SubagentTotals::from_sums(None, None, None),
            SubagentTotals {
                turns: 0,
                tokens: None,
                cost: None
            }
        );
    }

    #[test]
    fn totals_clamp_out_of_range_counts() {
        let totals = SubagentTotals::from_sums(Some(i64::MAX), Some(-1), Some(0.5));
        assert_eq!(totals.turns, u32::MAX);
        assert_eq!(totals.tokens, Some(0));
        assert_eq!(totals.cost, Some(0.5));
    }

    #[test]
    fn child_window_falls_back_to_the_session_creation_time() {
        let created_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let run = ChildRow {
            session_id: "child".into(),
            subagent_type: Some("Explore".into()),
            spawn_tool_call_id: None,
            created_at,
            turns: Some(2),
            tokens: Some(40),
            cost: None,
            started: None,
            ended: None,
        }
        .into_run();
        assert_eq!(run.start_time, run.end_time);
        assert_eq!(
            run.start_time.as_deref(),
            Some(super::super::service::fmt_ts(created_at).as_str())
        );
        assert_eq!((run.turns, run.tokens), (2, Some(40)));
    }

    #[test]
    fn totals_fragment_reads_every_source_for_the_named_session() {
        let sql = coding_totals("c.session_id");
        for source in [
            "e.session_id = c.session_id",
            "m.session_id = c.session_id",
            "u.session_id = c.session_id",
        ] {
            assert!(sql.contains(source), "{source} missing from {sql}");
        }
        let lateral = subagent_totals_lateral("p");
        assert!(lateral.contains("c.parent_session_id = p.session_id"));
        assert!(lateral.contains("c.user_id = p.user_id"));
    }
}
