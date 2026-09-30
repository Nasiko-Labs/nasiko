//! `/api/me/context-strategy` and `/api/me/pacms-budget` — the two settings a
//! user tunes to control how much of their session history a request carries,
//! and how that slice is chosen.
//!
//! They are one concern and one router: the strategy picks *which* selection
//! algorithm runs, the budget tier picks *how much* it may keep. Both resolve
//! against `nasiko_orchestrator::context_selection`, which owns the read path.
//!
//! Neither route is superuser-gated — a user owns their own preference. A
//! value outside each enum never reaches the handler: Axum's `Json<_>`
//! extractor rejects it during deserialization (HTTP 422).

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use nasiko_orchestrator::{ContextSelectionStrategy, PacmsBudgetLevel};
use serde::{Deserialize, Serialize};

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/me/context-strategy",
            get(get_context_strategy).patch(update_context_strategy),
        )
        .route(
            "/me/pacms-budget",
            get(get_pacms_budget).patch(update_pacms_budget),
        )
}

// ── Context-selection strategy ────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ContextStrategyResponse {
    strategy: ContextSelectionStrategy,
}

#[derive(Debug, Deserialize)]
struct ContextStrategyUpdate {
    strategy: ContextSelectionStrategy,
}

async fn get_context_strategy(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let strategy = ContextSelectionStrategy::for_user(&state.db, user_id).await;
    Json(ContextStrategyResponse { strategy }).into_response()
}

async fn update_context_strategy(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ContextStrategyUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query("UPDATE users SET context_selection_strategy = $1 WHERE id = $2")
        .bind(body.strategy)
        .bind(user_id)
        .execute(&state.db)
        .await;

    match result {
        Ok(_) => Json(ContextStrategyResponse {
            strategy: body.strategy,
        })
        .into_response(),
        Err(e) => {
            tracing::error!(%e, "update_context_strategy: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ── PACMS budget tier ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct PacmsBudgetResponse {
    level: PacmsBudgetLevel,
}

#[derive(Debug, Deserialize)]
struct PacmsBudgetUpdate {
    level: PacmsBudgetLevel,
}

async fn get_pacms_budget(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let level = PacmsBudgetLevel::for_user(&state.db, user_id).await;
    Json(PacmsBudgetResponse { level }).into_response()
}

async fn update_pacms_budget(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<PacmsBudgetUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query("UPDATE users SET pacms_budget_level = $1 WHERE id = $2")
        .bind(body.level)
        .bind(user_id)
        .execute(&state.db)
        .await;

    match result {
        Ok(_) => Json(PacmsBudgetResponse { level: body.level }).into_response(),
        Err(e) => {
            tracing::error!(%e, "update_pacms_budget: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
