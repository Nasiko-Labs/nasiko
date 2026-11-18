//! `/api/overview/*` — the Overview's estate and what people did about its problems
//! (ui/docs/requirements/overview.md OV-1, OV-2).
//!
//! - `GET /overview/estate?range=7d|30d|90d` — every node the map draws, the flows between its
//!   cards, and the problems, cause chains and opportunities found in them, scoped to what the
//!   caller can see. Volumes and spend cover the range; problems are "now" (the last week against
//!   the one before), whatever the range.
//! - `PATCH /overview/problems/{id}` — who is on a problem, a snooze, or "handled". Answers the
//!   problem as it now stands.
//!
//! Problems are computed on every read, never stored: only the decisions people make about them
//! are (`overview_problem_state`). Responses use the `{data}` envelope; errors are plain text.

mod estate;
mod load;
mod rules;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use nasiko_mcp_gateway::error::McpError;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::Claims;
use crate::catalog::routes::listing_scope;
use crate::state::AppState;
use load::Windows;
use rules::{Decision, Problem};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/overview/estate", get(get_estate))
        .route("/overview/problems/{id}", patch(patch_problem))
}

#[derive(Deserialize)]
struct EstateQuery {
    range: Option<String>,
}

async fn get_estate(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<EstateQuery>,
) -> Response {
    let Some(range) = Range::parse(q.range.as_deref()) else {
        return (StatusCode::BAD_REQUEST, "range must be 7d, 30d or 90d").into_response();
    };
    match estate_for(&state, &claims, range, Utc::now()).await {
        Ok(estate) => Json(Envelope { data: estate }).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn patch_problem(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
    Json(body): Json<ProblemPatch>,
) -> Response {
    match apply_patch(&state, &claims, &id, body).await {
        Ok(problem) => Json(Envelope { data: problem }).into_response(),
        Err(e) => e.into_response(),
    }
}

/// The estate as `claims` may see it, with everyone's decisions on its problems applied.
async fn estate_for(
    state: &AppState,
    claims: &Claims,
    range: Range,
    now: DateTime<Utc>,
) -> Result<estate::Estate, OverviewError> {
    let scope = Scope::of(state, claims).await?;
    let snapshot = load::snapshot(state, &scope, Windows::new(range, now)).await?;
    let mut estate = estate::build(&snapshot);
    let ids: Vec<String> = estate.problems.iter().map(|p| p.id.clone()).collect();
    let decisions = decisions(&state.db, &ids).await?;
    rules::apply_decisions(&mut estate.problems, &decisions, now);
    Ok(estate)
}

/// Records one decision on a problem the caller can see, and answers the problem with it applied.
async fn apply_patch(
    state: &AppState,
    claims: &Claims,
    id: &str,
    body: ProblemPatch,
) -> Result<Problem, OverviewError> {
    let change = body.into_change()?;
    let me = claims
        .user_uuid()
        .map_err(|e| OverviewError::Rejected(e.into_response()))?;
    let now = Utc::now();
    // Problems are "now" whatever the range: any range finds the same ones.
    let estate = estate_for(state, claims, Range::Days30, now).await?;
    let mut problem = estate
        .problems
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(OverviewError::NoSuchProblem)?;
    let current = decisions(&state.db, &[id.to_owned()])
        .await?
        .into_iter()
        .next()
        .unwrap_or_else(|| Decision::new(id));
    let next = current.with(change, now);
    save_decision(&state.db, id, &next, me).await?;
    rules::apply_decisions(std::slice::from_mut(&mut problem), &[next], now);
    Ok(problem)
}

#[derive(Serialize)]
struct Envelope<T> {
    data: T,
}

/// The range the page shows. Only volumes and spend follow it; problems always read the last week.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Range {
    Days7,
    Days30,
    Days90,
}

impl Range {
    /// `None` (no `range`) is the page's default, 30 days; an unknown value is `None`.
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw {
            Some("7d") => Some(Range::Days7),
            None | Some("30d") => Some(Range::Days30),
            Some("90d") => Some(Range::Days90),
            Some(_) => None,
        }
    }

    pub(super) fn days(self) -> i64 {
        match self {
            Range::Days7 => 7,
            Range::Days30 => 30,
            Range::Days90 => 90,
        }
    }
}

/// What the caller may see. A superuser sees the whole workspace; anyone else sees the agents the
/// catalog lists for them, their own workflows and configs, and themselves (plus, in an edition
/// with an org chart, the people it opens up).
pub(super) struct Scope {
    pub me: Uuid,
    pub superuser: bool,
    /// Whether the caller sees everyone's usage (`AuthService::can_manage_users`: a superuser or an
    /// admin), the same rule the FinOps pages follow. Anyone else sees only their own calls.
    pub sees_everyone: bool,
    pub org_granted: Vec<String>,
    /// `None`: everyone.
    pub visible_people: Option<Vec<Uuid>>,
}

impl Scope {
    async fn of(state: &AppState, claims: &Claims) -> Result<Self, OverviewError> {
        let me = claims
            .user_uuid()
            .map_err(|e| OverviewError::Rejected(e.into_response()))?;
        let listing = listing_scope(state, claims)
            .await
            .map_err(OverviewError::Rejected)?;
        let identity: nasiko_auth::Identity = claims.clone().into();
        let sees_everyone = state.auth.can_manage_users(&identity).await;
        let visible_people = if claims.is_superuser {
            None
        } else {
            let mut ids: Vec<Uuid> = state
                .auth
                .org_visible_user_ids(&identity)
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|id| Uuid::parse_str(id).ok())
                .collect();
            ids.push(me);
            Some(ids)
        };
        Ok(Scope {
            me,
            superuser: claims.is_superuser,
            sees_everyone,
            org_granted: listing.org_granted,
            visible_people,
        })
    }

    /// The caller for queries that narrow to "theirs"; `None` lets a superuser see everything.
    pub(super) fn listing_user(&self) -> Option<Uuid> {
        (!self.superuser).then_some(self.me)
    }
}

/// OV-2's body: exactly one change. `null` clears an owner or a snooze; `handled: false` reopens.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProblemPatch {
    #[serde(default, deserialize_with = "present")]
    owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    snoozed_until: Option<Option<DateTime<Utc>>>,
    handled: Option<bool>,
}

/// Tells a field sent as `null` (clear it) from a field not sent at all.
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl ProblemPatch {
    fn into_change(self) -> Result<rules::Change, OverviewError> {
        use rules::Change;
        match (self.owner_id, self.snoozed_until, self.handled) {
            (Some(owner), None, None) => Ok(Change::Owner(owner)),
            (None, Some(until), None) => Ok(Change::Snooze(until)),
            (None, None, Some(handled)) => Ok(Change::Handled(handled)),
            _ => Err(OverviewError::BadPatch),
        }
    }
}

async fn decisions(db: &PgPool, ids: &[String]) -> Result<Vec<Decision>, sqlx::Error> {
    sqlx::query_as(
        "SELECT problem_id, owner_id, snoozed_until, handled_at FROM overview_problem_state \
          WHERE problem_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await
}

async fn save_decision(db: &PgPool, id: &str, d: &Decision, by: Uuid) -> Result<(), OverviewError> {
    let saved = sqlx::query(
        "INSERT INTO overview_problem_state \
             (problem_id, owner_id, snoozed_until, handled_at, updated_by, updated_at) \
         VALUES ($1, $2, $3, $4, $5, now()) \
         ON CONFLICT (problem_id) DO UPDATE SET \
             owner_id = EXCLUDED.owner_id, snoozed_until = EXCLUDED.snoozed_until, \
             handled_at = EXCLUDED.handled_at, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(id)
    .bind(d.owner_id)
    .bind(d.snoozed_until)
    .bind(d.handled_at)
    .bind(by)
    .execute(db)
    .await;
    match saved {
        Ok(_) => Ok(()),
        // The only foreign key a caller controls is the owner.
        Err(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation()) =>
        {
            Err(OverviewError::NoSuchPerson)
        }
        Err(e) => Err(e.into()),
    }
}

pub(super) enum OverviewError {
    /// An auth answer to pass through as it is.
    Rejected(Response),
    Db(sqlx::Error),
    Connectors(McpError),
    NoSuchProblem,
    NoSuchPerson,
    BadPatch,
}

impl From<sqlx::Error> for OverviewError {
    fn from(e: sqlx::Error) -> Self {
        OverviewError::Db(e)
    }
}

impl From<McpError> for OverviewError {
    fn from(e: McpError) -> Self {
        OverviewError::Connectors(e)
    }
}

impl IntoResponse for OverviewError {
    fn into_response(self) -> Response {
        match self {
            OverviewError::Rejected(r) => r,
            OverviewError::Db(e) => {
                tracing::error!(%e, "overview: db error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
            OverviewError::Connectors(e) => {
                tracing::error!(%e, "overview: connector read failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
            OverviewError::NoSuchProblem => {
                (StatusCode::NOT_FOUND, "no such problem").into_response()
            }
            OverviewError::NoSuchPerson => {
                (StatusCode::UNPROCESSABLE_ENTITY, "no such person").into_response()
            }
            OverviewError::BadPatch => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "send exactly one of owner_id, snoozed_until or handled",
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_defaults_to_thirty_days_and_rejects_unknown_values() {
        assert_eq!(Range::parse(None), Some(Range::Days30));
        assert_eq!(Range::parse(Some("7d")), Some(Range::Days7));
        assert_eq!(Range::parse(Some("90d")), Some(Range::Days90));
        assert_eq!(Range::parse(Some("24h")), None);
    }

    fn patch(json: &str) -> Result<rules::Change, OverviewError> {
        serde_json::from_str::<ProblemPatch>(json)
            .expect("valid json")
            .into_change()
    }

    #[test]
    fn a_patch_tells_a_cleared_owner_from_an_absent_one() {
        assert!(matches!(
            patch(r#"{"owner_id": null}"#),
            Ok(rules::Change::Owner(None))
        ));
        assert!(matches!(
            patch(r#"{"handled": true}"#),
            Ok(rules::Change::Handled(true))
        ));
        assert!(matches!(
            patch(r#"{"snoozed_until": null}"#),
            Ok(rules::Change::Snooze(None))
        ));
    }

    #[test]
    fn a_patch_with_none_or_two_changes_is_refused() {
        assert!(matches!(patch("{}"), Err(OverviewError::BadPatch)));
        assert!(matches!(
            patch(r#"{"owner_id": null, "handled": true}"#),
            Err(OverviewError::BadPatch)
        ));
    }
}
