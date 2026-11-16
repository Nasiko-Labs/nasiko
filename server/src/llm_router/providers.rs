//! User-facing model catalog for the LLM router (`GET /api/llm-router/providers`).
//!
//! Backs a UI provider/model dropdown. Unlike the OpenAI-compat `/v1/models` egress
//! endpoint (a flat `{id, provider}` list for agent SDKs) and `/api/model-registry`
//! (admin tier→model config), this lists every `(provider, model)` the platform knows
//! — the union of what each endpoint actually **serves** (`provider_models`, synced
//! from its `GET /models`) and what carries a **currently-effective price**
//! (`model_pricing`) — so a served-but-unpriced model (a custom provider with no
//! Portkey price book) still appears, with null prices and `pricing_available: false`,
//! and a priced-but-unlisted provider (Gemini, whose `/models` shape the catalog sync
//! can't speak) is not dropped. No metadata beyond what the DB stores is invented.
//!
//! **Custom providers are the exception: for them `provider_models` is authoritative.**
//! We asked the endpoint what it serves and it answered, so pricing may only annotate
//! that answer — never extend it. The union would otherwise let a price book invent
//! models the endpoint has never heard of: registering an Azure resource pulls Portkey's
//! whole `azure-openai` book (Grok, `text-davinci-001`, every `.ft` variant) into
//! `model_pricing` under the provider's label, and every one of those names would be
//! offered for tier routing despite resolving to nothing but a 404 at call time. Azure
//! routes by *deployment* name, which no price book can know.

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::Claims;
use crate::mcp::ApiResponse;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Read-only catalog; any authenticated user may list it (matches model_registry::list).
    Router::new().route("/llm-router/providers", get(list_providers))
}

/// The raw joined shape we read; `Decimal` prices are projected to `f64` for the
/// response (as [`crate::llm_router::model_registry`]'s neighbours and `DbPricing` do).
/// Every pricing column is optional: a `provider_models` row with no matching price
/// carries all-null prices.
#[derive(sqlx::FromRow)]
struct PricingRow {
    provider: String,
    model: String,
    input_price_per_1m: Option<Decimal>,
    output_price_per_1m: Option<Decimal>,
    cache_creation_price_per_1m: Option<Decimal>,
    cache_read_price_per_1m: Option<Decimal>,
    currency: Option<String>,
    notes: Option<String>,
    effective_from: Option<DateTime<Utc>>,
    effective_until: Option<DateTime<Utc>>,
    /// Whether `provider_models` vouches for this row — i.e. the provider's own model
    /// listing returned it. False ⇒ the row exists only because something priced it.
    /// The `COALESCE`d `provider`/`model` above cannot tell the two apart.
    served: bool,
}

/// One model within a provider group. Field names mirror the `model_pricing` columns;
/// prices are null when the model has no currently-effective price row.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelEntry {
    model: String,
    input_price_per_1m: Option<f64>,
    output_price_per_1m: Option<f64>,
    cache_creation_price_per_1m: Option<f64>,
    cache_read_price_per_1m: Option<f64>,
    currency: Option<String>,
    notes: Option<String>,
    effective_from: Option<DateTime<Utc>>,
    effective_until: Option<DateTime<Utc>>,
    /// Whether a currently-effective price row backs this model. `false` ⇒ the model is
    /// served but its cost is not tracked (shown as "cost not tracked" in the UI, not $0).
    pricing_available: bool,
}

/// A provider and its models, e.g. `{ "provider": "openai", "models": [...] }`.
#[derive(Serialize, ToSchema)]
pub(crate) struct ProviderCatalog {
    provider: String,
    /// UUID of the custom provider, if this is a DB-registered custom endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_id: Option<uuid::Uuid>,
    /// Human-friendly name of the custom provider, if this is a DB-registered custom endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    models: Vec<ModelEntry>,
}

/// `crate::mcp::ApiResponse` envelope around a list of [`ProviderCatalog`] groups.
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct ProviderCatalogEnvelope {
    data: Vec<ProviderCatalog>,
    status_code: u16,
    message: String,
}

/// List every provider/model with a currently-effective `model_pricing` row,
/// grouped by provider. Backs the UI provider/model dropdown.
#[utoipa::path(
    get,
    path = "/api/llm-router/providers",
    tag = "llm-router",
    responses(
        (status = 200, description = "Currently-effective model catalog, grouped by provider", body = ProviderCatalogEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn list_providers(
    State(state): State<AppState>,
    _claims: Claims,
) -> impl IntoResponse {
    // The union of served models (`provider_models`) and currently-effective priced
    // models (`model_pricing`). A served model with no price row shows with null
    // prices; a priced model that isn't in the catalog (e.g. Gemini) still shows.
    let rows = sqlx::query_as::<_, PricingRow>(
        r#"SELECT
               COALESCE(pm.provider, mp.provider) AS provider,
               COALESCE(pm.model, mp.model)       AS model,
               pm.provider IS NOT NULL            AS served,
               mp.input_price_per_1m, mp.output_price_per_1m,
               mp.cache_creation_price_per_1m, mp.cache_read_price_per_1m,
               mp.currency, mp.notes, mp.effective_from, mp.effective_until
           FROM provider_models pm
           FULL OUTER JOIN (
               SELECT DISTINCT ON (provider, model)
                   provider, model,
                   input_price_per_1m, output_price_per_1m,
                   cache_creation_price_per_1m, cache_read_price_per_1m,
                   currency, notes, effective_from, effective_until
               FROM model_pricing
               WHERE effective_from <= now()
                 AND (effective_until IS NULL OR effective_until > now())
               ORDER BY provider, model, effective_from DESC
           ) mp ON pm.provider = mp.provider AND pm.model = mp.model
           ORDER BY provider, model"#,
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

    // Registered custom providers are never hidden, even if their label collides with
    // a `HIDDEN_PROVIDERS` entry (e.g. an admin registers "deepseek").
    let custom_meta: std::collections::HashMap<String, (uuid::Uuid, String)> =
        match sqlx::query_as::<_, (uuid::Uuid, String, String)>(
            "SELECT id, label, display_name FROM custom_providers WHERE deleted_at IS NULL",
        )
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(id, label, name)| (label, (id, name)))
                .collect(),
            Err(e) => {
                tracing::error!(%e, "list_providers: custom provider lookup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        };
    let custom_labels: std::collections::HashSet<String> = custom_meta.keys().cloned().collect();

    ApiResponse::ok(
        json!(group_by_provider(rows, &custom_labels, &custom_meta)),
        "Providers retrieved successfully",
    )
    .into_response()
}

/// Collapse provider-ordered rows into per-provider groups. Relies on the query's
/// `ORDER BY provider` so each provider's rows arrive contiguously.
/// Normalize legacy provider names so the API returns a single canonical name.
fn normalize_provider(name: &str) -> &str {
    match name {
        "google" => "gemini",
        other => other,
    }
}

/// Providers hidden from the catalog until their router integration is ready. A
/// registered custom provider under one of these labels is exempt (see `custom_labels`).
const HIDDEN_PROVIDERS: &[&str] = &["groq", "deepseek", "amazon-bedrock"];

fn group_by_provider(
    rows: Vec<PricingRow>,
    custom_labels: &std::collections::HashSet<String>,
    custom_meta: &std::collections::HashMap<String, (uuid::Uuid, String)>,
) -> Vec<ProviderCatalog> {
    // A built-in whose own listing has synced (any served row) is authoritative too:
    // price-only names (`gemini-4-argon-lte-128k`, retired 1.5 seeds) are dropped. A
    // built-in with no listing yet keeps the union so the dropdown is never empty.
    let listed: std::collections::HashSet<String> = rows
        .iter()
        .filter(|r| r.served)
        .map(|r| normalize_provider(&r.provider).to_owned())
        .collect();
    let mut out: Vec<ProviderCatalog> = Vec::new();
    for row in rows {
        let provider = normalize_provider(&row.provider).to_owned();
        // Hide built-in-but-unready providers, but never a registered custom provider.
        if HIDDEN_PROVIDERS.contains(&provider.as_str()) && !custom_labels.contains(&provider) {
            continue;
        }
        // A custom provider's own listing is the whole truth (see the module doc): drop a
        // model only a price book claims. Built-ins keep the union — that is what carries
        // Gemini, which is priced but whose listing shape the catalog sync cannot read.
        if (custom_labels.contains(&provider) || listed.contains(&provider)) && !row.served {
            continue;
        }
        let input = row.input_price_per_1m.and_then(|d| d.to_f64());
        let output = row.output_price_per_1m.and_then(|d| d.to_f64());
        let entry = ModelEntry {
            model: row.model,
            // A price row provides both input and output; treat either present as priced.
            pricing_available: input.is_some() || output.is_some(),
            input_price_per_1m: input,
            output_price_per_1m: output,
            cache_creation_price_per_1m: row.cache_creation_price_per_1m.and_then(|d| d.to_f64()),
            cache_read_price_per_1m: row.cache_read_price_per_1m.and_then(|d| d.to_f64()),
            currency: row.currency,
            notes: row.notes,
            effective_from: row.effective_from,
            effective_until: row.effective_until,
        };
        if let Some(group) = out.iter_mut().find(|g| g.provider == provider) {
            // A model priced under both a legacy and a canonical name (`google` and `gemini`)
            // lands in the same group twice; keep the first. `ORDER BY provider` puts the
            // canonical `gemini` row ahead of `google`, so its price is the one shown.
            if !group.models.iter().any(|m| m.model == entry.model) {
                group.models.push(entry);
            }
        } else {
            let (provider_id, display_name) = custom_meta
                .get(&provider)
                .map(|(id, name)| (Some(*id), Some(name.clone())))
                .unwrap_or((None, None));
            out.push(ProviderCatalog {
                provider,
                provider_id,
                display_name,
                models: vec![entry],
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A priced row: it carries a price, so `pricing_available` is true.
    fn priced(provider: &str, model: &str, served: bool) -> PricingRow {
        PricingRow {
            provider: provider.into(),
            model: model.into(),
            served,
            input_price_per_1m: Some(Decimal::new(1, 0)),
            output_price_per_1m: Some(Decimal::new(2, 0)),
            cache_creation_price_per_1m: None,
            cache_read_price_per_1m: None,
            currency: Some("USD".into()),
            notes: None,
            effective_from: None,
            effective_until: None,
        }
    }

    /// A row the provider listed but nothing prices — the `FULL OUTER JOIN`'s left-only side.
    fn unpriced(provider: &str, model: &str) -> PricingRow {
        PricingRow {
            input_price_per_1m: None,
            output_price_per_1m: None,
            ..priced(provider, model, true)
        }
    }

    fn custom(labels: &[&str]) -> (HashSet<String>, HashMap<String, (uuid::Uuid, String)>) {
        let set: HashSet<String> = labels.iter().map(|s| (*s).to_string()).collect();
        let meta = labels
            .iter()
            .map(|l| {
                (
                    (*l).to_string(),
                    (uuid::Uuid::nil(), format!("{l} display")),
                )
            })
            .collect();
        (set, meta)
    }

    fn models_of(groups: &[ProviderCatalog], provider: &str) -> Vec<String> {
        groups
            .iter()
            .find(|g| g.provider == provider)
            .map(|g| g.models.iter().map(|m| m.model.clone()).collect())
            .unwrap_or_default()
    }

    /// The bug this guard exists for: registering an Azure resource pulls Portkey's whole
    /// `azure-openai` price book in under the provider's label, and every one of those
    /// names used to be offered for tier routing. Only the deployment the endpoint
    /// actually listed may survive.
    #[test]
    fn custom_provider_keeps_only_what_it_serves() {
        let (labels, meta) = custom(&["azure"]);
        let rows = vec![
            unpriced("azure", "gpt4o"),       // the one real deployment
            priced("azure", "grok-3", false), // price book only
            priced("azure", "text-davinci-001", false),
        ];
        assert_eq!(
            models_of(&group_by_provider(rows, &labels, &meta), "azure"),
            vec!["gpt4o"]
        );
    }

    /// Built-ins keep the union: Gemini is priced but its listing shape the catalog sync
    /// cannot read, so a price-only row is the only evidence it exists.
    #[test]
    fn builtin_provider_keeps_priced_but_unlisted_models() {
        let (labels, meta) = custom(&["azure"]);
        let rows = vec![priced("gemini", "gemini-2.0-flash", false)];
        assert_eq!(
            models_of(&group_by_provider(rows, &labels, &meta), "gemini"),
            vec!["gemini-2.0-flash"]
        );
    }

    /// Q3-7: a model priced under both `gemini` and the legacy `google` is listed once, with
    /// the `gemini` row's price (the query's order puts it first).
    #[test]
    fn model_priced_under_legacy_and_canonical_name_is_listed_once() {
        let (labels, meta) = custom(&[]);
        let mut legacy = priced("google", "gemini-2.0-flash", false);
        legacy.input_price_per_1m = Some(Decimal::new(9, 0));
        let rows = vec![
            priced("gemini", "gemini-2.0-flash", false),
            priced("gemini", "gemini-2.5-pro", false),
            legacy,
        ];
        let groups = group_by_provider(rows, &labels, &meta);
        assert_eq!(
            models_of(&groups, "gemini"),
            vec!["gemini-2.0-flash", "gemini-2.5-pro"]
        );
        assert_eq!(groups[0].models[0].input_price_per_1m, Some(1.0));
    }

    /// A custom provider's served model stays even when nothing prices it — the common
    /// case for Azure, whose deployment nicknames no price book can match.
    #[test]
    fn custom_provider_keeps_served_but_unpriced_model() {
        let (labels, meta) = custom(&["bdrock"]);
        let groups = group_by_provider(vec![unpriced("bdrock", "claude-sonnet-4")], &labels, &meta);
        let entry = &groups
            .iter()
            .find(|g| g.provider == "bdrock")
            .unwrap()
            .models[0];
        assert_eq!(entry.model, "claude-sonnet-4");
        assert!(!entry.pricing_available);
    }

    /// A custom provider under a hidden built-in's label keeps its exemption, and the new
    /// guard still applies to it.
    #[test]
    fn custom_provider_under_hidden_label_is_exempt_but_still_filtered() {
        let (labels, meta) = custom(&["deepseek"]);
        let rows = vec![
            unpriced("deepseek", "deepseek-chat"),
            priced("deepseek", "deepseek-phantom", false),
        ];
        assert_eq!(
            models_of(&group_by_provider(rows, &labels, &meta), "deepseek"),
            vec!["deepseek-chat"]
        );
    }

    /// Once Gemini's ListModels has synced, price-only names are dropped.
    #[test]
    fn builtin_with_listing_drops_price_only_models() {
        let (labels, meta) = custom(&[]);
        let rows = vec![
            unpriced("gemini", "gemini-4-argon"),
            priced("gemini", "gemini-4-argon-lte-128k", false),
            priced("gemini", "gemini-1.5-pro", false),
        ];
        assert_eq!(
            models_of(&group_by_provider(rows, &labels, &meta), "gemini"),
            vec!["gemini-4-argon"]
        );
    }

    /// A hidden built-in with no custom provider registered under its label stays hidden.
    #[test]
    fn hidden_builtin_without_custom_registration_is_dropped() {
        let (labels, meta) = custom(&[]);
        let groups = group_by_provider(vec![priced("groq", "llama-3", false)], &labels, &meta);
        assert!(groups.is_empty());
    }
}
