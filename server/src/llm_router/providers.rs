//! User-facing model catalog for the LLM router (`GET /api/llm-router/providers`).
//!
//! Backs a UI provider/model dropdown. Unlike the OpenAI-compat `/v1/models` egress
//! endpoint (a flat `{id, provider}` list for agent SDKs) and `/api/model-registry`
//! (admin tier→model config), this groups the **currently-effective** rows of the
//! `model_pricing` table by provider and exposes every column that table carries —
//! prices, currency, notes, and the temporal window. No metadata beyond what the DB
//! already stores is invented here.

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;
use serde_json::json;

use crate::auth::Claims;
use crate::mcp::ApiResponse;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Read-only catalog; any authenticated user may list it (matches model_registry::list).
    Router::new().route("/llm-router/providers", get(list_providers))
}

/// The raw `model_pricing` shape we read; `Decimal` prices are projected to `f64` for
/// the response (as [`crate::llm_router::model_registry`]'s neighbours and `DbPricing` do).
#[derive(sqlx::FromRow)]
struct PricingRow {
    provider: String,
    model: String,
    input_price_per_1m: Decimal,
    output_price_per_1m: Decimal,
    cache_creation_price_per_1m: Option<Decimal>,
    cache_read_price_per_1m: Option<Decimal>,
    currency: String,
    notes: Option<String>,
    effective_from: DateTime<Utc>,
    effective_until: Option<DateTime<Utc>>,
}

/// One model within a provider group. Field names mirror the `model_pricing` columns.
#[derive(Serialize)]
struct ModelEntry {
    model: String,
    input_price_per_1m: f64,
    output_price_per_1m: f64,
    cache_creation_price_per_1m: Option<f64>,
    cache_read_price_per_1m: Option<f64>,
    currency: String,
    notes: Option<String>,
    effective_from: DateTime<Utc>,
    effective_until: Option<DateTime<Utc>>,
}

/// A provider and its models, e.g. `{ "provider": "openai", "models": [...] }`.
#[derive(Serialize)]
struct ProviderCatalog {
    provider: String,
    models: Vec<ModelEntry>,
}

async fn list_providers(State(state): State<AppState>, _claims: Claims) -> impl IntoResponse {
    // One row per (provider, model): the latest window that is effective right now.
    let rows = sqlx::query_as::<_, PricingRow>(
        r#"SELECT DISTINCT ON (provider, model)
               provider, model,
               input_price_per_1m, output_price_per_1m,
               cache_creation_price_per_1m, cache_read_price_per_1m,
               currency, notes, effective_from, effective_until
           FROM model_pricing
           WHERE effective_from <= now()
             AND (effective_until IS NULL OR effective_until > now())
           ORDER BY provider, model, effective_from DESC"#,
    )
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, "list_providers: db error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    ApiResponse::ok(
        json!(group_by_provider(rows)),
        "Providers retrieved successfully",
    )
    .into_response()
}

/// Collapse provider-ordered rows into per-provider groups. Relies on the query's
/// `ORDER BY provider` so each provider's rows arrive contiguously.
fn group_by_provider(rows: Vec<PricingRow>) -> Vec<ProviderCatalog> {
    let mut out: Vec<ProviderCatalog> = Vec::new();
    for row in rows {
        let entry = ModelEntry {
            model: row.model,
            input_price_per_1m: row.input_price_per_1m.to_f64().unwrap_or(0.0),
            output_price_per_1m: row.output_price_per_1m.to_f64().unwrap_or(0.0),
            cache_creation_price_per_1m: row.cache_creation_price_per_1m.and_then(|d| d.to_f64()),
            cache_read_price_per_1m: row.cache_read_price_per_1m.and_then(|d| d.to_f64()),
            currency: row.currency,
            notes: row.notes,
            effective_from: row.effective_from,
            effective_until: row.effective_until,
        };
        match out.last_mut() {
            Some(group) if group.provider == row.provider => group.models.push(entry),
            _ => out.push(ProviderCatalog {
                provider: row.provider,
                models: vec![entry],
            }),
        }
    }
    out
}
