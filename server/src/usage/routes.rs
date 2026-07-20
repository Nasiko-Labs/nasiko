use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Deserialize)]
struct SummaryQuery {
    #[serde(default = "default_days")]
    days: i64,
}
fn default_days() -> i64 {
    30
}

#[derive(Debug, Serialize)]
struct UsageSummaryResponse {
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
    period_days: i64,
}

async fn summary(
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

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_days")]
    days: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DailyUsage {
    date: chrono::NaiveDate,
    request_count: i64,
    total_tokens: i64,
    total_cost_usd: f64,
}

async fn history(
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

#[derive(Debug, Deserialize)]
struct PaginatedQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    q: Option<String>,
    #[serde(default = "default_days")]
    days: i64,
}
fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AgentUsage {
    agent_id: Option<Uuid>,
    agent_name: Option<String>,
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
}

async fn by_agent(
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

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ModelUsage {
    provider: String,
    model: String,
    request_count: i64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    avg_latency_ms: Option<f64>,
}

async fn by_model(
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
