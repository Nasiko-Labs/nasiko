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

/// Protected orchestrator — mounted under /api/v1/observability (auth required).
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

/// Observe endpoints — mounted under `/api/v1/observability` (auth required via middleware).
///
/// Path params that contain `{agent_ref}` accept either a UUID or an agent name,
/// so both the UI (UUID) and CLI (`nasiko logs my-agent`) work without pre-resolving.
pub fn observe_router() -> Router<AppState> {
    Router::new()
        .route("/observability/agents/{agent_ref}/logs", get(agent_logs))
        .route("/observability/agents/{agent_ref}/logs/stream", get(agent_logs_stream))
        .route("/observability/agents/{agent_ref}/stats", get(agent_stats))
        .route("/observability/traces", get(list_traces))
        .route("/observability/traces/{trace_id}", get(get_trace))
        .route("/observability/finops", get(finops))
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

#[derive(Debug, Deserialize)]
struct StatsParams {
    /// ISO-8601 start time (default: 24 h ago)
    since: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TracesParams {
    /// Filter by agent UUID
    agent_id: Option<Uuid>,
    since: Option<String>,
    #[serde(default = "default_traces_limit")]
    limit: usize,
}

fn default_traces_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
struct FinOpsParams {
    since: Option<String>,
    /// Comma-separated agent UUIDs (empty = all agents visible to caller)
    agent_ids: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AgentStatsResponse {
    agent_id: String,
    period_start: DateTime<Utc>,
    total_requests: u64,
    error_rate: f64,
    avg_latency_ms: f64,
    p50_latency_ms: Option<f64>,
    p95_latency_ms: Option<f64>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    /// "tempo" when backed by the observability provider, "proxy_logs" when using DB fallback.
    source: &'static str,
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

/// `GET /api/observe/agents/{agent_ref}/stats`
///
/// `agent_ref` may be a UUID or agent name.
///
/// Returns performance statistics for the agent:
///   • **Tempo backend**: full stats including token usage, p50/p95 latency.
///   • **Fallback** (proxy_logs DB): request count, error rate, latency percentiles.
///     Token usage will be zero when Tempo is not configured.
async fn agent_stats(
    State(state): State<AppState>,
    Path(agent_ref): Path<String>,
    Query(params): Query<StatsParams>
) -> Response {
    let Some((agent_id, agent_name)) = resolve_agent(&state.db, &agent_ref).await else {
        return (StatusCode::NOT_FOUND, format!("Agent '{}' not found", agent_ref)).into_response();
    };

    let since = params.since
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));

    // ── Tempo path ───────────────────────────────────────────────────────────
    if
        let Some(ref obs) = state.observability &&
        let Ok(stats) = obs.get_agent_stats(&agent_name, since).await
    {
        let resp = AgentStatsResponse {
            agent_id: agent_id.to_string(),
            period_start: stats.period_start,
            total_requests: stats.total_requests,
            error_rate: stats.error_rate,
            avg_latency_ms: stats.avg_latency_ms,
            p50_latency_ms: None,
            p95_latency_ms: None,
            total_input_tokens: stats.total_tokens.input_tokens,
            total_output_tokens: stats.total_tokens.output_tokens,
            source: "tempo",
        };
        return Json(resp).into_response();
    }

    // ── proxy_logs fallback ──────────────────────────────────────────────────
    #[derive(sqlx::FromRow)]
    struct ProxyStats {
        total_requests: i64,
        avg_latency_ms: Option<f64>,
        p50_latency_ms: Option<f64>,
        p95_latency_ms: Option<f64>,
        error_rate: Option<f64>,
    }

    let stats: Option<ProxyStats> = sqlx
        ::query_as(
            r#"SELECT
             COUNT(*)::bigint                                                     AS total_requests,
             AVG(latency_ms::float8)                                             AS avg_latency_ms,
             PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY latency_ms::float8)    AS p50_latency_ms,
             PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY latency_ms::float8)   AS p95_latency_ms,
             SUM(CASE WHEN status >= 400 THEN 1 ELSE 0 END)::float8
               / NULLIF(COUNT(*)::float8, 0)                                     AS error_rate
           FROM proxy_logs
           WHERE target_agent_id = $1 AND timestamp >= $2"#
        )
        .bind(agent_id)
        .bind(since)
        .fetch_optional(&state.db).await
        .ok()
        .flatten();

    let resp = AgentStatsResponse {
        agent_id: agent_id.to_string(),
        period_start: since,
        total_requests: stats
            .as_ref()
            .map(|s| s.total_requests as u64)
            .unwrap_or(0),
        error_rate: stats
            .as_ref()
            .and_then(|s| s.error_rate)
            .unwrap_or(0.0),
        avg_latency_ms: stats
            .as_ref()
            .and_then(|s| s.avg_latency_ms)
            .unwrap_or(0.0),
        p50_latency_ms: stats.as_ref().and_then(|s| s.p50_latency_ms),
        p95_latency_ms: stats.as_ref().and_then(|s| s.p95_latency_ms),
        total_input_tokens: 0,
        total_output_tokens: 0,
        source: "proxy_logs",
    };
    Json(resp).into_response()
}

/// `GET /api/observe/traces?agent_id=<uuid>&since=<iso8601>&limit=<n>`
///
/// Lists recent distributed trace sessions.
/// Returns 503 when no observability backend is configured.
async fn list_traces(
    State(state): State<AppState>,
    Query(params): Query<TracesParams>
) -> Response {
    let Some(ref obs) = state.observability else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Observability backend not configured (set TEMPO_URL + LOKI_URL)",
        ).into_response();
    };

    let since = params.since.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok());

    // Build agent_ids list: either the requested one or all running agents.
    let agent_ids: Vec<String> = if let Some(id) = params.agent_id {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM agents WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        // If a specific agent was requested but doesn't exist, return 404 rather
        // than silently falling back to all agents (would be an information leak).
        match name {
            Some(n) => vec![n],
            None => return (StatusCode::NOT_FOUND, "agent not found").into_response(),
        }
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM agents WHERE status = 'running' AND deleted_at IS NULL LIMIT 20"
        )
            .fetch_all(&state.db).await
            .unwrap_or_default()
    };

    match obs.list_sessions(&agent_ids, since, params.limit).await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/observe/traces/{trace_id}`
///
/// Returns full trace detail with all spans.
///
/// Data sources (tried in order):
///   1. Tempo (when observability backend is configured via TEMPO_URL + LOKI_URL)
///   2. `flows` + `flow_steps` DB tables (always available as fallback)
///
/// This ensures the trace page renders useful data even without a Tempo deployment.
async fn get_trace(State(state): State<AppState>, Path(trace_id): Path<String>) -> Response {
    // ── Try Tempo first ─────────────────────────────────────────────────────
    if let Some(ref obs) = state.observability {
        match obs.get_trace(&trace_id).await {
            Ok(trace) => return Json(trace).into_response(),
            Err(e) => {
                // If Tempo returned a real error (not just "not found"), log and fall through
                // to the DB fallback so the user still sees flow-level data.
                tracing::debug!(trace_id = %trace_id, error = %e, "Tempo get_trace failed, falling back to DB");
            }
        }
    }

    // ── Fallback: build TraceDetails from flows + flow_steps ────────────────
    let flow_row = sqlx::query_as::<_, (String, Option<String>, Option<i64>, chrono::DateTime<Utc>, Option<chrono::DateTime<Utc>>)>(
        r#"SELECT flow_id, root_agent_name, duration_ms, created_at, completed_at
           FROM flows WHERE flow_id = $1"#,
    )
    .bind(&trace_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((flow_id, root_agent, duration_ms, created_at, completed_at)) = flow_row else {
        return (StatusCode::NOT_FOUND, "Trace not found").into_response();
    };

    #[derive(sqlx::FromRow)]
    struct StepRow {
        step_order: i32,
        agent_name: String,
        caller_agent_name: Option<String>,
        input_summary: Option<String>,
        output_summary: Option<String>,
        status: String,
        tokens_used: i32,
        latency_ms: Option<i32>,
        created_at: chrono::DateTime<Utc>,
        completed_at: Option<chrono::DateTime<Utc>>,
    }

    let steps: Vec<StepRow> = sqlx::query_as(
        "SELECT step_order, agent_name, caller_agent_name, input_summary, output_summary, status, tokens_used, latency_ms, created_at, completed_at FROM flow_steps WHERE flow_id = $1 ORDER BY step_order ASC",
    )
    .bind(&flow_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    use nasiko_observability::types::{Span, TraceDetails};
    use std::collections::HashMap;

    // Build a root span representing the orchestrator flow.
    let root_span_id = format!("{:016x}", 1u64);
    let root_service = root_agent.unwrap_or_else(|| "orchestrator".into());

    let mut spans = vec![Span {
        span_id: root_span_id.clone(),
        parent_span_id: None,
        name: format!("flow {}", &flow_id[..12.min(flow_id.len())]),
        started_at: created_at,
        ended_at: completed_at,
        duration_ms: duration_ms.map(|d| d as u64),
        service_name: root_service,
        kind: 2, // server
        status_code: 0,
        status_message: String::new(),
        attributes: HashMap::new(),
    }];

    for step in &steps {
        let span_id = format!("{:016x}", step.step_order as u64 + 1);
        let step_duration = step.latency_ms.map(|ms| ms as u64).or_else(|| {
            step.completed_at.map(|end| (end - step.created_at).num_milliseconds().max(0) as u64)
        });

        let mut attrs: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(ref input) = step.input_summary {
            attrs.insert("input_summary".into(), serde_json::Value::String(input.clone()));
        }
        if let Some(ref output) = step.output_summary {
            attrs.insert("output_summary".into(), serde_json::Value::String(output.clone()));
        }
        if step.tokens_used > 0 {
            attrs.insert("gen_ai.usage.input_tokens".into(), serde_json::json!(step.tokens_used));
        }
        if step.status == "error" || step.status == "failed" {
            attrs.insert("otel.status_code".into(), serde_json::Value::String("ERROR".into()));
        }
        if let Some(ref caller) = step.caller_agent_name {
            attrs.insert("caller".into(), serde_json::Value::String(caller.clone()));
        }

        spans.push(Span {
            span_id,
            parent_span_id: Some(root_span_id.clone()),
            name: step.agent_name.clone(),
            started_at: step.created_at,
            ended_at: step.completed_at,
            duration_ms: step_duration,
            service_name: step.agent_name.clone(),
            kind: 3, // client (calling out to agent)
            status_code: if step.status == "error" || step.status == "failed" { 2 } else { 0 },
            status_message: String::new(),
            attributes: attrs,
        });
    }

    let trace = TraceDetails {
        trace_id: flow_id,
        spans,
        started_at: Some(created_at),
        ended_at: completed_at,
        duration_ms: duration_ms.map(|d| d as u64),
    };

    Json(trace).into_response()
}

/// `GET /api/observe/finops?since=<iso8601>&agent_ids=<uuid,uuid,...>`
///
/// Returns token usage and estimated cost per agent.
/// Returns 503 when no observability backend is configured.
async fn finops(State(state): State<AppState>, Query(params): Query<FinOpsParams>) -> Response {
    let Some(ref obs) = state.observability else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Observability backend not configured (set TEMPO_URL + LOKI_URL)",
        ).into_response();
    };

    let since = params.since.as_deref().and_then(|s| s.parse::<DateTime<Utc>>().ok());

    // Resolve agent UUIDs → names for the Tempo query.
    let agent_ids: Vec<String> = if let Some(ref ids_csv) = params.agent_ids {
        let tokens: Vec<&str> = ids_csv.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        let uuids: Vec<Uuid> = tokens.iter().filter_map(|s| s.parse::<Uuid>().ok()).collect();

        // If the caller supplied ids but ALL of them are malformed, return 400
        // rather than silently falling back to all agents (information disclosure).
        if !tokens.is_empty() && uuids.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                "agent_ids must be comma-separated UUIDs",
            )
                .into_response();
        }

        if uuids.is_empty() {
            // Empty string / no real ids — default to all running agents.
            sqlx::query_scalar::<_, String>(
                "SELECT name FROM agents WHERE status = 'running' AND deleted_at IS NULL LIMIT 50"
            )
                .fetch_all(&state.db).await
                .unwrap_or_default()
        } else {
            // Look up names for the given UUIDs.
            sqlx::query_scalar::<_, String>(
                "SELECT name FROM agents WHERE id = ANY($1) AND deleted_at IS NULL"
            )
                .bind(&uuids)
                .fetch_all(&state.db).await
                .unwrap_or_default()
        }
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM agents WHERE status = 'running' AND deleted_at IS NULL LIMIT 50"
        )
            .fetch_all(&state.db).await
            .unwrap_or_default()
    };

    match obs.get_finops_dashboard(&agent_ids, since).await {
        Ok(dashboard) => Json(dashboard).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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
