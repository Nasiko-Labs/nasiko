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
            // `msg` here is a hand-authored, safe description (e.g. "span 'x' in
            // trace 'y'") — not a raw underlying error — so it's fine to return.
            (StatusCode::NOT_FOUND, msg).into_response()
        }
        ObservabilityError::Deserialization(_) => {
            tracing::error!(error = %e, "observability: failed to deserialize upstream response");
            (StatusCode::BAD_GATEWAY, "observability backend returned an invalid response").into_response()
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



// ─── Request params ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SessionListParams {
    pub start_time: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentStatsParams {
    /// Optional — the service defaults to the last 24 hours, matching the
    /// other observe endpoints (the UI calls this with no params at all).
    pub start_time: Option<String>,
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
            None, // role gating handled by the EE observability provider, not the identity
            None,
            None,
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

// ─── 3. GET /v1/observability/trace/{trace_id} ───────────────────────────────

#[instrument(skip(state))]
pub async fn get_trace_details(
    State(state): State<AppState>,
    _claims: Claims,
    Path(trace_id): Path<String>,
) -> impl IntoResponse {
    match svc(&state).get_trace_details(&trace_id).await {
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
    // OTEL_SERVICE_NAME is set to the agent name (not UUID), so Tempo indexes
    // traces under the agent name. Resolve by name or UUID but always pass
    // the name to the Tempo query.
    let tempo_ref = match super::routes::resolve_agent(&state.db, &agent_id).await {
        Some((_id, name)) => name,
        None => agent_id.clone(),
    };
    match svc(&state)
        .get_agent_stats(&tempo_ref, params.start_time.as_deref().unwrap_or_default())
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
    match svc(&state)
        .get_finops_dashboard(
            &claims.sub,
            None, // role gating handled by the EE observability provider, not the identity
            None,
            None,
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