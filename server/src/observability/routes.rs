use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;

use crate::state::AppState;

use super::handler;

/// Public router — mounted at root (no /api prefix, no auth required).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/readiness", get(readiness))
}

/// Protected router — mounted under /api/v1/observability (auth required).
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/session/list", get(handler::get_all_sessions))
        .route("/session/{session_id}", get(handler::get_session_details))
        .route("/trace/{project_id}/{trace_id}", get(handler::get_trace_details))
        .route("/span/{trace_id}/{span_id}", get(handler::get_span_details))
        .route("/agent/{agent_id}/stats", get(handler::get_agent_stats))
        .route("/finops/dashboard", get(handler::get_finops_dashboard))
        .route("/finops/insights", post(handler::get_finops_insights))
}

#[derive(Debug, Serialize)]
struct Metrics {
    agents_total: i64,
    agents_running: i64,
    containers_total: i64,
    users_total: i64,
    builds_total: i64,
    builds_pending: i64,
    chat_sessions_total: i64,
    token_usage_total: i64,
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let agents_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let agents_running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE status = 'running'")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);

    let containers_total = state
        .runtime
        .list()
        .await
        .map(|c| c.len() as i64)
        .unwrap_or(0);

    let users_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let builds_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_builds")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let builds_pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_builds WHERE status IN ('queued', 'building')")
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);

    let chat_sessions_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let token_usage_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM token_usage")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    Json(Metrics {
        agents_total,
        agents_running,
        containers_total,
        users_total,
        builds_total,
        builds_pending,
        chat_sessions_total,
        token_usage_total,
    })
}

#[derive(Debug, Serialize)]
struct ReadinessCheck {
    status: &'static str,
    postgres: bool,
    redis: bool,
    orchestrator: bool,
}

async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let pg_ok = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    let redis_ok = match state.redis.get_multiplexed_async_connection().await {
        Ok(mut conn) => redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .is_ok(),
        Err(_) => false,
    };

    // Simple: try to list deployments as a health check
    let orch_ok = state.runtime.list().await.is_ok();

    let all_ok = pg_ok && orch_ok;
    let status = if all_ok { "ready" } else { "degraded" };
    let http_status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        http_status,
        Json(ReadinessCheck {
            status,
            postgres: pg_ok,
            redis: redis_ok,
            orchestrator: orch_ok,
        }),
    )
}
