//! `GET /api/observability/coding-agents/usage` — coding-harness usage (H-1).
//!
//! Replaces a 404 plus a client-side rebuild. Without this route the Harnesses
//! page detects the bare 404 and reconstructs the viewer's own usage from
//! `/finops/dashboard?my_agent=true`, `/api/agents` and `/api/chat/sessions`
//! (`ui/common/src/features/harnesses/liveIndividual.ts`) — several requests
//! that still cannot produce a trend, a daily strip, top models, or a correct
//! per-harness "unpriced".
//!
//! **OSS answers one level: the caller's own.** There is no org tree here, so
//! every `scope` and every `user_id` but the caller's is "not visible". That is
//! a policy of this endpoint, deliberately not derived from
//! `AuthService::org_visible_user_ids`, whose `None` means *unrestricted*. EE
//! layers the org levels on top (H-2).
//!
//! Errors carry `{error, code}`, never the standard envelope: a 404 **without**
//! a code is how the UI detects that the endpoint is absent, so a coded 404 is
//! "not visible" and an uncoded one would silently re-enable the fallback.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

use crate::auth::Claims;
use crate::state::AppState;

/// Sum costs without emitting negative zero.
///
/// Rust's `Sum` for floats folds from `-0.0` (the IEEE additive identity), so
/// summing an empty iterator serializes as `-0.0` and a developer with no
/// activity would read "-$0.00".
fn sum_cost(costs: impl Iterator<Item = f64>) -> f64 {
    costs.fold(0.0, |a, b| a + b)
}

/// Harness sessions listed at the Individual level, newest first.
const RECENT_SESSION_LIMIT: i64 = 20;
/// Model names per harness in `top_models`, by turns.
const TOP_MODELS: usize = 2;

// ─── Request ─────────────────────────────────────────────────────────────────

/// Everything arrives as a string and is parsed here. A typed extractor would
/// answer axum's own plain-text 400, which the UI cannot route to a level.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct UsageParams {
    pub scope: Option<String>,
    pub unit_id: Option<String>,
    pub user_id: Option<String>,
    pub group_by: Option<String>,
    pub range: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub compare: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
}

// ─── Response ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Serialize, utoipa::ToSchema)]
pub struct HarnessTotals {
    /// Distinct active, non-deleted users in scope. Always 1 here: the caller.
    pub scope_devs: i64,
    pub active_devs: i64,
    /// Users with at least one non-deleted agent row for (owner, harness).
    pub registered_devs: i64,
    /// Registered (developer, harness) pairs with no turn in the window.
    pub idle_seats: i64,
    pub sessions: i64,
    pub turns: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub unpriced_calls: i64,
    /// Change in **turns** against the previous window, one decimal. `null`
    /// without `compare`, and when the previous window had no turns — an
    /// undefined percentage, not a fabricated 0.
    pub delta_pct: Option<f64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HarnessEntry {
    pub harness: String,
    pub top_models: Vec<String>,
    #[serde(flatten)]
    pub totals: HarnessTotals,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsageScope {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub label: String,
    pub visibility: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsageWindow {
    pub start_time: String,
    pub end_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesPoint {
    pub date: String,
    pub harness: String,
    pub active_devs: i64,
    pub cost_usd: f64,
    pub tokens: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecentSession {
    pub session_id: String,
    pub harness: String,
    pub started_at: String,
    pub turns: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsageResponse {
    pub scope: UsageScope,
    pub window: UsageWindow,
    pub totals: HarnessTotals,
    pub by_harness: Vec<HarnessEntry>,
    /// The scope's children. Always empty at the Individual level.
    #[schema(value_type = Vec<Object>)]
    pub rows: Vec<serde_json::Value>,
    pub overlap_devs: i64,
    pub series: Vec<SeriesPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_sessions: Option<Vec<RecentSession>>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub prev_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UsageEnvelope {
    pub data: UsageResponse,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

/// `{error, code}`. Every 404 from this handler must carry a code — an uncoded
/// one tells the UI the route does not exist and re-enables the fallback.
fn err(status: StatusCode, message: &str, code: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": message, "code": code })),
    )
        .into_response()
}

fn not_visible_unit() -> Response {
    err(
        StatusCode::NOT_FOUND,
        "unit not found or not visible",
        "unit_not_visible",
    )
}

fn not_visible_user() -> Response {
    err(
        StatusCode::NOT_FOUND,
        "user not found or not visible",
        "user_not_visible",
    )
}

// ─── Handler ─────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/observability/coding-agents/usage",
    tag = "observability",
    params(UsageParams),
    responses(
        (status = 200, description = "Usage for the level", body = UsageEnvelope),
        (status = 400, description = "Malformed scope, group_by, range, window or cursor"),
        (status = 404, description = "Level not found or not visible (always coded)"),
    ),
)]
#[tracing::instrument(skip(state))]
pub async fn get_coding_agent_usage(
    State(state): State<AppState>,
    claims: Claims,
    Query(p): Query<UsageParams>,
) -> Response {
    // Validation order matters: a bad value is a 400 in every edition, checked
    // before the visibility rule, so a typo never reads as "not visible".
    if let Some(scope) = p.scope.as_deref()
        && !matches!(scope, "org" | "mine" | "unassigned" | "unit" | "direct")
    {
        return err(
            StatusCode::BAD_REQUEST,
            "scope must be one of org, mine, unassigned, unit, direct",
            "invalid_scope",
        );
    }
    if matches!(p.scope.as_deref(), Some("unit") | Some("direct")) && p.unit_id.is_none() {
        return err(
            StatusCode::BAD_REQUEST,
            "unit_id is required for this scope",
            "missing_unit_id",
        );
    }
    if let Some(g) = p.group_by.as_deref()
        && !matches!(g, "unit" | "user")
    {
        return err(
            StatusCode::BAD_REQUEST,
            "group_by must be unit or user",
            "invalid_group_by",
        );
    }
    let window = match resolve_window(&p) {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    // Only developer lists page, and OSS never returns one, so any cursor here
    // was issued for a list this edition cannot produce.
    if p.cursor.is_some() {
        return err(
            StatusCode::BAD_REQUEST,
            "cursor was not issued for this list",
            "invalid_cursor",
        );
    }

    // OSS has no org tree: every org level is invisible, and the only readable
    // developer is the caller.
    if p.scope.is_some() {
        return not_visible_unit();
    }
    let Ok(caller) = claims.user_uuid() else {
        return not_visible_user();
    };
    if let Some(requested) = p.user_id.as_deref()
        && requested != caller.to_string()
    {
        return not_visible_user();
    }

    match individual(&state.db, caller, &claims, &window, p.compare.is_some()).await {
        Ok(data) => Json(UsageEnvelope { data }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "coding-agents usage failed");
            err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
                "internal",
            )
        }
    }
}

struct Window {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    range: Option<String>,
}

#[allow(clippy::result_large_err)] // one early-return per request, not a hot loop
fn resolve_window(p: &UsageParams) -> Result<Window, Response> {
    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let end = match p.end_time.as_deref() {
        None => Utc::now(),
        Some(s) => match parse(s) {
            Some(d) => d,
            None => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "end_time is not RFC 3339",
                    "invalid_window",
                ));
            }
        },
    };
    if let Some(r) = p.range.as_deref() {
        let hours = match r {
            "24h" => 24,
            "7d" => 24 * 7,
            "30d" => 24 * 30,
            _ => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "range must be one of 24h, 7d, 30d",
                    "invalid_range",
                ));
            }
        };
        return Ok(Window {
            start: end - Duration::hours(hours),
            end,
            range: Some(r.to_string()),
        });
    }
    let start = match p.start_time.as_deref() {
        None => end - Duration::days(30),
        Some(s) => match parse(s) {
            Some(d) => d,
            None => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "start_time is not RFC 3339",
                    "invalid_window",
                ));
            }
        },
    };
    if start >= end {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "start_time must be before end_time",
            "invalid_window",
        ));
    }
    Ok(Window {
        start,
        end,
        range: None,
    })
}

/// Per-harness activity for one developer in one window.
#[derive(sqlx::FromRow)]
struct ActivityRow {
    harness: String,
    sessions: i64,
    turns: i64,
    tokens: i64,
    cost_usd: f64,
    unpriced_calls: i64,
}

/// A developer's own usage: the only level OSS answers.
async fn individual(
    db: &PgPool,
    user: uuid::Uuid,
    claims: &Claims,
    w: &Window,
    compare: bool,
) -> Result<UsageResponse, sqlx::Error> {
    // Harness activity is `trace_usage.user_id` joined to the agent's
    // `coding_agent_integration_id`. An agent row deleted since the turn ran
    // still counts (the join is on the id, not on liveness), which is what
    // "activity through a since-deleted registration still counts" means.
    const ACTIVITY: &str = r#"
        SELECT a.coding_agent_integration_id AS harness,
               count(DISTINCT u.session_id)::bigint AS sessions,
               count(*)::bigint AS turns,
               COALESCE(sum(u.input_tokens + u.output_tokens
                            + u.cache_read_tokens + u.cache_creation_tokens), 0)::bigint AS tokens,
               COALESCE(sum(u.cost_usd), 0)::float8 AS cost_usd,
               count(*) FILTER (
                   WHERE u.cost_usd = 0
                     AND (u.input_tokens + u.output_tokens
                          + u.cache_read_tokens + u.cache_creation_tokens) > 0
               )::bigint AS unpriced_calls
          FROM trace_usage u
          JOIN agents a ON a.id = u.agent_id
         WHERE u.user_id = $1
           AND a.coding_agent_integration_id IS NOT NULL
           AND u.started_at >= $2 AND u.started_at < $3
         GROUP BY a.coding_agent_integration_id
    "#;

    let current: Vec<ActivityRow> = sqlx::query_as(ACTIVITY)
        .bind(user)
        .bind(w.start)
        .bind(w.end)
        .fetch_all(db)
        .await?;

    // Registered seats: live agent rows only. A reinstall is still one seat
    // (the unique index is on (owner_id, coding_agent_integration_id) where not
    // deleted), so this is a set, not a count of rows.
    let registered: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT coding_agent_integration_id FROM agents \
         WHERE owner_id = $1 AND deleted_at IS NULL \
           AND coding_agent_integration_id IS NOT NULL",
    )
    .bind(user)
    .fetch_all(db)
    .await?;
    let registered: HashSet<String> = registered.into_iter().collect();

    // Previous window of equal length, for `delta_pct` on turns.
    let previous: HashMap<String, i64> = if compare {
        let len = w.end - w.start;
        let rows: Vec<ActivityRow> = sqlx::query_as(ACTIVITY)
            .bind(user)
            .bind(w.start - len)
            .bind(w.start)
            .fetch_all(db)
            .await?;
        rows.into_iter().map(|r| (r.harness, r.turns)).collect()
    } else {
        HashMap::new()
    };

    let top_models = top_models_by_harness(db, user, w).await?;

    // Every harness the developer is registered on or active with, so an idle
    // seat still appears with zeros rather than vanishing.
    let mut harnesses: Vec<String> = current
        .iter()
        .map(|r| r.harness.clone())
        .chain(registered.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    harnesses.sort();

    let by_name: HashMap<&str, &ActivityRow> =
        current.iter().map(|r| (r.harness.as_str(), r)).collect();

    let delta = |harness: &str, turns: i64| -> Option<f64> {
        if !compare {
            return None;
        }
        let before = previous.get(harness).copied().unwrap_or(0);
        (before > 0).then(|| (((turns - before) as f64 / before as f64) * 1000.0).round() / 10.0)
    };

    let by_harness: Vec<HarnessEntry> = harnesses
        .iter()
        .map(|h| {
            let a = by_name.get(h.as_str());
            let turns = a.map_or(0, |r| r.turns);
            let is_registered = registered.contains(h);
            HarnessEntry {
                harness: h.clone(),
                top_models: top_models.get(h).cloned().unwrap_or_default(),
                totals: HarnessTotals {
                    scope_devs: 1,
                    active_devs: i64::from(turns > 0),
                    registered_devs: i64::from(is_registered),
                    // A registered seat with no turn in the window.
                    idle_seats: i64::from(is_registered && turns == 0),
                    sessions: a.map_or(0, |r| r.sessions),
                    turns,
                    tokens: a.map_or(0, |r| r.tokens),
                    cost_usd: a.map_or(0.0, |r| r.cost_usd),
                    unpriced_calls: a.map_or(0, |r| r.unpriced_calls),
                    delta_pct: delta(h, turns),
                },
            }
        })
        .collect();

    // The developer counts once across harnesses; seats and volumes sum.
    let total_turns: i64 = current.iter().map(|r| r.turns).sum();
    let prev_total: i64 = previous.values().sum();
    let totals = HarnessTotals {
        scope_devs: 1,
        active_devs: i64::from(total_turns > 0),
        registered_devs: i64::from(!registered.is_empty()),
        idle_seats: by_harness.iter().map(|e| e.totals.idle_seats).sum(),
        sessions: current.iter().map(|r| r.sessions).sum(),
        turns: total_turns,
        tokens: current.iter().map(|r| r.tokens).sum(),
        cost_usd: sum_cost(current.iter().map(|r| r.cost_usd)),
        unpriced_calls: current.iter().map(|r| r.unpriced_calls).sum(),
        delta_pct: (compare && prev_total > 0).then(|| {
            (((total_turns - prev_total) as f64 / prev_total as f64) * 1000.0).round() / 10.0
        }),
    };

    Ok(UsageResponse {
        scope: UsageScope {
            kind: "user".into(),
            user_id: Some(user.to_string()),
            label: display_name(db, user, claims).await?,
            visibility: "named".into(),
        },
        window: UsageWindow {
            start_time: super::service::fmt_ts(w.start),
            end_time: super::service::fmt_ts(w.end),
            range: w.range.clone(),
        },
        totals,
        by_harness,
        // The Individual level has no children.
        rows: vec![],
        overlap_devs: 0,
        series: series(db, user, w).await?,
        recent_sessions: Some(recent_sessions(db, user, w).await?),
        has_more: false,
        next_cursor: None,
        prev_cursor: None,
    })
}

async fn display_name(
    db: &PgPool,
    user: uuid::Uuid,
    claims: &Claims,
) -> Result<String, sqlx::Error> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT display_name, username FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(db)
            .await?;
    Ok(row
        .map(|(display, username)| display.filter(|d| !d.is_empty()).unwrap_or(username))
        .unwrap_or_else(|| claims.sub.clone()))
}

/// Up to `TOP_MODELS` model names per harness, by turns.
async fn top_models_by_harness(
    db: &PgPool,
    user: uuid::Uuid,
    w: &Window,
) -> Result<HashMap<String, Vec<String>>, sqlx::Error> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT a.coding_agent_integration_id AS harness, u.model, count(*)::bigint AS turns \
           FROM trace_usage u \
           JOIN agents a ON a.id = u.agent_id \
          WHERE u.user_id = $1 \
            AND a.coding_agent_integration_id IS NOT NULL \
            AND u.model IS NOT NULL \
            AND u.started_at >= $2 AND u.started_at < $3 \
          GROUP BY 1, 2 \
          ORDER BY 1, 3 DESC, 2",
    )
    .bind(user)
    .bind(w.start)
    .bind(w.end)
    .fetch_all(db)
    .await?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (harness, model, _) in rows {
        let slot = out.entry(harness).or_default();
        if slot.len() < TOP_MODELS {
            slot.push(model);
        }
    }
    Ok(out)
}

/// Per UTC day × harness, sorted by date then harness, so the trend needs no
/// second request.
async fn series(
    db: &PgPool,
    user: uuid::Uuid,
    w: &Window,
) -> Result<Vec<SeriesPoint>, sqlx::Error> {
    let rows: Vec<(DateTime<Utc>, String, i64, f64, i64)> = sqlx::query_as(
        "SELECT date_trunc('day', u.started_at) AS day, \
                a.coding_agent_integration_id AS harness, \
                count(*)::bigint AS turns, \
                COALESCE(sum(u.cost_usd), 0)::float8 AS cost_usd, \
                COALESCE(sum(u.input_tokens + u.output_tokens \
                             + u.cache_read_tokens + u.cache_creation_tokens), 0)::bigint AS tokens \
           FROM trace_usage u \
           JOIN agents a ON a.id = u.agent_id \
          WHERE u.user_id = $1 \
            AND a.coding_agent_integration_id IS NOT NULL \
            AND u.started_at >= $2 AND u.started_at < $3 \
          GROUP BY 1, 2 \
          ORDER BY 1, 2",
    )
    .bind(user)
    .bind(w.start)
    .bind(w.end)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(day, harness, turns, cost_usd, tokens)| SeriesPoint {
            date: day.format("%Y-%m-%d").to_string(),
            harness,
            // One developer in scope: active on a day means any turn that day.
            active_devs: i64::from(turns > 0),
            cost_usd,
            tokens,
        })
        .collect())
}

async fn recent_sessions(
    db: &PgPool,
    user: uuid::Uuid,
    w: &Window,
) -> Result<Vec<RecentSession>, sqlx::Error> {
    let rows: Vec<(String, String, DateTime<Utc>, i64, f64)> = sqlx::query_as(
        "SELECT u.session_id, \
                a.coding_agent_integration_id AS harness, \
                min(u.started_at) AS started_at, \
                count(*)::bigint AS turns, \
                COALESCE(sum(u.cost_usd), 0)::float8 AS cost_usd \
           FROM trace_usage u \
           JOIN agents a ON a.id = u.agent_id \
          WHERE u.user_id = $1 \
            AND a.coding_agent_integration_id IS NOT NULL \
            AND u.session_id IS NOT NULL \
            AND u.started_at >= $2 AND u.started_at < $3 \
          GROUP BY u.session_id, a.coding_agent_integration_id \
          ORDER BY started_at DESC \
          LIMIT $4",
    )
    .bind(user)
    .bind(w.start)
    .bind(w.end)
    .bind(RECENT_SESSION_LIMIT)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(session_id, harness, started_at, turns, cost_usd)| RecentSession {
                session_id,
                harness,
                started_at: super::service::fmt_ts(started_at),
                turns,
                cost_usd,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> UsageParams {
        UsageParams {
            scope: None,
            unit_id: None,
            user_id: None,
            group_by: None,
            range: None,
            start_time: None,
            end_time: None,
            compare: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn a_range_anchors_to_end_time() {
        let p = UsageParams {
            range: Some("7d".into()),
            end_time: Some("2026-10-13T00:00:00Z".into()),
            ..params()
        };
        let Ok(w) = resolve_window(&p) else {
            panic!("valid")
        };
        assert_eq!(w.end.to_rfc3339(), "2026-10-13T00:00:00+00:00");
        assert_eq!(w.start.to_rfc3339(), "2026-10-06T00:00:00+00:00");
        assert_eq!(w.range.as_deref(), Some("7d"));
    }

    #[test]
    fn the_default_window_is_thirty_days_and_carries_no_range() {
        let Ok(w) = resolve_window(&params()) else {
            panic!("valid")
        };
        assert!(w.range.is_none(), "no range was asked for");
        assert_eq!((w.end - w.start).num_days(), 30);
    }

    #[test]
    fn an_unknown_range_is_rejected() {
        let p = UsageParams {
            range: Some("90d".into()),
            ..params()
        };
        assert!(resolve_window(&p).is_err());
    }

    #[test]
    fn a_start_at_or_after_the_end_is_rejected() {
        // Equal bounds would answer 200 with an empty report, which reads as
        // "no usage" rather than "you asked for nothing".
        let p = UsageParams {
            start_time: Some("2026-10-13T00:00:00Z".into()),
            end_time: Some("2026-10-13T00:00:00Z".into()),
            ..params()
        };
        assert!(resolve_window(&p).is_err());
    }

    #[test]
    fn an_empty_cost_sum_is_positive_zero() {
        // `[].iter().sum::<f64>()` is -0.0, which serializes as `-0.0` and
        // renders as "-$0.00" for a developer with no activity.
        let empty: Vec<f64> = vec![];
        assert!(empty.iter().copied().sum::<f64>().is_sign_negative());
        assert!(sum_cost(empty.into_iter()).is_sign_positive());
        assert_eq!(sum_cost([1.5, 2.25].into_iter()), 3.75);
    }

    #[test]
    fn an_unparseable_bound_is_rejected() {
        let p = UsageParams {
            start_time: Some("last tuesday".into()),
            ..params()
        };
        assert!(resolve_window(&p).is_err());
    }
}
