use crate::auth::Claims;
use crate::state::AppState;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use nasiko_observability::ObservabilityError;
use serde::Deserialize;
use tracing::instrument;
use utoipa::IntoParams;

use super::service::{
    EnsureSessionOutcome, InsightsRequest, ObservabilityService, parse_iso_or_default,
};

/// Request extension injected by EE middleware to scope FinOps queries to a
/// set of user UUIDs (org-unit filter). OSS handlers check for this extension
/// and pass it through to the service layer; if absent, no user filtering.
#[derive(Clone, Debug)]
pub struct FinopsUserScope(pub Vec<uuid::Uuid>);

// ─── Error mapping ────────────────────────────────────────────────────────────

fn obs_err(e: ObservabilityError) -> Response {
    match e {
        ObservabilityError::NotFound(msg) => {
            // `msg` here is a hand-authored, safe description (e.g. "span 'x' in
            // trace 'y'") — not a raw underlying error — so it's fine to return.
            (StatusCode::NOT_FOUND, msg).into_response()
        }
        ObservabilityError::BadRequest(msg) => {
            // `msg` is a hand-authored validation message (e.g. an invalid
            // query param) — safe to return so the caller can correct it.
            (StatusCode::BAD_REQUEST, msg).into_response()
        }
        ObservabilityError::Deserialization(_) => {
            tracing::error!(error = %e, "observability: failed to deserialize upstream response");
            (
                StatusCode::BAD_GATEWAY,
                "observability backend returned an invalid response",
            )
                .into_response()
        }
        other => {
            // Catches `Internal` and any future variants — these wrap raw
            // Tempo/Loki client/HTTP errors that must not reach the client.
            tracing::error!(error = %other, "observability request failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

fn svc(state: &AppState) -> ObservabilityService {
    ObservabilityService::from_state(state)
}

/// `get_finops_dashboard` degrades to a zeroed response when there's nothing
/// to query, but `get_agent_stats`/`get_session_details` ask about one
/// specific entity — there's no honest "zero" to fabricate, so surface a
/// clear, actionable status instead of letting the provider's connection
/// failure reach the client as an opaque `internal error`.
fn observability_unconfigured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "observability backend not configured (set TEMPO_URL and LOKI_URL)",
    )
        .into_response()
}

// ─── Request params ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, IntoParams)]
pub struct SessionListParams {
    /// ISO-8601 window start (default: 7 days ago).
    pub start_time: Option<String>,
    /// Page size (default 25, max 100). Each row costs one trace-store lookup,
    /// so this bounds the request's real work — it is not just a display limit.
    pub limit: Option<i64>,
    /// Rows to skip, for offset paging (default 0).
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AgentStatsParams {
    /// Optional — the service defaults to the last 24 hours, matching the
    /// other observe endpoints (the UI calls this with no params at all).
    pub start_time: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FinopsFilterParams {
    /// ISO-8601 window start (default: 30 days ago). Ignored when `range` is set.
    pub start_time: Option<String>,
    /// ISO-8601 window end (default: now). Without it a past-month selection
    /// means "that month through today" rather than that month.
    pub end_time: Option<String>,
    /// "24h" | "7d" | "30d" — quick-select range, overrides `start_time`.
    pub range: Option<String>,
    /// Agent UUID or name — scopes the whole response to one agent.
    pub agent_id: Option<String>,
    /// Exact model id, matched against span model attributes.
    pub model: Option<String>,
    /// Provider name filter (e.g. "openai", "anthropic"), matched against
    /// the `provider` column in `trace_usage` (derived from `model_pricing`).
    pub provider: Option<String>,
    /// Org-unit filter — resolved to user_ids by the EE auth layer. OSS
    /// accepts the param but ignores it (no org hierarchy). EE reads it in
    /// the handler and passes user_ids to the service.
    #[allow(dead_code)]
    pub org_unit: Option<String>,
    /// "agent" | "workflow" — which attribution source powers the response's
    /// `attributions` field (default "agent").
    pub view: Option<String>,
    /// When `true`, restricts results to agents owned by the caller.
    /// Ignored when `agent_id` is also set (already scoped to one agent).
    #[serde(default)]
    pub my_agent: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FinopsDayDrilldownParams {
    /// Calendar day, "YYYY-MM-DD", interpreted in UTC.
    pub date: String,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FinopsSpendCalendarParams {
    /// "YYYY-MM", interpreted in UTC.
    pub month: String,
    pub range: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FinopsAttributionsParams {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub range: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub view: Option<String>,
    /// "cost" | "tokens" | "operations" | "avg_latency" | "container_hours" | "name"
    pub sort_by: Option<String>,
    /// "asc" | "desc" (default "desc")
    pub sort_dir: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Validates `range` is one of "24h"/"7d"/"30d" when present.
// `Response` as the Err type is clippy::result_large_err-flagged, but this
// runs at most once per request (an early-return 400 path), not a hot loop —
// boxing it would just move the allocation, not remove it.
#[allow(clippy::result_large_err)]
fn validate_range(range: Option<&str>) -> Result<(), Response> {
    match range {
        None | Some("24h") | Some("7d") | Some("30d") => Ok(()),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid range '{other}' — expected 24h, 7d, or 30d"),
        )
            .into_response()),
    }
}

/// Validates `view` is one of "agent"/"workflow" when present, returning the
/// resolved value (default "agent").
#[allow(clippy::result_large_err)] // see validate_range
fn validate_view(view: Option<&str>) -> Result<&str, Response> {
    match view {
        None | Some("agent") => Ok("agent"),
        Some("workflow") => Ok("workflow"),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!("invalid view '{other}' — expected agent or workflow"),
        )
            .into_response()),
    }
}

/// Resolves an optional agent UUID-or-name filter into the Tempo-queryable
/// agent name. Returns `Ok(None)` when no filter was given, `Err` (400) when
/// one was given but didn't resolve — a silently-dropped filter would show
/// unfiltered data under a caller-set "Agent: X" label, which is worse than
/// an explicit error.
async fn resolve_agent_filter(
    db: &sqlx::PgPool,
    agent_id: Option<&str>,
) -> Result<Option<String>, Response> {
    match agent_id {
        None => Ok(None),
        Some(id) => match super::routes::resolve_agent(db, id).await {
            Some((_, name)) => Ok(Some(name)),
            None => {
                Err((StatusCode::BAD_REQUEST, format!("agent '{id}' not found")).into_response())
            }
        },
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AgentHoursParams {
    /// ISO-8601 window start (default: all-time; 30 days ago when `bucket` is set).
    pub start_time: Option<String>,
    /// ISO-8601 window end (default: now).
    pub end_time: Option<String>,
    /// Optional agent UUID — restricts the report to one agent.
    pub agent_id: Option<String>,
    /// Optional series granularity: "hour" | "day". Anything else is ignored.
    pub bucket: Option<String>,
}

// ─── 1. GET /v1/observability/session/list ────────────────────────────────────

/// List chat sessions (DB-authoritative, enriched from Tempo when available).
#[utoipa::path(
    get,
    path = "/api/observability/session/list",
    tag = "observability",
    params(SessionListParams),
    responses(
        (status = 200, description = "Sessions in the window", body = crate::observability::service::SessionListResponse),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_all_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<SessionListParams>,
) -> impl IntoResponse {
    match svc(&state)
        .get_all_sessions(
            &claims.sub,
            None, // role gating handled by the EE observability provider, not the identity
            None,
            None,
            params.start_time.as_deref(),
            claims.is_superuser,
            params.limit,
            params.offset,
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 1b. POST /v1/observability/session/ensure ──────────────────────────────

/// Request body for the ensure-session endpoint.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EnsureSessionRequest {
    /// The coding agent's session id (e.g. a Claude Code session UUID).
    pub session_id: String,
    /// The agent name as registered in the `agents` table (e.g. "claude-code").
    pub agent_name: String,
}

/// Ensure a `chat_sessions` row exists for an external coding agent session.
///
/// Called by the CLI after the first OTLP export for a session. Idempotent —
/// returns 200 whether the row was just created or already existed.
#[utoipa::path(
    post,
    path = "/api/observability/session/ensure",
    tag = "observability",
    request_body = EnsureSessionRequest,
    responses(
        (status = 200, description = "Session ensured (created or already existed)"),
        (status = 404, description = "Owned agent not found"),
        (status = 409, description = "Session already belongs to another user or agent"),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
#[instrument(skip(state))]
pub async fn ensure_session(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<EnsureSessionRequest>,
) -> impl IntoResponse {
    match svc(&state)
        .ensure_session(&body.session_id, &body.agent_name, &claims.sub)
        .await
    {
        Ok(EnsureSessionOutcome::Created) => StatusCode::CREATED.into_response(),
        Ok(EnsureSessionOutcome::Existing) => StatusCode::OK.into_response(),
        Ok(EnsureSessionOutcome::Conflict) => (
            StatusCode::CONFLICT,
            "session belongs to another user or agent",
        )
            .into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 2. GET /v1/observability/session/{session_id} ────────────────────────────

/// Detail for one session: traces, token usage, and cost summary.
#[utoipa::path(
    get,
    path = "/api/observability/session/{session_id}",
    tag = "observability",
    params(
        ("session_id" = String, Path, description = "A2A context/session ID"),
    ),
    responses(
        (status = 200, description = "Session detail", body = crate::observability::service::SessionDetailResponse),
        (status = 404, description = "Session not found in the observability backend"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_session_details(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = svc(&state)
        .authorize_session_access(&session_id, &claims.sub, claims.is_superuser)
        .await
    {
        return obs_err(error);
    }
    if !state.config.observability_enabled {
        return observability_unconfigured();
    }
    match svc(&state)
        .get_session_details(&session_id, &claims.sub, claims.is_superuser)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 3. GET /v1/observability/trace/{trace_id} ───────────────────────────────

/// Detail for one trace: full span tree with per-span token/cost attribution.
#[utoipa::path(
    get,
    path = "/api/observability/trace/{trace_id}",
    tag = "observability",
    params(
        ("trace_id" = String, Path, description = "W3C trace ID (hex)"),
    ),
    responses(
        (status = 200, description = "Trace detail with span tree", body = crate::observability::service::TraceDetailResponse),
        (status = 404, description = "Trace not found"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_trace_details(
    State(state): State<AppState>,
    claims: Claims,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    match svc(&state)
        .get_trace_details(&trace_id, &claims.sub, claims.is_superuser)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 4. GET /v1/observability/span/{trace_id}/{span_id} ──────────────────────

/// Detail for one span: attributes, input/output content, and cost.
#[utoipa::path(
    get,
    path = "/api/observability/span/{trace_id}/{span_id}",
    tag = "observability",
    params(
        ("trace_id" = String, Path, description = "W3C trace ID (hex)"),
        ("span_id" = String, Path, description = "Span ID (hex)"),
    ),
    responses(
        (status = 200, description = "Span detail", body = crate::observability::service::SpanDetailResponse),
        (status = 404, description = "Span not found in this trace"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_span_details(
    State(state): State<AppState>,
    claims: Claims,
    Path((trace_id, span_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match svc(&state)
        .get_span_details(&trace_id, &span_id, &claims.sub, claims.is_superuser)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 5. GET /v1/observability/agent/{agent_id}/stats ─────────────────────────

/// Trace/cost/latency stats for one agent (accepts a UUID or agent name).
#[utoipa::path(
    get,
    path = "/api/observability/agent/{agent_id}/stats",
    tag = "observability",
    params(
        ("agent_id" = String, Path, description = "Agent UUID or name"),
        AgentStatsParams,
    ),
    responses(
        (status = 200, description = "Agent stats", body = crate::observability::service::AgentStatsResponse),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_agent_stats(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<String>,
    Query(params): Query<AgentStatsParams>,
) -> impl IntoResponse {
    let Some((_resolved_id, tempo_ref)) =
        super::routes::resolve_accessible_agent(&state, &claims, &agent_id).await
    else {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    };
    if !super::routes::agent_name_fully_accessible(&state, &claims, &tempo_ref).await {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    }
    if !state.config.observability_enabled {
        return observability_unconfigured();
    }
    // Tempo's service.name is the agent name (the injector sets
    // OTEL_SERVICE_NAME to the container/agent name); accept a name or UUID
    // here (same contract as the logs endpoints) and query by name.
    match svc(&state)
        .get_agent_stats(&tempo_ref, params.start_time.as_deref())
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 6. GET /v1/observability/finops/dashboard ───────────────────────────────

/// FinOps dashboard: per-agent cost/token rows plus fleet-wide summary.
#[utoipa::path(
    get,
    path = "/api/observability/finops/dashboard",
    tag = "observability",
    params(FinopsFilterParams),
    responses(
        (status = 200, description = "FinOps dashboard data", body = crate::observability::service::FinopsDashboardResponse),
        (status = 400, description = "Malformed filter (range/view/agent_id)"),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
#[instrument(skip(state, user_scope))]
pub async fn get_finops_dashboard(
    State(state): State<AppState>,
    claims: Claims,
    user_scope: Option<Extension<FinopsUserScope>>,
    Query(params): Query<FinopsFilterParams>,
) -> Response {
    if let Err(r) = validate_range(params.range.as_deref()) {
        return r;
    }
    let view = match validate_view(params.view.as_deref()) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let agent_name = match resolve_agent_filter(&state.db, params.agent_id.as_deref()).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    if let Some(name) = agent_name.as_deref()
        && !super::routes::agent_name_fully_accessible(&state, &claims, name).await
    {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    }

    let (start_time, end_time) =
        match resolve_range_params(&params.start_time, &params.end_time, &params.range) {
            Ok(v) => v,
            Err(r) => return r,
        };

    let user_ids = user_scope.map(|Extension(s)| s.0);
    let accessible_agent_ids = accessible_agent_ids(&state, &claims).await;
    let owner_id = (params.my_agent && agent_name.is_none()).then_some(claims.sub.as_str());
    match svc(&state)
        .get_finops_dashboard(
            &claims.sub,
            None,
            None,
            None,
            start_time.as_deref(),
            end_time.as_deref(),
            agent_name.as_deref(),
            params.model.as_deref(),
            params.provider.as_deref(),
            user_ids.as_deref(),
            accessible_agent_ids.as_deref(),
            owner_id,
            view,
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

/// "range" (24h/7d/30d) overrides `start_time`/`end_time` when present,
/// computed relative to `end_time` (or now). Returns the resolved
/// `(start_time, end_time)` ISO strings the service layer already knows how
/// to parse.
#[allow(clippy::result_large_err)] // see validate_range
fn resolve_range_params(
    start_time: &Option<String>,
    end_time: &Option<String>,
    range: &Option<String>,
) -> Result<(Option<String>, Option<String>), Response> {
    let Some(r) = range.as_deref() else {
        return Ok((start_time.clone(), end_time.clone()));
    };
    let hours = match r {
        "24h" => 24,
        "7d" => 24 * 7,
        "30d" => 24 * 30,
        other => {
            return Err(
                (StatusCode::BAD_REQUEST, format!("invalid range '{other}'")).into_response(),
            );
        }
    };
    let end = end_time
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let start = end - chrono::Duration::hours(hours);
    Ok((
        Some(start.to_rfc3339()),
        Some(end_time.clone().unwrap_or_else(|| end.to_rfc3339())),
    ))
}

// ─── 6b. GET /v1/observability/finops/spend-timeseries ───────────────────────

/// Spend-over-time series, dollar-only, honoring the full requested range
/// (not silently clamped to Tempo's 168h single-search limit).
#[utoipa::path(
    get,
    path = "/api/observability/finops/spend-timeseries",
    tag = "observability",
    params(FinopsFilterParams),
    responses(
        (status = 200, description = "Spend time series", body = crate::observability::service::FinopsSpendTimeseriesResponse),
        (status = 400, description = "Malformed filter"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_finops_spend_timeseries(
    State(state): State<AppState>,
    _claims: Claims,
    Query(params): Query<FinopsFilterParams>,
) -> Response {
    if let Err(r) = validate_range(params.range.as_deref()) {
        return r;
    }
    let agent_name = match resolve_agent_filter(&state.db, params.agent_id.as_deref()).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    match svc(&state)
        .get_finops_spend_timeseries(
            params.start_time.as_deref(),
            params.end_time.as_deref(),
            params.range.as_deref(),
            agent_name.as_deref(),
            params.model.as_deref(),
            params.provider.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 6c. GET /v1/observability/finops/spend-calendar ─────────────────────────

/// Day-of-month spend heatmap for the given month.
#[utoipa::path(
    get,
    path = "/api/observability/finops/spend-calendar",
    tag = "observability",
    params(FinopsSpendCalendarParams),
    responses(
        (status = 200, description = "Spend calendar", body = crate::observability::service::FinopsSpendCalendarResponse),
        (status = 400, description = "Malformed month/filter"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_finops_spend_calendar(
    State(state): State<AppState>,
    _claims: Claims,
    Query(params): Query<FinopsSpendCalendarParams>,
) -> Response {
    if let Err(r) = validate_range(params.range.as_deref()) {
        return r;
    }
    let agent_name = match resolve_agent_filter(&state.db, params.agent_id.as_deref()).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    match svc(&state)
        .get_finops_spend_calendar(
            &params.month,
            params.range.as_deref(),
            agent_name.as_deref(),
            params.model.as_deref(),
            params.provider.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 6d. GET /v1/observability/finops/spend-calendar/day ─────────────────────

/// Hourly spend drill-down for one calendar day.
#[utoipa::path(
    get,
    path = "/api/observability/finops/spend-calendar/day",
    tag = "observability",
    params(FinopsDayDrilldownParams),
    responses(
        (status = 200, description = "Hourly spend for one day", body = crate::observability::service::FinopsDayDrilldownResponse),
        (status = 400, description = "Malformed date/filter"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_finops_spend_calendar_day(
    State(state): State<AppState>,
    _claims: Claims,
    Query(params): Query<FinopsDayDrilldownParams>,
) -> Response {
    let agent_name = match resolve_agent_filter(&state.db, params.agent_id.as_deref()).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    match svc(&state)
        .get_finops_spend_calendar_day(
            &params.date,
            agent_name.as_deref(),
            params.model.as_deref(),
            params.provider.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 6e. GET /v1/observability/finops/attributions ────────────────────────────

/// Agent/Workflow attribution table, server-side sorted and paginated —
/// separate from `/dashboard` so a sort/page click doesn't re-run the
/// KPI/timeseries work.
#[utoipa::path(
    get,
    path = "/api/observability/finops/attributions",
    tag = "observability",
    params(FinopsAttributionsParams),
    responses(
        (status = 200, description = "Attribution rows", body = crate::observability::service::FinopsAttributionsResponse),
        (status = 400, description = "Malformed filter"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_finops_attributions(
    State(state): State<AppState>,
    _claims: Claims,
    Query(params): Query<FinopsAttributionsParams>,
) -> Response {
    if let Err(r) = validate_range(params.range.as_deref()) {
        return r;
    }
    let view = match validate_view(params.view.as_deref()) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let agent_name = match resolve_agent_filter(&state.db, params.agent_id.as_deref()).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    let (start_time, end_time) =
        match resolve_range_params(&params.start_time, &params.end_time, &params.range) {
            Ok(v) => v,
            Err(r) => return r,
        };
    match svc(&state)
        .get_finops_attributions(
            start_time.as_deref(),
            end_time.as_deref(),
            agent_name.as_deref(),
            params.model.as_deref(),
            params.provider.as_deref(),
            view,
            params.sort_by.as_deref(),
            params.sort_dir.as_deref(),
            params.limit,
            params.offset,
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 7. POST /v1/observability/finops/insights ───────────────────────────────

/// LLM-generated cost insights from the caller-supplied FinOps KPI snapshot.
#[utoipa::path(
    post,
    path = "/api/observability/finops/insights",
    tag = "observability",
    request_body = crate::observability::service::InsightsRequest,
    responses(
        (status = 200, description = "Up to 3 insight bullet points", body = crate::observability::service::InsightsResponseEnvelope),
        (status = 500, description = "LLM call failed"),
    ),
)]
#[instrument(skip(state, body))]
pub async fn get_finops_insights(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<InsightsRequest>,
) -> impl IntoResponse {
    match svc(&state).get_finops_insights(&body).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 8. GET /v1/observability/finops/agent-hours ─────────────────────────────

/// Windowed replica-hours per agent (billing source of truth), optionally bucketed.
#[utoipa::path(
    get,
    path = "/api/observability/finops/agent-hours",
    tag = "observability",
    params(AgentHoursParams),
    responses(
        (status = 200, description = "Replica-hours report", body = crate::observability::service::AgentHoursResponse),
        (status = 400, description = "Malformed start_time/end_time/agent_id"),
    ),
)]
#[instrument(skip(state))]
pub async fn get_agent_hours(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<AgentHoursParams>,
) -> impl IntoResponse {
    if !claims.is_superuser
        && let Some(Ok(agent_id)) = params.agent_id.as_deref().map(str::parse)
        && !crate::acl::can_access_agent(&state, &claims, agent_id).await
    {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    }
    let accessible_agent_ids = accessible_agent_ids(&state, &claims).await;
    match svc(&state)
        .get_agent_hours(
            params.start_time.as_deref(),
            params.end_time.as_deref(),
            params.agent_id.as_deref(),
            params.bucket.as_deref(),
            accessible_agent_ids.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

async fn accessible_agent_ids(state: &AppState, claims: &Claims) -> Option<Vec<uuid::Uuid>> {
    if claims.is_superuser {
        return None;
    }

    let agent_ids: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM agents WHERE deleted_at IS NULL ORDER BY id")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    let mut accessible = Vec::new();
    for agent_id in agent_ids {
        if crate::acl::can_access_agent(state, claims, agent_id).await {
            accessible.push(agent_id);
        }
    }
    Some(accessible)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_range ───────────────────────────────────────────────────────

    #[test]
    fn validate_range_accepts_none_and_the_three_known_values() {
        assert!(validate_range(None).is_ok());
        assert!(validate_range(Some("24h")).is_ok());
        assert!(validate_range(Some("7d")).is_ok());
        assert!(validate_range(Some("30d")).is_ok());
    }

    #[test]
    fn validate_range_rejects_anything_else_with_400() {
        for bad in ["1h", "7D", "", "30days", "24H"] {
            let err = validate_range(Some(bad)).expect_err(&format!("{bad:?} must be rejected"));
            assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        }
    }

    // ── validate_view ────────────────────────────────────────────────────────

    #[test]
    fn validate_view_defaults_to_agent() {
        assert_eq!(validate_view(None).unwrap(), "agent");
        assert_eq!(validate_view(Some("agent")).unwrap(), "agent");
    }

    #[test]
    fn validate_view_accepts_workflow() {
        assert_eq!(validate_view(Some("workflow")).unwrap(), "workflow");
    }

    #[test]
    fn validate_view_rejects_unknown_values_with_400() {
        for bad in ["Agent", "workflows", "team", ""] {
            let err = validate_view(Some(bad)).expect_err(&format!("{bad:?} must be rejected"));
            assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        }
    }

    // ── resolve_range_params ─────────────────────────────────────────────────

    #[test]
    fn resolve_range_params_passthrough_when_no_range() {
        let start = Some("2024-01-01T00:00:00Z".to_string());
        let end = Some("2024-01-31T00:00:00Z".to_string());
        let (rs, re) = resolve_range_params(&start, &end, &None).unwrap();
        assert_eq!(rs, start);
        assert_eq!(re, end);
    }

    #[test]
    fn resolve_range_params_24h_overrides_start_time_and_keeps_a_given_end_time() {
        let stale_start = Some("2000-01-01T00:00:00Z".to_string());
        let fixed_end = Some("2024-06-15T12:00:00Z".to_string());
        let (rs, re) =
            resolve_range_params(&stale_start, &fixed_end, &Some("24h".to_string())).unwrap();
        assert_eq!(
            re, fixed_end,
            "an explicit end_time must be preserved verbatim"
        );
        let start_parsed = DateTime::parse_from_rfc3339(rs.as_deref().unwrap()).unwrap();
        let end_parsed = DateTime::parse_from_rfc3339(re.as_deref().unwrap()).unwrap();
        assert_eq!(
            (end_parsed - start_parsed).num_hours(),
            24,
            "resolved start must be exactly 24h before the given end_time"
        );
    }

    #[test]
    fn resolve_range_params_30d_with_no_end_time_anchors_to_now() {
        let before = Utc::now();
        let (rs, re) = resolve_range_params(&None, &None, &Some("30d".to_string())).unwrap();
        let start_parsed = DateTime::parse_from_rfc3339(rs.as_deref().unwrap()).unwrap();
        let end_parsed = DateTime::parse_from_rfc3339(re.as_deref().unwrap()).unwrap();
        assert!(
            end_parsed.with_timezone(&Utc) >= before,
            "end_time must default to (roughly) now"
        );
        assert_eq!((end_parsed - start_parsed).num_hours(), 720);
    }

    #[test]
    fn resolve_range_params_rejects_an_invalid_range_with_400() {
        let err = resolve_range_params(&None, &None, &Some("bogus".to_string())).unwrap_err();
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }
}

// ── finops/savings ────────────────────────────────────────────────────────────

/// Query params for `GET /finops/savings`.
///
/// The window and dimension filters are deliberately the same ones `/finops/dashboard` takes, so a
/// client can carry its filter state across without translating it, and so the savings aggregate
/// answers the same question as the spend aggregate beside it.
#[derive(Debug, Deserialize)]
pub struct SavingsParams {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub range: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// "total" (default) | "agent" | "session".
    pub scope: Option<String>,
    /// One session's savings; implies `scope=session`.
    pub session_id: Option<String>,
    /// Row cap for the agent and session scopes.
    pub limit: Option<i64>,
}

/// Rows returned for the agent and session scopes. Bounded so a wide window cannot return the whole
/// estate in one response.
const SAVINGS_DEFAULT_LIMIT: i64 = 25;
const SAVINGS_MAX_LIMIT: i64 = 200;

pub async fn get_finops_savings(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<SavingsParams>,
) -> Response {
    if let Err(r) = validate_range(params.range.as_deref()) {
        return r;
    }
    let scope = match super::savings::Scope::parse(params.scope.as_deref()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // `session_id` names one session, which only means anything in the session rollup.
    let scope = if params.session_id.is_some() {
        super::savings::Scope::Session
    } else {
        scope
    };

    let (start_time, end_time) =
        match resolve_range_params(&params.start_time, &params.end_time, &params.range) {
            Ok(v) => v,
            Err(r) => return r,
        };
    let start = parse_iso_or_default(start_time.as_deref(), 30);
    let end = end_time
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    // The dashboard accepts a name or a UUID here; the ledger is UUID-keyed, so resolve before use.
    let agent_uuid = match resolve_agent_uuid(&state.db, params.agent_id.as_deref()).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Some(id) = agent_uuid
        && !crate::acl::can_access_agent(&state, &claims, id).await
    {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    }

    let accessible = accessible_agent_ids(&state, &claims).await;
    let query = super::savings::SavingsQuery {
        start,
        end,
        scope,
        agent_id: agent_uuid,
        provider: params.provider.as_deref(),
        model: params.model.as_deref(),
        session_id: params.session_id.as_deref(),
        limit: params
            .limit
            .unwrap_or(SAVINGS_DEFAULT_LIMIT)
            .clamp(1, SAVINGS_MAX_LIMIT),
        accessible_agent_ids: accessible.as_deref(),
    };

    match super::savings::get_savings(&state.db, &query).await {
        Ok(data) => Json(super::savings::SavingsResponse {
            data,
            status_code: 200,
            message: "ok".into(),
        })
        .into_response(),
        Err(e) => {
            tracing::error!(%e, "finops savings query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

/// Resolve an `agent_id` param that may be a UUID or a name to the UUID the ledger is keyed on.
#[allow(clippy::result_large_err)]
async fn resolve_agent_uuid(
    db: &sqlx::PgPool,
    raw: Option<&str>,
) -> Result<Option<uuid::Uuid>, Response> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Ok(id) = uuid::Uuid::parse_str(raw) {
        return Ok(Some(id));
    }
    let found: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM agents WHERE name = $1 AND deleted_at IS NULL")
            .bind(raw)
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    match found {
        Some(id) => Ok(Some(id)),
        None => Err((StatusCode::NOT_FOUND, "agent not found").into_response()),
    }
}
