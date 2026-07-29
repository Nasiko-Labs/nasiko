//! Admin API for the tier→model registry (`model_registry` table).
//!
//! The table is seeded at migration time (migration 021) with sensible defaults, which the
//! smart router's `PgTierRegistry` reads (falling back to compiled-in static seeds on a
//! missing row/DB error). These routes let an operator override those defaults: point a
//! `(provider, tier)` pair at whatever concrete model they want.
//!
//! - `GET  /api/model-registry` — list all configured mappings (any authenticated user).
//! - `PUT  /api/model-registry` — upsert one `(provider, tier)` → model mapping (superuser).

use axum::{
    Json, Router, extract::State, http::StatusCode, middleware, response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Claims;
use crate::auth::rbac::require_superuser;
use crate::mcp::ApiResponse;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Mutations are superuser-only (platform-wide config), matching settings::router().
    let write = Router::new()
        .route("/model-registry", axum::routing::put(upsert_mapping))
        .layer(middleware::from_fn(require_superuser));

    Router::new()
        .route("/model-registry", get(list_mappings))
        .merge(write)
}

/// A single `(provider, tier)` → model row.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ModelMapping {
    pub provider: String,
    /// Model strength tier: 1 = strongest … 3 = smallest.
    pub tier: i16,
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct UpsertMappingRequest {
    pub provider: String,
    pub tier: i16,
    pub model: String,
}

async fn list_mappings(State(state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, ModelMapping>(
        "SELECT provider, tier, model FROM model_registry ORDER BY provider, tier",
    )
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(r) => {
            ApiResponse::ok(json!(r), "Model registry retrieved successfully").into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_mappings: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn upsert_mapping(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<UpsertMappingRequest>,
) -> impl IntoResponse {
    // Match the DB CHECK constraint so callers get a clean 400 instead of a 500.
    if !(1..=3).contains(&body.tier) {
        return (StatusCode::BAD_REQUEST, "tier must be 1, 2, or 3").into_response();
    }
    // Provider is stored lowercased so it matches PgTierRegistry's case-insensitive lookup.
    let provider = body.provider.trim().to_ascii_lowercase();
    if provider.is_empty() {
        return (StatusCode::BAD_REQUEST, "provider is required").into_response();
    }
    let model = body.model.trim();
    if model.is_empty() {
        return (StatusCode::BAD_REQUEST, "model is required").into_response();
    }

    let result = sqlx::query_as::<_, ModelMapping>(
        r#"INSERT INTO model_registry (provider, tier, model)
           VALUES ($1, $2, $3)
           ON CONFLICT (provider, tier) DO UPDATE SET model = EXCLUDED.model
           RETURNING provider, tier, model"#,
    )
    .bind(&provider)
    .bind(body.tier)
    .bind(model)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(m) => {
            ApiResponse::ok(json!(m), "Model mapping upserted successfully").into_response()
        }
        Err(e) => {
            tracing::error!(%e, "upsert_mapping: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
