use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/usage/summary", get(summary))
        .route("/usage/history", get(history))
        .route("/usage/by-agent", get(by_agent))
        .route("/usage/by-model", get(by_model))
}

// ─── GET /usage/summary ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct SummaryQuery {
    /// Look-back window in days (default: 30).
    #[serde(default = "default_days")]
    days: i64,
}
fn default_days() -> i64 {
    30
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsageSummaryResponse {
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
    period_days: i64,
}

/// Aggregate token usage and cost for the caller over the last `days` days.
#[utoipa::path(
    get,
    path = "/api/usage/summary",
    tag = "usage",
    params(SummaryQuery),
    responses(
        (status = 200, description = "Aggregate usage for the window", body = UsageSummaryResponse),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn summary(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<SummaryQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let from = Utc::now() - Duration::days(q.days);

    let row = sqlx::query_as::<_, SummaryRow>(
        r#"SELECT
            COUNT(*)::bigint as request_count,
            COALESCE(SUM(input_tokens), 0)::bigint as total_input,
            COALESCE(SUM(output_tokens), 0)::bigint as total_output,
            COALESCE(SUM(total_tokens), 0)::bigint as total_tokens,
            COALESCE(SUM(cost_usd)::double precision, 0) as total_cost,
            AVG(latency_ms)::double precision as avg_latency
        FROM token_usage
        WHERE user_id = $1 AND created_at >= $2"#,
    )
    .bind(user_id)
    .bind(from)
    .fetch_one(&state.db)
    .await;

    match row {
        Ok(r) => Json(UsageSummaryResponse {
            request_count: r.request_count.unwrap_or(0),
            total_input_tokens: r.total_input.unwrap_or(0),
            total_output_tokens: r.total_output.unwrap_or(0),
            total_tokens: r.total_tokens.unwrap_or(0),
            total_cost_usd: r.total_cost.unwrap_or(0.0),
            avg_latency_ms: r.avg_latency,
            period_days: q.days,
        })
        .into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "usage summary: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    request_count: Option<i64>,
    total_input: Option<i64>,
    total_output: Option<i64>,
    total_tokens: Option<i64>,
    total_cost: Option<f64>,
    avg_latency: Option<f64>,
}

// ─── GET /usage/history ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct HistoryQuery {
    /// Look-back window in days (default: 30).
    #[serde(default = "default_days")]
    days: i64,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct DailyUsage {
    date: chrono::NaiveDate,
    request_count: i64,
    total_tokens: i64,
    total_cost_usd: f64,
}

/// Per-day usage breakdown for the caller over the last `days` days.
#[utoipa::path(
    get,
    path = "/api/usage/history",
    tag = "usage",
    params(HistoryQuery),
    responses(
        (status = 200, description = "One row per day with activity", body = [DailyUsage]),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn history(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<HistoryQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let from = Utc::now() - Duration::days(q.days);

    let rows = sqlx::query_as::<_, DailyUsage>(
        r#"SELECT
            DATE(created_at) as date,
            COUNT(*)::bigint as request_count,
            COALESCE(SUM(total_tokens), 0)::bigint as total_tokens,
            COALESCE(SUM(cost_usd)::double precision, 0) as total_cost_usd
        FROM token_usage
        WHERE user_id = $1 AND created_at >= $2
        GROUP BY DATE(created_at)
        ORDER BY date ASC"#,
    )
    .bind(user_id)
    .bind(from)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => Json(data).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "usage history: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── GET /usage/by-agent ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct PaginatedQuery {
    /// Page size (default: 50).
    #[serde(default = "default_limit")]
    limit: i64,
    /// Page offset (default: 0).
    #[serde(default)]
    offset: i64,
    /// Substring filter on agent name (`by-agent` only; ignored by `by-model`).
    q: Option<String>,
    /// Look-back window in days (default: 30).
    #[serde(default = "default_days")]
    days: i64,
}
fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct AgentUsage {
    agent_id: Option<Uuid>,
    agent_name: Option<String>,
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
}

/// Doc-only schema for `crate::Paginated<AgentUsage>` (the generic envelope
/// itself is not utoipa-annotated).
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct AgentUsagePage {
    pub data: Vec<AgentUsage>,
    pub total: usize,
}

/// Per-agent usage breakdown for the caller, ordered by total tokens.
#[utoipa::path(
    get,
    path = "/api/usage/by-agent",
    tag = "usage",
    params(PaginatedQuery),
    responses(
        (status = 200, description = "Paginated per-agent usage rows", body = AgentUsagePage),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn by_agent(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<PaginatedQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let from = Utc::now() - Duration::days(q.days);

    let rows = sqlx::query_as::<_, AgentUsage>(
        r#"SELECT
            tu.agent_id,
            a.name as agent_name,
            COUNT(*)::bigint as request_count,
            COALESCE(SUM(tu.input_tokens), 0)::bigint as total_input_tokens,
            COALESCE(SUM(tu.output_tokens), 0)::bigint as total_output_tokens,
            COALESCE(SUM(tu.total_tokens), 0)::bigint as total_tokens,
            COALESCE(SUM(tu.cost_usd)::double precision, 0) as total_cost_usd,
            AVG(tu.latency_ms)::double precision as avg_latency_ms
        FROM token_usage tu
        LEFT JOIN agents a ON a.id = tu.agent_id
        WHERE tu.user_id = $1 AND tu.created_at >= $2
          AND ($3::text IS NULL OR a.name ILIKE '%' || $3 || '%')
        GROUP BY tu.agent_id, a.name
        ORDER BY total_tokens DESC
        LIMIT $4 OFFSET $5"#,
    )
    .bind(user_id)
    .bind(from)
    .bind(&q.q)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => Json(crate::Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "usage by-agent: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── GET /usage/by-model ───────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct ModelUsage {
    provider: String,
    model: String,
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
}

/// Doc-only schema for `crate::Paginated<ModelUsage>` (the generic envelope
/// itself is not utoipa-annotated).
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct ModelUsagePage {
    pub data: Vec<ModelUsage>,
    pub total: usize,
}

/// Per-provider/model usage breakdown for the caller, ordered by total tokens.
#[utoipa::path(
    get,
    path = "/api/usage/by-model",
    tag = "usage",
    params(PaginatedQuery),
    responses(
        (status = 200, description = "Paginated per-model usage rows", body = ModelUsagePage),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn by_model(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<PaginatedQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let from = Utc::now() - Duration::days(q.days);

    let rows = sqlx::query_as::<_, ModelUsage>(
        r#"SELECT
            provider,
            model,
            COUNT(*)::bigint as request_count,
            COALESCE(SUM(input_tokens), 0)::bigint as total_input_tokens,
            COALESCE(SUM(output_tokens), 0)::bigint as total_output_tokens,
            COALESCE(SUM(total_tokens), 0)::bigint as total_tokens,
            COALESCE(SUM(cost_usd)::double precision, 0) as total_cost_usd,
            AVG(latency_ms)::double precision as avg_latency_ms
        FROM token_usage
        WHERE user_id = $1 AND created_at >= $2
        GROUP BY provider, model
        ORDER BY total_tokens DESC
        LIMIT $3 OFFSET $4"#,
    )
    .bind(user_id)
    .bind(from)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => Json(crate::Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "usage by-model: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
