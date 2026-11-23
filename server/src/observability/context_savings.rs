//! `GET /api/observability/finops/context-savings` — how much conversation history each request
//! carried, before and after selection.
//!
//! # Why this is not part of `/finops/savings`
//!
//! The two answer different questions and divide by different denominators. `/finops/savings` asks
//! "what did each optimization layer remove, out of everything this scope spent", so its baseline
//! is all tokens. This endpoint asks "how much of the history we could have sent did we send", so
//! its baseline is the pool. The same data yields a very different percentage under each — serving
//! both from one response would publish two numbers that look comparable and are not.
//!
//! # One report per request, never per call
//!
//! A turn is several LLM calls; history is selected once for the turn. Counting a pool once per
//! call would multiply every figure here by the ReAct loop's depth, so the unit is the request and
//! the source is `context_reports`, not `token_usage`.
//!
//! # Costs are resolved here, not stored
//!
//! No model call has happened when selection runs, so no price is known at write time. Each report
//! is priced at its own request's first LLM call, joined through `model_pricing`. A request whose
//! model has no price row counts in `reports` but not in `priced_reports`, and the two cost figures
//! are `null` rather than 0 when nothing could be priced — an unknown cost is not a free one.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder};
use utoipa::ToSchema;
use uuid::Uuid;

/// Bucket width for the savings trend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Hour,
    Day,
}

impl Bucket {
    /// `series=hourly|daily`, the spelling the UI sends. Absent means no series at all, which is a
    /// smaller response, not an empty one.
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, Response> {
        match raw {
            None => Ok(None),
            Some("hourly") => Ok(Some(Self::Hour)),
            Some("daily") => Ok(Some(Self::Day)),
            Some(other) => Err((
                StatusCode::BAD_REQUEST,
                format!("invalid series '{other}' — expected daily or hourly"),
            )
                .into_response()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    fn trunc(self) -> &'static str {
        self.label()
    }

    fn step(self) -> Duration {
        match self {
            Self::Hour => Duration::hours(1),
            Self::Day => Duration::days(1),
        }
    }

    /// Start of the bucket `at` falls in. Buckets are UTC, like every other finops surface.
    fn floor(self, at: DateTime<Utc>) -> DateTime<Utc> {
        use chrono::{DurationRound, Timelike};
        match self {
            Self::Hour => at.duration_trunc(Duration::hours(1)).unwrap_or(at),
            Self::Day => at
                .with_hour(0)
                .and_then(|d| d.with_minute(0))
                .and_then(|d| d.with_second(0))
                .and_then(|d| d.with_nanosecond(0))
                .unwrap_or(at),
        }
    }
}

/// Everything the queries need to scope themselves, resolved by the handler.
pub struct ContextSavingsQuery<'a> {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub agent_id: Option<Uuid>,
    /// The caller, so their own agent-less chat reports stay visible to them.
    pub caller_id: Uuid,
    pub caller_is_superuser: bool,
    /// Restricts every aggregate to agents the caller may see. `None` means unrestricted
    /// (superuser); an empty slice means the caller can see nothing.
    pub accessible_agent_ids: Option<&'a [Uuid]>,
    /// Restricts every aggregate to these people's requests. `None` means everyone's (a user
    /// manager); anyone else gets only their own, whatever agent the request went to.
    pub user_ids: Option<&'a [Uuid]>,
    pub series: Option<Bucket>,
    pub limit: i64,
    /// Narrows `top-requests` to one chart bar's interval.
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContextSavings {
    /// Requests that ran selection in the window. Larger than `reports` by the turns that selected
    /// over an empty history — a brand-new chat runs selection and has nothing to optimize.
    pub eligible_requests: i64,
    pub reports: i64,
    /// Reports whose request had a priced LLM call. The two cost figures cover only these.
    pub priced_reports: i64,
    pub pool_tokens: i64,
    pub sent_tokens: i64,
    /// `null`, never 0, when nothing in the window could be priced.
    pub pool_cost_usd: Option<f64>,
    pub sent_cost_usd: Option<f64>,
    pub messages_dropped: i64,
    pub compressed_bytes: i64,
    /// The first report we hold. A window starting earlier is not fully counted, and the chart
    /// draws those buckets as unrecorded rather than as zero savings.
    pub recorded_since: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Series>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Series {
    /// `hour` | `day`.
    pub bucket: &'static str,
    pub points: Vec<SavingsPoint>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SavingsPoint {
    pub bucket_start: DateTime<Utc>,
    pub eligible_requests: i64,
    pub reports: i64,
    pub pool_tokens: i64,
    pub sent_tokens: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentContextSavings {
    pub agent_id: Uuid,
    pub name: String,
    pub requests: i64,
    pub reports: i64,
    pub pool_tokens: i64,
    pub sent_tokens: i64,
    /// The agent's Token optimization switch **now** — this is a control the row renders, not a
    /// historical fact about the requests above it.
    pub compress_enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TopRequest {
    /// Whether the viewer may open this chat. Agent access is not chat access: a shared agent's
    /// owner must not learn other users' chat titles or be able to open their traces.
    pub own: bool,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub session_id: Option<String>,
    pub chat_title: Option<String>,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub pool_tokens: i64,
    pub sent_tokens: i64,
    pub compress_enabled: bool,
    pub started_at: DateTime<Utc>,
}

/// The window, the agent filter and the access list, appended to any query over `context_reports`.
///
/// One function for every aggregate here, so the totals, the series, the per-agent rows and the
/// top requests can never be scoped differently from each other.
///
/// `eff_agent_id` rather than `cr.agent_id`: an orchestrator-routed chat selects history before
/// routing picks an agent, so the row's own column is NULL and the agent is resolved through
/// `flows`. Filtering on the raw column would drop every routed chat from the page.
fn push_filters<'a>(qb: &mut QueryBuilder<'a, Postgres>, q: &'a ContextSavingsQuery<'a>) {
    qb.push(" WHERE cr.created_at >= ")
        .push_bind(q.start)
        .push(" AND cr.created_at < ")
        .push_bind(q.end);

    if let Some(agent) = q.agent_id {
        qb.push(" AND eff.agent_id = ").push_bind(agent);
    }
    if let Some(ids) = q.accessible_agent_ids {
        // A report with no resolvable agent is still the caller's own chat, and dropping it would
        // silently understate their own page. `= ANY('{}')` already matches nothing for an empty
        // list; it is spelled out so the intent survives a future edit.
        qb.push(" AND (eff.agent_id = ANY(")
            .push_bind(ids)
            .push(") OR (eff.agent_id IS NULL AND cr.user_id = ")
            .push_bind(q.caller_id)
            .push("))");
    }
    push_user_filter(qb, q, "cr.user_id");
}

/// Narrows a query to the people in `q.user_ids` on `column`, the request's user, so no count, total
/// or row on the page includes anyone else's requests. Nothing to add when `user_ids` is `None`.
fn push_user_filter<'a>(
    qb: &mut QueryBuilder<'a, Postgres>,
    q: &'a ContextSavingsQuery<'a>,
    column: &str,
) {
    if let Some(users) = q.user_ids {
        qb.push(" AND ")
            .push(column)
            .push(" = ANY(")
            .push_bind(users)
            .push(")");
    }
}

/// `context_reports` with its agent resolved. Every query here starts from this shape, so
/// `eff.agent_id` means the same thing in all of them.
///
/// The agent is resolved rather than read: an orchestrator-routed chat selects history *before*
/// routing picks one, so the row's own column is NULL and the agent has to be recovered from what
/// the turn did next. Three sources, in descending order of directness:
///
/// 1. `context_reports.agent_id` — set at write time by the direct-agent proxy path, which knows
///    its target before it reads history.
/// 2. `flows.root_agent_id` — set when a flow is dispatched at a named agent.
/// 3. `token_usage.agent_id` — the agents the turn actually billed against.
///
/// (3) exists because the orchestrator writes its own flow row with a NULL root_agent_id and
/// the name `'orchestrator'`, so (2) resolves to nothing for the main chat path and every routed
/// turn fell out of the per-agent views. The LLM calls a turn makes are the surviving evidence of
/// where it went. A turn the orchestrator answered itself has no agent in any of the three, which
/// is correct — it belongs in the totals and in no per-agent row.
const REPORTS_FROM: &str = "FROM context_reports_spend cr \
     LEFT JOIN LATERAL ( \
         SELECT f.root_agent_id AS agent_id FROM flows f \
          WHERE f.flow_id = cr.trace_id AND f.root_agent_id IS NOT NULL LIMIT 1 \
     ) st ON true \
     LEFT JOIN LATERAL ( \
         SELECT tu.agent_id FROM (SELECT trace_id AS session_id, started_at AS created_at, agent_id FROM trace_first_calls) tu \
          WHERE tu.session_id = cr.trace_id AND tu.agent_id IS NOT NULL \
          ORDER BY tu.created_at ASC LIMIT 1 \
     ) tua ON true \
     LEFT JOIN LATERAL ( \
         SELECT COALESCE(cr.agent_id, st.agent_id, tua.agent_id) AS agent_id \
     ) eff ON true ";

/// Each report priced at its own request's first LLM call.
///
/// Appended only by the queries that report money. `recorded_since` and the per-agent breakdown do
/// not, and running this per row for them would price the whole table to answer a question about
/// dates.
const PRICE_JOIN: &str = "LEFT JOIN LATERAL ( \
         SELECT (mp.input_price_per_1m / 1000000.0)::FLOAT8 AS rate \
           FROM (SELECT trace_id AS session_id, started_at AS created_at, provider, model FROM trace_first_calls) tu \
           JOIN model_pricing mp \
             ON mp.provider = tu.provider AND mp.model = tu.model \
            AND mp.effective_from <= cr.created_at \
            AND (mp.effective_until IS NULL OR mp.effective_until > cr.created_at) \
          WHERE tu.session_id = cr.trace_id \
          ORDER BY tu.created_at ASC, mp.effective_from DESC \
          LIMIT 1 \
     ) price ON true ";

#[derive(sqlx::FromRow)]
struct TotalsRow {
    reports: i64,
    priced_reports: i64,
    pool_tokens: i64,
    sent_tokens: i64,
    messages_dropped: i64,
    compressed_bytes: i64,
    pool_cost: f64,
    sent_cost: f64,
}

async fn totals(db: &PgPool, q: &ContextSavingsQuery<'_>) -> Result<TotalsRow, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT COALESCE(SUM(cr.reports), 0)::BIGINT AS reports, \
                COALESCE(SUM(CASE WHEN cr.frozen_priced IS NOT NULL THEN cr.frozen_priced \
                     WHEN price.rate IS NOT NULL THEN 1 ELSE 0 END), 0)::BIGINT AS priced_reports, \
                COALESCE(SUM(cr.pool_tokens), 0)::BIGINT AS pool_tokens, \
                COALESCE(SUM(cr.kept_tokens), 0)::BIGINT AS sent_tokens, \
                COALESCE(SUM(cr.pool_messages - cr.kept_messages), 0)::BIGINT AS messages_dropped, \
                COALESCE(SUM(cr.compressed_bytes_saved), 0)::BIGINT AS compressed_bytes, \
                COALESCE(SUM(COALESCE(cr.frozen_pool_cost, cr.pool_tokens * price.rate)), 0)::FLOAT8 AS pool_cost, \
                COALESCE(SUM(COALESCE(cr.frozen_sent_cost, cr.kept_tokens * price.rate)), 0)::FLOAT8 AS sent_cost ",
    );
    qb.push(REPORTS_FROM);
    qb.push(PRICE_JOIN);
    push_filters(&mut qb, q);
    qb.build_query_as().fetch_one(db).await
}

/// The first report in scope. Not the first in the window — a window that starts before we began
/// recording has to say so, and that is only knowable from the whole table.
async fn recorded_since(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    // Unfiltered — a superuser with no agent picked — is the common case and needs none of the
    // agent resolution below. A plain MIN walks the window index once instead of resolving an agent
    // for every report ever written.
    if q.agent_id.is_none() && q.accessible_agent_ids.is_none() && q.user_ids.is_none() {
        return sqlx::query_scalar(
            "SELECT MIN(created_at) FROM context_reports_spend WHERE created_at < $1",
        )
        .bind(q.end)
        .fetch_one(db)
        .await;
    }

    let mut qb = QueryBuilder::new("SELECT MIN(cr.created_at) ");
    qb.push(REPORTS_FROM);
    // Deliberately unbounded on the left: the question is when recording started, not what falls
    // inside the requested window.
    qb.push(" WHERE cr.created_at < ").push_bind(q.end);
    if let Some(agent) = q.agent_id {
        qb.push(" AND eff.agent_id = ").push_bind(agent);
    }
    if let Some(ids) = q.accessible_agent_ids {
        qb.push(" AND (eff.agent_id = ANY(")
            .push_bind(ids)
            .push(") OR (eff.agent_id IS NULL AND cr.user_id = ")
            .push_bind(q.caller_id)
            .push("))");
    }
    push_user_filter(&mut qb, q, "cr.user_id");
    qb.build_query_scalar().fetch_one(db).await
}

/// Requests that ran selection, reported or not.
///
/// Counted from the user's own turns rather than from `context_reports`, because the gap between
/// the two is the point: a turn whose history was empty ran selection and wrote no report, and the
/// page says "based on R of E requests" precisely to show that.
///
/// Callers pass the result through [`at_least_reports`]: this count alone can come out below the
/// number of reports, and a ratio above 1 is worse than an imprecise one.
async fn eligible_requests(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
    recorded_since: Option<DateTime<Utc>>,
) -> Result<i64, sqlx::Error> {
    // Nothing recorded yet means nothing was eligible — not that every turn in the window failed to
    // report. "Based on 0 of 116 requests" reads as a broken feature; "0 of 0" reads as what it is,
    // a window that starts before we began recording.
    let Some(floor) = recorded_since else {
        return Ok(0);
    };
    let from = q.start.max(floor);

    let mut qb = QueryBuilder::new(
        "SELECT COUNT(*)::BIGINT FROM chat_messages m \
           JOIN chat_sessions s ON s.session_id = m.session_id \
          WHERE m.role = 'user' AND m.timestamp >= ",
    );
    qb.push_bind(from)
        .push(" AND m.timestamp < ")
        .push_bind(q.end);
    if let Some(agent) = q.agent_id {
        qb.push(" AND s.agent_id = ").push_bind(agent);
    }
    if let Some(ids) = q.accessible_agent_ids {
        qb.push(" AND (s.agent_id = ANY(")
            .push_bind(ids)
            .push(") OR (s.agent_id IS NULL AND s.user_id = ")
            .push_bind(q.caller_id)
            .push("))");
    }
    push_user_filter(&mut qb, q, "s.user_id");
    qb.build_query_scalar().fetch_one(db).await
}

#[derive(sqlx::FromRow)]
struct BucketRow {
    bucket_start: DateTime<Utc>,
    reports: i64,
    pool_tokens: i64,
    sent_tokens: i64,
}

/// A report proves its own request was eligible, so eligibility can never be the smaller number.
///
/// Both figures are counted from different tables over the same window, and within one turn the
/// `chat_messages` row lands *before* the `context_reports` row. The turn that establishes
/// `recorded_since` therefore falls microseconds below its own floor and is missed by the count —
/// leaving the page to render "Based on 4 of 3 requests", a ratio above 1 that reads as a bug in
/// the figure rather than in the counting.
///
/// Raising the denominator rather than lowering the numerator keeps the claim conservative: the
/// share of requests we can speak for never overstates itself.
fn at_least_reports(counted: i64, reports: i64) -> i64 {
    counted.max(reports)
}

#[derive(sqlx::FromRow)]
struct EligibleBucketRow {
    bucket_start: DateTime<Utc>,
    eligible: i64,
}

async fn series(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
    bucket: Bucket,
    recorded_since: Option<DateTime<Utc>>,
) -> Result<Series, sqlx::Error> {
    let mut qb = QueryBuilder::new("SELECT date_trunc('");
    qb.push(bucket.trunc()).push(
        "', cr.created_at) AS bucket_start, \
                COALESCE(SUM(cr.reports), 0)::BIGINT AS reports, \
                COALESCE(SUM(cr.pool_tokens), 0)::BIGINT AS pool_tokens, \
                COALESCE(SUM(cr.kept_tokens), 0)::BIGINT AS sent_tokens ",
    );
    qb.push(REPORTS_FROM);
    push_filters(&mut qb, q);
    qb.push(" GROUP BY bucket_start");
    let rows: Vec<BucketRow> = qb.build_query_as().fetch_all(db).await?;

    // Same floor as the total, or the per-bucket counts would not sum to it.
    let eligible_from = recorded_since.map(|r| q.start.max(r));

    let mut eb = QueryBuilder::new("SELECT date_trunc('");
    eb.push(bucket.trunc()).push(
        "', m.timestamp) AS bucket_start, COUNT(*)::BIGINT AS eligible \
           FROM chat_messages m \
           JOIN chat_sessions s ON s.session_id = m.session_id \
          WHERE m.role = 'user' AND m.timestamp >= ",
    );
    eb.push_bind(eligible_from.unwrap_or(q.end))
        .push(" AND m.timestamp < ")
        .push_bind(q.end);
    if let Some(agent) = q.agent_id {
        eb.push(" AND s.agent_id = ").push_bind(agent);
    }
    if let Some(ids) = q.accessible_agent_ids {
        eb.push(" AND (s.agent_id = ANY(")
            .push_bind(ids)
            .push(") OR (s.agent_id IS NULL AND s.user_id = ")
            .push_bind(q.caller_id)
            .push("))");
    }
    push_user_filter(&mut eb, q, "s.user_id");
    eb.push(" GROUP BY bucket_start");
    let eligible: Vec<EligibleBucketRow> = match eligible_from {
        Some(_) => eb.build_query_as().fetch_all(db).await?,
        None => Vec::new(),
    };

    // Zero-fill in Rust rather than `generate_series`: the gaps are what the chart is for. A sparse
    // array would let a quiet day render as a narrower bar instead of an empty one, which reads as
    // "we saved less that day" rather than "nothing ran".
    let mut points = Vec::new();
    let mut at = bucket.floor(q.start);
    while at < q.end {
        let row = rows.iter().find(|r| r.bucket_start == at);
        let el = eligible.iter().find(|r| r.bucket_start == at);
        points.push(SavingsPoint {
            bucket_start: at,
            eligible_requests: at_least_reports(
                el.map_or(0, |r| r.eligible),
                row.map_or(0, |r| r.reports),
            ),
            reports: row.map_or(0, |r| r.reports),
            pool_tokens: row.map_or(0, |r| r.pool_tokens),
            sent_tokens: row.map_or(0, |r| r.sent_tokens),
        });
        at += bucket.step();
    }

    Ok(Series {
        bucket: bucket.label(),
        points,
    })
}

pub async fn get_context_savings(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
) -> Result<ContextSavings, sqlx::Error> {
    let totals = totals(db, q).await?;
    let priced = totals.priced_reports > 0;
    // Resolved first: it is the floor every "eligible" count below is measured from.
    let recorded_since = recorded_since(db, q).await?;

    Ok(ContextSavings {
        eligible_requests: at_least_reports(
            eligible_requests(db, q, recorded_since).await?,
            totals.reports,
        ),
        reports: totals.reports,
        priced_reports: totals.priced_reports,
        pool_tokens: totals.pool_tokens,
        sent_tokens: totals.sent_tokens,
        pool_cost_usd: priced.then_some(totals.pool_cost),
        sent_cost_usd: priced.then_some(totals.sent_cost),
        messages_dropped: totals.messages_dropped,
        compressed_bytes: totals.compressed_bytes,
        recorded_since,
        series: match q.series {
            Some(bucket) => Some(series(db, q, bucket, recorded_since).await?),
            None => None,
        },
    })
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    agent_id: Uuid,
    name: String,
    reports: i64,
    pool_tokens: i64,
    sent_tokens: i64,
    compress_enabled: bool,
}

pub async fn get_by_agent(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
) -> Result<Vec<AgentContextSavings>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT eff.agent_id AS agent_id, \
                COALESCE(a.display_name, a.name) AS name, \
                COALESCE(SUM(cr.reports), 0)::BIGINT AS reports, \
                COALESCE(SUM(cr.pool_tokens), 0)::BIGINT AS pool_tokens, \
                COALESCE(SUM(cr.kept_tokens), 0)::BIGINT AS sent_tokens, \
                bool_or(a.compress_enabled) AS compress_enabled ",
    );
    qb.push(REPORTS_FROM);
    qb.push(" JOIN agents a ON a.id = eff.agent_id AND a.deleted_at IS NULL ");
    push_filters(&mut qb, q);
    // A report with no resolvable agent stays in the totals and is left out here: it is real
    // traffic, but it belongs to no row of this table.
    // Grouped by the underlying columns, not the `name` output alias: `name` is also a column of
    // `agents`, and Postgres binds the input column in preference to the alias — which silently
    // leaves `display_name` ungrouped and fails the query.
    qb.push(
        " AND eff.agent_id IS NOT NULL           GROUP BY eff.agent_id, a.display_name, a.name           ORDER BY pool_tokens DESC LIMIT ",
    );
    qb.push_bind(q.limit);

    let rows: Vec<AgentRow> = qb.build_query_as().fetch_all(db).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Eligible counts for the whole page in one grouped query. Asking per row would be one round
    // trip per agent, and this list is capped at 50.
    let ids: Vec<Uuid> = rows.iter().map(|r| r.agent_id).collect();
    let eligible: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT s.agent_id, COUNT(*)::BIGINT FROM chat_messages m \
           JOIN chat_sessions s ON s.session_id = m.session_id \
          WHERE m.role = 'user' AND m.timestamp >= $1 AND m.timestamp < $2 \
            AND s.agent_id = ANY($3) \
            AND ($4::uuid[] IS NULL OR s.user_id = ANY($4)) \
          GROUP BY s.agent_id",
    )
    // Unprepared: a prepared statement's generic plan (from its sixth run) can't tell `$4 IS NULL`
    // from a list of people, so it reads every caller's messages in the window.
    .persistent(false)
    .bind(q.start)
    .bind(q.end)
    .bind(&ids)
    .bind(q.user_ids)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AgentContextSavings {
            requests: eligible
                .iter()
                .find(|(id, _)| *id == r.agent_id)
                .map_or(r.reports, |(_, n)| *n),
            agent_id: r.agent_id,
            name: r.name,
            reports: r.reports,
            pool_tokens: r.pool_tokens,
            sent_tokens: r.sent_tokens,
            compress_enabled: r.compress_enabled,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct TopRow {
    owner_id: Option<Uuid>,
    trace_id: Option<String>,
    span_id: Option<String>,
    session_id: Option<String>,
    chat_title: Option<String>,
    agent_id: Uuid,
    agent_name: String,
    pool_tokens: i64,
    sent_tokens: i64,
    compress_enabled: Option<bool>,
    started_at: DateTime<Utc>,
}

pub async fn get_top_requests(
    db: &PgPool,
    q: &ContextSavingsQuery<'_>,
) -> Result<Vec<TopRequest>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT cr.user_id AS owner_id, cr.trace_id, cr.span_id, cr.session_id, \
                cs.title AS chat_title, eff.agent_id AS agent_id, \
                COALESCE(a.display_name, a.name) AS agent_name, \
                cr.pool_tokens, cr.kept_tokens AS sent_tokens, \
                a.compress_enabled, cr.created_at AS started_at ",
    );
    // Row-level list: only hot rows have a trace to open.
    qb.push(REPORTS_FROM.replace("FROM context_reports_spend cr", "FROM context_reports cr"));
    qb.push(
        " JOIN agents a ON a.id = eff.agent_id AND a.deleted_at IS NULL \
          LEFT JOIN chat_sessions cs ON cs.session_id = cr.session_id ",
    );
    push_filters(&mut qb, q);
    // Every row on this list names an agent: the UI's schema requires one, and a row it cannot
    // attribute is not a row anybody can act on. Those requests stay in the totals.
    qb.push(" AND eff.agent_id IS NOT NULL");
    // One chart bar's interval, inside the window the filters already applied.
    if let Some(from) = q.from {
        qb.push(" AND cr.created_at >= ").push_bind(from);
    }
    if let Some(to) = q.to {
        qb.push(" AND cr.created_at < ").push_bind(to);
    }
    qb.push(" ORDER BY cr.pool_tokens DESC LIMIT ")
        .push_bind(q.limit);

    let rows: Vec<TopRow> = qb.build_query_as().fetch_all(db).await?;

    Ok(rows.into_iter().map(|r| redact(r, q)).collect())
}

/// Blank everything that identifies someone else's conversation.
///
/// Access to an agent is not access to the chats that used it. Without this, any user a shared
/// agent is granted to could read every other user's chat titles off this list and open their
/// traces. Tokens and the timestamp stay: they are the row's reason for being on the page.
fn redact(r: TopRow, q: &ContextSavingsQuery<'_>) -> TopRequest {
    let own = q.caller_is_superuser || r.owner_id == Some(q.caller_id);
    TopRequest {
        own,
        trace_id: own.then_some(r.trace_id).flatten(),
        span_id: own.then_some(r.span_id).flatten(),
        session_id: own.then_some(r.session_id).flatten(),
        chat_title: own.then_some(r.chat_title).flatten(),
        agent_id: r.agent_id,
        agent_name: r.agent_name,
        pool_tokens: r.pool_tokens,
        sent_tokens: r.sent_tokens,
        compress_enabled: r.compress_enabled.unwrap_or(false),
        started_at: r.started_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query<'a>(caller: Uuid, superuser: bool) -> ContextSavingsQuery<'a> {
        ContextSavingsQuery {
            start: Utc::now() - Duration::days(30),
            end: Utc::now(),
            agent_id: None,
            caller_id: caller,
            caller_is_superuser: superuser,
            accessible_agent_ids: None,
            user_ids: None,
            series: None,
            limit: 10,
            from: None,
            to: None,
        }
    }

    fn row(owner: Uuid) -> TopRow {
        TopRow {
            owner_id: Some(owner),
            trace_id: Some("4f1c9a7e".into()),
            span_id: Some("9a3e5c7b".into()),
            session_id: Some("ses_1".into()),
            chat_title: Some("Review PR #481".into()),
            agent_id: Uuid::new_v4(),
            agent_name: "PR reviewer".into(),
            pool_tokens: 38_200,
            sent_tokens: 9_100,
            compress_enabled: Some(true),
            started_at: Utc::now(),
        }
    }

    #[test]
    fn your_own_request_comes_back_whole() {
        let me = Uuid::new_v4();
        let out = redact(row(me), &query(me, false));
        assert!(out.own);
        assert_eq!(out.chat_title.as_deref(), Some("Review PR #481"));
        assert!(out.trace_id.is_some());
    }

    /// Agent access is not chat access. This is the whole reason the endpoint exists in this shape.
    #[test]
    fn another_users_request_loses_everything_that_identifies_their_chat() {
        let out = redact(row(Uuid::new_v4()), &query(Uuid::new_v4(), false));
        assert!(!out.own);
        assert_eq!(out.chat_title, None);
        assert_eq!(out.trace_id, None);
        assert_eq!(out.span_id, None);
        assert_eq!(out.session_id, None);
        // Still a row on the page: the tokens are why it is listed at all.
        assert_eq!(out.pool_tokens, 38_200);
        assert_eq!(out.agent_name, "PR reviewer");
    }

    #[test]
    fn a_superuser_sees_every_row_in_full() {
        let out = redact(row(Uuid::new_v4()), &query(Uuid::new_v4(), true));
        assert!(out.own);
        assert!(out.chat_title.is_some());
    }

    /// The live symptom this guards: four reports against three counted turns rendered as
    /// "Based on 4 of 3 requests".
    #[test]
    fn eligibility_is_never_smaller_than_the_reports_it_covers() {
        assert_eq!(at_least_reports(3, 4), 4);
        assert_eq!(at_least_reports(0, 1), 1);
    }

    /// The clamp must not invent eligibility where the count is already the larger figure — a turn
    /// that selected over empty history is eligible and reports nothing, which is the gap the page
    /// exists to show.
    #[test]
    fn a_larger_count_is_left_alone() {
        assert_eq!(at_least_reports(116, 4), 116);
        assert_eq!(at_least_reports(0, 0), 0);
    }

    /// Every per-agent view filters on the resolved agent, so the chain that resolves it is the
    /// difference between those views working and being permanently empty.
    #[test]
    fn the_agent_resolution_chain_has_all_three_sources() {
        assert!(REPORTS_FROM.contains("COALESCE(cr.agent_id, st.agent_id, tua.agent_id)"));
        // The trace's calls (`trace_first_calls`, hot or tiered) recover orchestrator-routed turns: the orchestrator writes
        // its own flow row with a NULL root_agent_id, so without this they resolve to nothing.
        assert!(REPORTS_FROM.contains("FROM trace_first_calls) tu"));
        // A NULL in an earlier source must not win the LIMIT 1 and mask a later one.
        assert!(REPORTS_FROM.contains("f.root_agent_id IS NOT NULL"));
        assert!(REPORTS_FROM.contains("tu.agent_id IS NOT NULL"));
    }

    #[test]
    fn series_values_are_the_spelling_the_ui_sends() {
        assert_eq!(Bucket::parse(None).unwrap(), None);
        assert_eq!(Bucket::parse(Some("daily")).unwrap(), Some(Bucket::Day));
        assert_eq!(Bucket::parse(Some("hourly")).unwrap(), Some(Bucket::Hour));
        let err = Bucket::parse(Some("weekly")).expect_err("weekly is not a bucket");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_day_bucket_floors_to_utc_midnight() {
        let at: DateTime<Utc> = "2026-10-04T14:22:08Z".parse().unwrap();
        assert_eq!(
            Bucket::Day.floor(at).to_rfc3339(),
            "2026-10-04T00:00:00+00:00"
        );
        assert_eq!(
            Bucket::Hour.floor(at).to_rfc3339(),
            "2026-10-04T14:00:00+00:00"
        );
    }
}
