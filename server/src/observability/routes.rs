use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json,
    Router,
    extract::{ Path, Query, State },
    http::StatusCode,
    response::{ IntoResponse, Response, sse::{ Event, KeepAlive, Sse } },
    routing::{ get, post },
};
use chrono::{ DateTime, Utc };
use serde::{ Deserialize, Serialize };
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;
use super::handler;
use super::logs::{ LogLine, LogQuery, parse_container_logs, parse_loki_logs, query_proxy_logs };

// ---------------------------------------------------------------------------
// Agent resolution helper
// ---------------------------------------------------------------------------

/// Resolve `agent_ref` (either a UUID string or agent name) into `(id, name)`.
///
/// Called at the start of every observe handler so that the CLI can pass either
/// `nasiko logs my-agent --follow` (name) or `nasiko logs <uuid> --follow` (UUID).
async fn resolve_agent(db: &PgPool, agent_ref: &str) -> Option<(Uuid, String)> {
    if let Ok(id) = agent_ref.parse::<Uuid>() {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM agents WHERE id = $1 AND deleted_at IS NULL"
        )
            .bind(id)
            .fetch_optional(db).await
            .ok()
            .flatten()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM agents WHERE name = $1 AND deleted_at IS NULL"
        )
            .bind(agent_ref)
            .fetch_optional(db).await
            .ok()
            .flatten()
    }
}

// ---------------------------------------------------------------------------
// Public orchestrator factories
// ---------------------------------------------------------------------------

/// Internal health/metrics endpoints — mounted at orchestrator root (no auth required).
pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics)).route("/readiness", get(readiness))
}

/// Protected observability router — mounted under /api/observability (auth required).
///
/// Path params with `{agent_ref}` accept either a UUID or agent name.
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/session/list", get(handler::get_all_sessions))
        .route("/session/{session_id}", get(handler::get_session_details))
        .route("/trace/{trace_id}", get(handler::get_trace_details))
        .route("/span/{trace_id}/{span_id}", get(handler::get_span_details))
        .route("/agent/{agent_id}/stats", get(handler::get_agent_stats))
        .route("/finops/dashboard", get(handler::get_finops_dashboard))
        .route("/finops/insights", post(handler::get_finops_insights))
        .route("/agents/{agent_ref}/logs", get(agent_logs))
        .route("/agents/{agent_ref}/logs/stream", get(agent_logs_stream))
}

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LogParams {
    /// ISO-8601 start time (default: 24 h ago)
    since: Option<String>,
    /// ISO-8601 end time (default: now)
    until: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    /// Filter by level: INFO | WARN | ERROR | DEBUG
    level: Option<String>,
    /// Substring search inside message
    search: Option<String>,
}

fn default_limit() -> usize {
    200
}


// ---------------------------------------------------------------------------
// Observe handlers
// ---------------------------------------------------------------------------

/// `GET /api/observe/agents/{agent_ref}/logs`
///
/// `agent_ref` may be a UUID or an agent name (e.g. `my-agent`).
///
/// Returns a merged, time-sorted list of log lines from up to three sources:
///   1. Loki (when observability backend is configured)
///   2. `proxy_logs` DB table (always)
///   3. Container stdout/stderr via the runtime (always)
async fn agent_logs(
    State(state): State<AppState>,
    Path(agent_ref): Path<String>,
    Query(params): Query<LogParams>
) -> Response {
    let Some((agent_id, agent_name)) = resolve_agent(&state.db, &agent_ref).await else {
        return (StatusCode::NOT_FOUND, format!("Agent '{}' not found", agent_ref)).into_response();
    };

    let since = params.since.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok());
    let until = params.until.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok());

    let q = LogQuery {
        since,
        until,
        limit: params.limit.min(500),
        level: params.level.clone(),
        search: params.search.clone(),
    };

    let mut all_logs: Vec<LogLine> = Vec::new();

    // ── Source 1: proxy_logs DB ──────────────────────────────────────────────
    let proxy = query_proxy_logs(&state.db, agent_id, &q).await;
    all_logs.extend(proxy);

    // ── Source 2: container stdout/stderr ───────────────────────────────────
    let container_id = nasiko_runtime::ContainerId::new(agent_name.clone());
    let tail = q.limit.min(200) as u32;
    if let Ok(raw) = state.runtime.logs(&container_id, tail).await {
        all_logs.extend(parse_container_logs(raw));
    }

    // ── Source 3: Loki (optional) ────────────────────────────────────────────
    if
        let Some(ref obs) = state.observability &&
        let Ok(entries) = obs.query_logs(&agent_name, q.since, q.until, q.limit).await
    {
        all_logs.extend(parse_loki_logs(entries));
    }

    // ── Merge, filter, sort ──────────────────────────────────────────────────
    if let Some(ref level_filter) = params.level {
        let level_upper = level_filter.to_uppercase();
        all_logs.retain(|l| l.level.as_deref() == Some(level_upper.as_str()));
    }
    if let Some(ref search) = params.search {
        let s = search.to_lowercase();
        all_logs.retain(|l| l.message.to_lowercase().contains(&s));
    }

    all_logs.sort_by_key(|l| std::cmp::Reverse(l.timestamp));
    all_logs.truncate(q.limit);

    Json(all_logs).into_response()
}

/// `GET /api/observe/agents/{agent_ref}/logs/stream`
///
/// SSE live-tail stream. `agent_ref` may be a UUID or agent name.
///
/// Protocol:
///   • Immediately emits the last 30 container log lines as historical context.
///   • Then polls every 3 s for new `proxy_logs` rows (since stream opened).
///   • Sends an SSE comment `keepalive` every cycle so the connection stays alive.
async fn agent_logs_stream(
    State(state): State<AppState>,
    Path(agent_ref): Path<String>
) -> Response {
    let Some((agent_id, agent_name)) = resolve_agent(&state.db, &agent_ref).await else {
        return (StatusCode::NOT_FOUND, format!("Agent '{}' not found", agent_ref)).into_response();
    };

    let db = state.db.clone();
    let runtime = state.runtime.clone();

    const MAX_STREAM_SECS: u64 = 3600; // 1-hour max lifetime — prevents connection hoarding

    let stream = async_stream::stream! {
        // ── Step 1: historical container logs ────────────────────────────────
        let container_id = nasiko_runtime::ContainerId::new(agent_name.clone());
        if let Ok(raw) = runtime.logs(&container_id, 30).await {
            for line in parse_container_logs(raw) {
                if let Ok(json) = serde_json::to_string(&line) {
                    yield Ok::<_, Infallible>(Event::default().data(json));
                }
            }
        }

        // ── Step 2: live tail of proxy_logs ──────────────────────────────────
        let mut last_ts = Utc::now();
        let deadline = std::time::Instant::now() + Duration::from_secs(MAX_STREAM_SECS);

        loop {
            if std::time::Instant::now() >= deadline {
                // Signal the client that the stream ended normally (reconnect if needed).
                yield Ok(Event::default().event("close").data("stream timeout — reconnect to continue"));
                break;
            }

            tokio::time::sleep(Duration::from_secs(3)).await;

            let rows: Vec<(DateTime<Utc>, i32, i64, Option<String>)> = sqlx
                ::query_as(
                    r#"SELECT timestamp, status, latency_ms, error
                   FROM proxy_logs
                   WHERE (caller_id = $1 OR target_agent_id = $1)
                     AND timestamp > $2
                   ORDER BY timestamp ASC
                   LIMIT 50"#
                )
                .bind(agent_id)
                .bind(last_ts)
                .fetch_all(&db).await
                .unwrap_or_default();

            for (ts, status, latency_ms, error) in rows {
                last_ts = ts;
                let level = if status >= 500 {
                    "ERROR"
                } else if status >= 400 {
                    "WARN"
                } else {
                    "INFO"
                };
                let message = match error {
                    Some(e) => format!("A2A call → HTTP {status}  ({latency_ms}ms) — {e}"),
                    None => format!("A2A call → HTTP {status}  ({latency_ms}ms)"),
                };
                let line = LogLine {
                    timestamp: ts,
                    level: Some(level.into()),
                    message,
                    trace_id: None,
                    source: "proxy",
                };
                if let Ok(json) = serde_json::to_string(&line) {
                    yield Ok(Event::default().data(json));
                }
            }

            yield Ok(Event::default().comment("keepalive"));
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}


// ---------------------------------------------------------------------------
// Internal health / metrics handlers (mounted at root, no auth)
// ---------------------------------------------------------------------------

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
    let agents_total: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM agents")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let agents_running: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM agents WHERE status = 'running'")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let containers_total = state.runtime
        .list().await
        .map(|c| c.len() as i64)
        .unwrap_or(0);

    let users_total: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let builds_total: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM agent_builds")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let builds_pending: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM agent_builds WHERE status IN ('queued', 'building')")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let chat_sessions_total: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM chat_sessions")
        .fetch_one(&state.db).await
        .unwrap_or(0);

    let token_usage_total: i64 = sqlx
        ::query_scalar("SELECT COUNT(*) FROM token_usage")
        .fetch_one(&state.db).await
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
    let pg_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    let redis_ok = match state.redis.get_multiplexed_async_connection().await {
        Ok(mut conn) => redis::cmd("PING").query_async::<String>(&mut conn).await.is_ok(),
        Err(_) => false,
    };

    let orch_ok = state.runtime.list().await.is_ok();

    let all_ok = pg_ok && orch_ok;
    let status = if all_ok { "ready" } else { "degraded" };
    let http_status = if all_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };

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
