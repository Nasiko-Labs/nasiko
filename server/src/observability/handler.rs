use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use nasiko_observability::ObservabilityError;
use serde::Deserialize;
use tracing::instrument;
use crate::auth::Claims;
use crate::state::AppState;

use super::service::{InsightsRequest, ObservabilityService};

// ─── Error mapping ────────────────────────────────────────────────────────────

fn obs_err(e: ObservabilityError) -> Response {
    match e {
        ObservabilityError::NotFound(msg) => {
            (StatusCode::NOT_FOUND, msg).into_response()
        }
        ObservabilityError::Deserialization(msg) => {
            (StatusCode::BAD_GATEWAY, msg).into_response()
        }
        other => {
            (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()).into_response()
        }
    }
}

fn svc(state: &AppState) -> ObservabilityService {
    ObservabilityService::from_state(state)
}



// ─── Request params ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SessionListParams {
    pub start_time: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentStatsParams {
    pub start_time: String,
}

#[derive(Debug, Deserialize)]
pub struct FinopsParams {
    pub start_time: Option<String>,
}


// ─── 1. GET /v1/observability/session/list ────────────────────────────────────
#[instrument(skip(state))]
pub async fn get_all_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<SessionListParams>,
) -> impl IntoResponse {

    let role = claims.role.as_ref().map(|r| format!("{r:?}"));

    tracing::info!(
        input_tokens = 120u64,
        output_tokens = 80u64,
        model = "gpt-4o",
        agent_id = "demo-agent",
        session_id = "demo-session",
        "simulated ai request"
    );

    match svc(&state)
        .get_all_sessions(
            &claims.sub,
            role.as_deref(),
            claims.department_id.as_deref(),
            claims.team_id.as_deref(),
            params.start_time.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}
// ─── 2. GET /v1/observability/session/{session_id} ────────────────────────────

#[instrument(skip(state))]
pub async fn get_session_details(
    State(state): State<AppState>,
    _claims: Claims,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match svc(&state).get_session_details(&session_id).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 3. GET /v1/observability/trace/{project_id}/{trace_id} ──────────────────

#[instrument(skip(state))]
pub async fn get_trace_details(
    State(state): State<AppState>,
    _claims: Claims,
    Path((project_id, trace_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match svc(&state)
        .get_trace_details(&trace_id, &project_id)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 4. GET /v1/observability/span/{trace_id}/{span_id} ──────────────────────

#[instrument(skip(state))]
pub async fn get_span_details(
    State(state): State<AppState>,
    _claims: Claims,
    Path((trace_id, span_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match svc(&state)
        .get_span_details(&trace_id, &span_id)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 5. GET /v1/observability/agent/{agent_id}/stats ─────────────────────────

#[instrument(skip(state))]
pub async fn get_agent_stats(
    State(state): State<AppState>,
    _claims: Claims,
    Path(agent_id): Path<String>,
    Query(params): Query<AgentStatsParams>,
) -> impl IntoResponse {
    match svc(&state)
        .get_agent_stats(&agent_id, &params.start_time)
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 6. GET /v1/observability/finops/dashboard ───────────────────────────────

#[instrument(skip(state))]
pub async fn get_finops_dashboard(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<FinopsParams>,
) -> impl IntoResponse {
    let role = claims.role.as_ref().map(|r| format!("{r:?}"));
    match svc(&state)
        .get_finops_dashboard(
            &claims.sub,
            role.as_deref(),
            claims.department_id.as_deref(),
            claims.team_id.as_deref(),
            params.start_time.as_deref(),
        )
        .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => obs_err(e),
    }
}

// ─── 7. POST /v1/observability/finops/insights ───────────────────────────────

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