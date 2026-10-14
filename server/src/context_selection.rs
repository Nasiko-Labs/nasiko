//! `/api/me/*` — the preferences that decide how much of a user's chat history each request
//! carries, and how that slice is chosen.
//!
//! One concern, one router: whether to select at all (`enabled`), which algorithm selects
//! (`strategy`), and how much it may keep (`level`). All three resolve against
//! `nasiko_orchestrator::context_selection`, which owns the read path.
//!
//! None of these routes is superuser-gated — a user owns their own preference. A value outside an
//! enum never reaches a handler: Axum's `Json<_>` extractor rejects it during deserialization
//! (HTTP 422).
//!
//! Responses here are **bare JSON**, not the `{data}` envelope. That is what the settings page
//! parses today, and these routes predate the envelope convention.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use nasiko_orchestrator::{ContextSelectionStrategy, ContextTiers, PacmsBudgetLevel};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

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
        .route("/me/context", axum::routing::patch(update_context))
        .route("/me/context-preview", post(context_preview))
        .route("/me/settings-history", get(get_settings_history))
}

// ── Change log (CX-H) ─────────────────────────────────────────────────────────

/// Record that a preference actually moved.
///
/// Called only when the new value differs from the stored one. A PATCH that re-sends the current
/// value is a no-op, and the savings chart must not draw a "you changed this" mark for it.
///
/// Best-effort: the preference is saved either way. A lost history row costs an annotation, and
/// failing the write would cost the user their setting.
async fn record_change(db: &PgPool, user_id: Uuid, field: &str, from: &str, to: &str) {
    let result = sqlx::query(
        "INSERT INTO user_setting_changes (user_id, field, from_value, to_value) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(field)
    .bind(from)
    .bind(to)
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, field, "user_setting_changes write failed (swallowed)");
    }
}

#[derive(Debug, Serialize)]
struct SettingsChange {
    at: DateTime<Utc>,
    field: String,
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    start_time: Option<String>,
    end_time: Option<String>,
    range: Option<String>,
}

/// `GET /api/me/settings-history` — when the caller's own preferences changed.
///
/// A bare JSON array, newest first. The chart reads it to mark the day a user switched strategy, so
/// a jump in the trend has a cause beside it.
#[utoipa::path(
    get,
    path = "/api/me/settings-history",
    tag = "context-selection",
    params(
        ("start_time" = Option<String>, Query, description = "Window start, RFC 3339"),
        ("end_time" = Option<String>, Query, description = "Window end, RFC 3339 (default now)"),
        ("range" = Option<String>, Query, description = "Preset window: 24h, 7d, 30d, 90d (default 30d when no start_time)"),
    ),
    responses((status = 200, description = "Bare JSON array of changes, newest first")),
    security(("bearer_auth" = []))
)]
pub(crate) async fn get_settings_history(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<HistoryParams>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let days = match params.range.as_deref() {
        Some("24h") => 1,
        Some("7d") => 7,
        Some("90d") => 90,
        // Includes `30d` and the no-range case: 30 days is the page's own default window.
        _ => 30,
    };
    let start = parse_rfc3339(params.start_time.as_deref())
        .unwrap_or_else(|| Utc::now() - Duration::days(days));
    let end = parse_rfc3339(params.end_time.as_deref()).unwrap_or_else(Utc::now);

    let rows: Vec<(DateTime<Utc>, String, String, String)> = sqlx::query_as(
        "SELECT changed_at, field, from_value, to_value FROM user_setting_changes \
          WHERE user_id = $1 AND changed_at >= $2 AND changed_at < $3 \
          ORDER BY changed_at DESC",
    )
    .bind(user_id)
    .bind(start)
    .bind(end)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    Json(
        rows.into_iter()
            .map(|(at, field, from, to)| SettingsChange {
                at,
                field,
                from,
                to,
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

fn parse_rfc3339(raw: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw?)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

// ── Context-selection strategy ────────────────────────────────────────────────

/// The full state of the caller's selection preferences.
///
/// `history_compression` is not a preference: it is the effective state for the caller's
/// Orchestrator chats. A direct chat follows the addressed agent's own switch.
#[derive(Debug, Serialize)]
struct ContextStrategyResponse {
    strategy: ContextSelectionStrategy,
    enabled: bool,
    history_compression: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct ContextStrategyUpdate {
    strategy: ContextSelectionStrategy,
    /// Sent only when it changed. Absent means keep.
    enabled: Option<bool>,
}

/// History compression for Orchestrator chats, resolved exactly as the chat path resolves it:
/// the deployment flag narrowed by `settings.compress_history`.
async fn history_compression_allowed(state: &AppState) -> bool {
    ContextTiers::from_config(&state.config)
        .with_overrides(&state.db)
        .await
        .compress
        .enabled
}

async fn strategy_response(
    state: &AppState,
    strategy: ContextSelectionStrategy,
    enabled: bool,
) -> ContextStrategyResponse {
    ContextStrategyResponse {
        strategy,
        enabled,
        history_compression: history_compression_allowed(state).await,
    }
}

#[utoipa::path(
    get,
    path = "/api/me/context-strategy",
    tag = "context-selection",
    responses(
        (status = 200, description = "The caller's selection strategy and effective state"),
        (status = 500, description = "Read failed (CX-4): never served as a silent default"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn get_context_strategy(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // CX-4: a failed read must 500, not quietly serve the column default as though it were the
    // caller's saved choice — the write side already refuses to report success without a row;
    // the read side owes the same honesty.
    let strategy = match ContextSelectionStrategy::for_user_checked(&state.db, user_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, %user_id, "get_context_strategy: read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    let enabled = match nasiko_orchestrator::selection_enabled_checked(&state.db, user_id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, %user_id, "get_context_strategy: read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    Json(strategy_response(&state, strategy, enabled).await).into_response()
}

#[utoipa::path(
    patch,
    path = "/api/me/context-strategy",
    tag = "context-selection",
    responses(
        (status = 200, description = "Saved; body `{strategy, enabled?}` — the strategy and effective state after the write"),
        (status = 404, description = "No such user (CX-4): the update touched no row"),
        (status = 422, description = "A value outside the enum (Axum's Json rejection)"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn update_context_strategy(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ContextStrategyUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    match apply_preferences(&state.db, user_id, Some(body.strategy), None, body.enabled).await {
        Ok(saved) => {
            Json(strategy_response(&state, saved.strategy, saved.enabled).await).into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ── PACMS budget tier ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct PacmsBudgetResponse {
    level: PacmsBudgetLevel,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct PacmsBudgetUpdate {
    level: PacmsBudgetLevel,
}

#[utoipa::path(
    get,
    path = "/api/me/pacms-budget",
    tag = "context-selection",
    responses(
        (status = 200, description = "The caller's PACMS budget tier: `{level}`"),
        (status = 500, description = "Read failed (CX-4): never served as a silent default"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn get_pacms_budget(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // CX-4: see get_context_strategy — a failed read must 500, not serve the default silently.
    let level = match PacmsBudgetLevel::for_user_checked(&state.db, user_id).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, %user_id, "get_pacms_budget: read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    Json(PacmsBudgetResponse { level }).into_response()
}

#[utoipa::path(
    patch,
    path = "/api/me/pacms-budget",
    tag = "context-selection",
    responses(
        (status = 200, description = "Saved; body `{level}` — the tier after the write"),
        (status = 404, description = "No such user (CX-4): the update touched no row"),
        (status = 422, description = "A value outside the enum (Axum's Json rejection)"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn update_pacms_budget(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<PacmsBudgetUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    match apply_preferences(&state.db, user_id, None, Some(body.level), None).await {
        Ok(saved) => Json(PacmsBudgetResponse { level: saved.level }).into_response(),
        Err(e) => e.into_response(),
    }
}

// ── One atomic save (CX-2) ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct ContextUpdate {
    strategy: Option<ContextSelectionStrategy>,
    level: Option<PacmsBudgetLevel>,
    enabled: Option<bool>,
}

/// The caller's whole preference state, after a write.
#[derive(Debug, Serialize)]
struct ContextResponse {
    strategy: ContextSelectionStrategy,
    level: PacmsBudgetLevel,
    enabled: bool,
    history_compression: bool,
}

/// `PATCH /api/me/context` — strategy, tier and switch in one transaction.
///
/// The two older routes stay for compatibility, but a Save that writes through them can half
/// succeed: the strategy lands, the tier 500s, and the page is left holding a draft that disagrees
/// with the server. One statement cannot.
#[utoipa::path(
    patch,
    path = "/api/me/context",
    tag = "context-selection",
    responses(
        (status = 200, description = "Saved; body `{strategy?, level?, enabled?}` — one transaction (CX-2), full state after the write"),
        (status = 404, description = "No such user (CX-4): the update touched no row"),
        (status = 422, description = "A value outside an enum (Axum's Json rejection)"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn update_context(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<ContextUpdate>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let saved = match apply_preferences(&state.db, user_id, body.strategy, body.level, body.enabled)
        .await
    {
        Ok(saved) => saved,
        Err(e) => return e.into_response(),
    };

    Json(ContextResponse {
        strategy: saved.strategy,
        level: saved.level,
        enabled: saved.enabled,
        history_compression: history_compression_allowed(&state).await,
    })
    .into_response()
}

/// Every preference the three write paths can set, as stored after the write.
struct SavedPreferences {
    strategy: ContextSelectionStrategy,
    level: PacmsBudgetLevel,
    enabled: bool,
}

/// Apply any subset of the three preferences in one transaction, logging what actually moved.
///
/// One function behind all three write routes, so they cannot disagree about what a write means:
/// the same "unchanged values write no history row" rule, the same missing-row handling, and the
/// same returned state.
///
/// A `None` field is left alone — absent means keep, never "set to default".
async fn apply_preferences(
    db: &PgPool,
    user_id: Uuid,
    strategy: Option<ContextSelectionStrategy>,
    level: Option<PacmsBudgetLevel>,
    enabled: Option<bool>,
) -> Result<SavedPreferences, PreferenceError> {
    let mut tx = db.begin().await.map_err(|e| {
        tracing::error!(%e, "apply_preferences: begin failed");
        PreferenceError::Internal
    })?;

    // Read the current row first: the change log needs the old values, and a missing row has to be
    // a 404 rather than a silent success against nothing.
    let current: Option<(ContextSelectionStrategy, PacmsBudgetLevel, bool)> = sqlx::query_as(
        "SELECT context_selection_strategy, pacms_budget_level, context_selection_enabled \
           FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(%e, "apply_preferences: read failed");
        PreferenceError::Internal
    })?;

    let Some((cur_strategy, cur_level, cur_enabled)) = current else {
        return Err(PreferenceError::NoSuchUser);
    };

    let next_strategy = strategy.unwrap_or(cur_strategy);
    let next_level = level.unwrap_or(cur_level);
    let next_enabled = enabled.unwrap_or(cur_enabled);

    sqlx::query(
        "UPDATE users SET context_selection_strategy = $1, pacms_budget_level = $2, \
                          context_selection_enabled = $3 \
          WHERE id = $4",
    )
    .bind(next_strategy)
    .bind(next_level)
    .bind(next_enabled)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(%e, "apply_preferences: write failed");
        PreferenceError::Internal
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!(%e, "apply_preferences: commit failed");
        PreferenceError::Internal
    })?;

    // Logged after the commit, and only for values that moved. Inside the transaction a rolled-back
    // write would still have to un-log, and a re-sent identical value is not a change.
    if next_strategy != cur_strategy {
        record_change(
            db,
            user_id,
            "strategy",
            cur_strategy.as_label(),
            next_strategy.as_label(),
        )
        .await;
    }
    if next_level != cur_level {
        record_change(
            db,
            user_id,
            "level",
            cur_level.as_label(),
            next_level.as_label(),
        )
        .await;
    }
    if next_enabled != cur_enabled {
        record_change(
            db,
            user_id,
            "enabled",
            &cur_enabled.to_string(),
            &next_enabled.to_string(),
        )
        .await;
    }

    Ok(SavedPreferences {
        strategy: next_strategy,
        level: next_level,
        enabled: next_enabled,
    })
}

enum PreferenceError {
    /// The update matched no row. Previously this answered 200, so a write against a deleted user
    /// reported success and the next GET served column defaults as if they had been saved.
    NoSuchUser,
    Internal,
}

impl IntoResponse for PreferenceError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::NoSuchUser => (StatusCode::NOT_FOUND, "user not found").into_response(),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response(),
        }
    }
}

// ── Preview (CX-6) ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(crate) struct PreviewRequest {
    strategy: ContextSelectionStrategy,
}

#[derive(Debug, Serialize)]
struct PreviewResponse {
    strategy: ContextSelectionStrategy,
    session_messages: usize,
    pool: nasiko_orchestrator::TierCounts,
    tiers: PreviewTiers,
}

#[derive(Debug, Serialize)]
struct PreviewTiers {
    low: nasiko_orchestrator::TierCounts,
    medium: nasiko_orchestrator::TierCounts,
    high: nasiko_orchestrator::TierCounts,
}

/// `POST /api/me/context-preview` — what each tier would keep, on the caller's latest chat.
///
/// Runs the real selectors over real history and sends nothing to any model, so the figures cannot
/// drift from what a turn would actually carry.
#[utoipa::path(
    post,
    path = "/api/me/context-preview",
    tag = "context-selection",
    responses(
        (status = 200, description = "Body `{strategy}`; what each tier would keep on the caller's latest chat, nothing sent to any model"),
        (status = 404, description = "`{\"error\": \"no chats yet\", \"code\": \"no_session\"}` — the caller has no chat"),
        (status = 422, description = "A value outside the enum (Axum's Json rejection)"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn context_preview(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<PreviewRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // The caller's own latest chat, and the last thing they asked in it — selection is
    // query-relevant, so previewing against an empty query would rank nothing.
    let latest: Option<(String,)> = sqlx::query_as(
        "SELECT session_id FROM chat_sessions \
          WHERE user_id = $1 ORDER BY updated_at DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let Some((session_id,)) = latest else {
        // Must carry a `code`: a bare 404 means "this server has no such route" to the page, which
        // would hide the whole table instead of saying the caller has no chats yet.
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no chats yet", "code": "no_session" })),
        )
            .into_response();
    };

    let query: String = sqlx::query_scalar(
        "SELECT content FROM chat_messages \
          WHERE session_id = $1 AND role = 'user' ORDER BY timestamp DESC LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    let store = state.history_vector_store();
    let preview = nasiko_orchestrator::preview(
        &state.db,
        &session_id,
        &store,
        &query,
        &ContextTiers::from_config(&state.config)
            .with_overrides(&state.db)
            .await,
        body.strategy,
    )
    .await;

    Json(PreviewResponse {
        strategy: body.strategy,
        session_messages: preview.session_messages,
        pool: preview.pool,
        tiers: PreviewTiers {
            low: preview.low,
            medium: preview.medium,
            high: preview.high,
        },
    })
    .into_response()
}
