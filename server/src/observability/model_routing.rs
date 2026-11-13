//! `GET /api/observability/finops/model-routing` — how the LLM router chose the model, for the
//! same window and filters as the FinOps dashboard beside it (NAS-1263).
//!
//! # What is counted, and why it is calls rather than traces
//!
//! The router picks a model per LLM call, through a fixed precedence chain (pinned, decision
//! cache, small talk, classifier, the agent's config, the default). Each call writes what chose it
//! into its usage row's `metadata.routing` block as flat counters, and a `trace_usage` row is a
//! whole trace's calls merged key by key — so one row can carry a classification *and* the cache
//! hits that followed it. Counting rows would answer a different, much blunter question; this
//! endpoint sums the counters, so "how many calls did small talk serve" is exact.
//!
//! # Why tokens and cost are attributed only to traces one level served alone
//!
//! Tokens and cost are per trace row, not per call: a trace that classified once and cached twice
//! has one cost figure and no way to split it between the two levels. Rather than apportion it —
//! which would publish an invented number beside exact ones — each bucket's `tokens` and
//! `cost_usd` cover only the traces where that level (or tier) served *every* call, and
//! `attributed_calls` says how many of the bucket's calls those traces account for. A bucket whose
//! `attributed_calls` is far below its `calls` has a cost figure that covers only part of its
//! work, and says so in the same object.
//!
//! # Coverage
//!
//! `unrecorded_traces` counts the window's rows with no routing block at all: calls made before
//! this shipped, and calls that never route a model (embeddings). The dashboard needs it to tell
//! "nothing was routed this way" from "this window predates the record".

use chrono::{DateTime, Utc};
use nasiko_llm_router::routing::RouteSource;
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder};
use utoipa::ToSchema;
use uuid::Uuid;

/// The three tier counter keys, in strength order, as `RouteDecision::to_metadata` writes them.
const TIER_KEYS: [&str; 3] = ["tier1", "tier2", "tier3"];

/// Everything the queries need to scope themselves, resolved by the handler. The same filters
/// `/finops/savings` takes, so a client carries its filter state across without translating it.
pub struct ModelRoutingQuery<'a> {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub agent_id: Option<Uuid>,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    /// Restricts every aggregate to agents the caller may see. `None` means unrestricted (a
    /// superuser); an empty slice means the caller can see nothing and every figure is zero.
    pub accessible_agent_ids: Option<&'a [Uuid]>,
    /// Restricts every aggregate to these callers' own calls, as `/finops/savings` does: a public
    /// agent's routing must not read the same for every viewer who can see it.
    pub user_ids: Option<&'a [Uuid]>,
}

/// One precedence level or tier over the window.
#[derive(Debug, Serialize, ToSchema)]
pub struct RoutingBucket {
    /// The counter key: a level (`small_talk`) or a tier (`tier3`).
    pub key: String,
    pub calls: i64,
    /// `calls / total calls`, rounded to 2dp. `null` when nothing was routed in the window — an
    /// undefined share, not a zero one.
    pub call_share_pct: Option<f64>,
    /// Traces with at least one call served this way.
    pub traces: i64,
    /// Calls on traces this bucket served alone — the ones `tokens` and `cost_usd` cover.
    pub attributed_calls: i64,
    pub tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoutingWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelRoutingData {
    pub window: RoutingWindow,
    /// Trace rows carrying a routing record.
    pub traces: i64,
    pub calls: i64,
    /// Calls where routing overrode the model the resolver had picked.
    pub model_overridden: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    /// Rows in the window with no routing record (see the module docs).
    pub unrecorded_traces: i64,
    /// Every precedence level, in precedence order, zeros included: a level that served nothing is
    /// a fact about the window, and a missing row would read as a missing feature.
    pub by_level: Vec<RoutingBucket>,
    /// Tier 1 → 3, zeros included, for the same reason.
    pub by_tier: Vec<RoutingBucket>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelRoutingResponse {
    pub data: ModelRoutingData,
    pub status_code: u16,
    pub message: String,
}

/// Percentage of the window's routed calls, rounded to 2dp. `None` when nothing was routed, which
/// mirrors `savings::pct`'s rule for an undefined percentage.
fn share(part: i64, whole: i64) -> Option<f64> {
    if whole == 0 {
        return None;
    }
    Some((part as f64 / whole as f64 * 100.0 * 100.0).round() / 100.0)
}

/// The window, dimension, ACL and ownership filters, applied to `trace_usage` exactly as
/// `savings::push_filters` applies them to the savings tables.
fn push_filters<'a>(qb: &mut QueryBuilder<'a, Postgres>, q: &'a ModelRoutingQuery<'a>) {
    qb.push(" WHERE started_at >= ")
        .push_bind(q.start)
        .push(" AND started_at < ")
        .push_bind(q.end);

    if let Some(agent) = q.agent_id {
        qb.push(" AND agent_id = ").push_bind(agent);
    }
    if let Some(provider) = q.provider {
        qb.push(" AND provider = ").push_bind(provider);
    }
    if let Some(model) = q.model {
        qb.push(" AND model = ").push_bind(model);
    }
    if let Some(ids) = q.accessible_agent_ids {
        // An empty allow-list must match nothing, as it does on `/finops/savings`.
        qb.push(" AND agent_id = ANY(").push_bind(ids).push(")");
    }
    if let Some(ids) = q.user_ids {
        qb.push(" AND user_id = ANY(").push_bind(ids).push(")");
    }
}

/// The window's totals: rows with a routing record, the calls on them, and the rows without one.
async fn totals(db: &PgPool, q: &ModelRoutingQuery<'_>) -> Result<TotalsRow, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT
             COUNT(*) FILTER (WHERE metadata ? 'routing')::BIGINT AS traces,
             COUNT(*) FILTER (WHERE NOT (metadata ? 'routing'))::BIGINT AS unrecorded_traces,
             COALESCE(SUM((metadata->'routing'->>'calls')::BIGINT), 0)::BIGINT AS calls,
             COALESCE(SUM((metadata->'routing'->>'model_overridden')::BIGINT), 0)::BIGINT
                 AS model_overridden,
             -- `FILTER` belongs to the aggregate, so it goes after `SUM(...)`'s closing paren:
             -- inside the argument list it is a syntax error, not a narrower sum.
             COALESCE(SUM(input_tokens + output_tokens + cache_read_tokens
                          + cache_creation_tokens)
                      FILTER (WHERE metadata ? 'routing'), 0)::BIGINT AS tokens,
             COALESCE(SUM(cost_usd) FILTER (WHERE metadata ? 'routing'), 0)::FLOAT8 AS cost_usd
         FROM trace_usage",
    );
    push_filters(&mut qb, q);
    qb.build_query_as().fetch_one(db).await
}

#[derive(sqlx::FromRow)]
struct TotalsRow {
    traces: i64,
    unrecorded_traces: i64,
    calls: i64,
    model_overridden: i64,
    tokens: i64,
    cost_usd: f64,
}

#[derive(sqlx::FromRow)]
struct BucketRow {
    key: String,
    calls: i64,
    traces: i64,
    attributed_calls: i64,
    tokens: i64,
    cost_usd: f64,
}

/// One row per counter key that served at least one call, levels and tiers together — they are
/// read from the same block over the same rows, so splitting them into two queries would scan
/// `trace_usage` twice to answer one question.
async fn buckets(
    db: &PgPool,
    q: &ModelRoutingQuery<'_>,
    keys: &[&str],
) -> Result<Vec<BucketRow>, sqlx::Error> {
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_string()).collect();
    let mut qb = QueryBuilder::new(
        "SELECT key,
                SUM(n)::BIGINT AS calls,
                COUNT(*)::BIGINT AS traces,
                COALESCE(SUM(n) FILTER (WHERE n = total), 0)::BIGINT AS attributed_calls,
                COALESCE(SUM(tokens) FILTER (WHERE n = total), 0)::BIGINT AS tokens,
                COALESCE(SUM(cost_usd) FILTER (WHERE n = total), 0)::FLOAT8 AS cost_usd
         FROM (
             SELECT counter.key AS key,
                    counter.value::BIGINT AS n,
                    (tu.metadata->'routing'->>'calls')::BIGINT AS total,
                    tu.input_tokens + tu.output_tokens + tu.cache_read_tokens
                        + tu.cache_creation_tokens AS tokens,
                    COALESCE(tu.cost_usd, 0) AS cost_usd
             FROM trace_usage tu
             CROSS JOIN LATERAL jsonb_each_text(tu.metadata->'routing') AS counter(key, value)",
    );
    push_filters(&mut qb, q);
    qb.push(" AND tu.metadata ? 'routing' AND counter.key = ANY(")
        .push_bind(keys)
        // `n > 0` outside the lateral: a key no call used adds nothing, and its zero would
        // otherwise count the trace under every level the window ever saw.
        .push(")) AS counters WHERE n > 0 GROUP BY key");
    qb.build_query_as().fetch_all(db).await
}

/// Lay the rows out over `keys` in a fixed order, filling a key no call used with zeros.
fn lay_out(keys: &[&str], rows: &[BucketRow], calls: i64) -> Vec<RoutingBucket> {
    keys.iter()
        .map(|key| {
            let row = rows.iter().find(|r| r.key == *key);
            RoutingBucket {
                key: (*key).to_string(),
                calls: row.map_or(0, |r| r.calls),
                call_share_pct: share(row.map_or(0, |r| r.calls), calls),
                traces: row.map_or(0, |r| r.traces),
                attributed_calls: row.map_or(0, |r| r.attributed_calls),
                tokens: row.map_or(0, |r| r.tokens),
                cost_usd: row.map_or(0.0, |r| r.cost_usd),
            }
        })
        .collect()
}

pub async fn get_model_routing(
    db: &PgPool,
    q: &ModelRoutingQuery<'_>,
) -> Result<ModelRoutingData, sqlx::Error> {
    let levels: Vec<&str> = RouteSource::ALL.iter().map(|s| s.as_label()).collect();
    let keys: Vec<&str> = levels.iter().copied().chain(TIER_KEYS).collect();

    let totals = totals(db, q).await?;
    let rows = buckets(db, q, &keys).await?;

    Ok(ModelRoutingData {
        window: RoutingWindow {
            start: q.start,
            end: q.end,
        },
        traces: totals.traces,
        calls: totals.calls,
        model_overridden: totals.model_overridden,
        tokens: totals.tokens,
        cost_usd: totals.cost_usd,
        unrecorded_traces: totals.unrecorded_traces,
        by_level: lay_out(&levels, &rows, totals.calls),
        by_tier: lay_out(&TIER_KEYS, &rows, totals.calls),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, calls: i64, attributed: i64) -> BucketRow {
        BucketRow {
            key: key.to_string(),
            calls,
            traces: 1,
            attributed_calls: attributed,
            tokens: 100,
            cost_usd: 0.5,
        }
    }

    /// Every level is laid out in precedence order whatever the database returned, so the card's
    /// rows never reorder themselves between two windows.
    #[test]
    fn levels_keep_their_precedence_order_and_fill_the_gaps() {
        let levels: Vec<&str> = RouteSource::ALL.iter().map(|s| s.as_label()).collect();
        let laid = lay_out(&levels, &[row("classified", 3, 3), row("pinned", 1, 1)], 4);

        let keys: Vec<&str> = laid.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "pinned",
                "cache_hit",
                "small_talk",
                "classified",
                "config",
                "default"
            ]
        );
        assert_eq!(laid[3].calls, 3);
        assert_eq!(laid[3].call_share_pct, Some(75.0));
        // A level no call used is zero, not absent: "small talk served nothing this window" is an
        // answer, and a missing row would read as a missing feature.
        assert_eq!(laid[2].calls, 0);
        assert_eq!(laid[2].cost_usd, 0.0);
    }

    /// An empty window has no share to report: `null`, never 0%, which would read as "every call
    /// went elsewhere".
    #[test]
    fn an_empty_window_has_no_share() {
        let laid = lay_out(&TIER_KEYS, &[], 0);
        assert!(laid.iter().all(|b| b.call_share_pct.is_none()));
    }

    /// The cost figure covers only the traces a bucket served alone, and `attributed_calls` says
    /// how much of its work that is — a consumer can tell a complete figure from a partial one
    /// without a second request.
    #[test]
    fn a_buckets_cost_is_reported_with_the_calls_it_covers() {
        let laid = lay_out(&["cache_hit"], &[row("cache_hit", 9, 2)], 9);
        assert_eq!((laid[0].calls, laid[0].attributed_calls), (9, 2));
    }
}
