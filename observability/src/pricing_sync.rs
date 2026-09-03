//! Mirrors LLM token pricing from Portkey's public dataset into the
//! `model_pricing` table so cost calculation stays current without a
//! hand-written seed migration per model.
//!
//! Portkey ([`Portkey-AI/models`](https://github.com/Portkey-AI/models))
//! publishes one JSON file per provider at `configs.portkey.ai`:
//!
//! - `pricing/{provider}.json` — token prices, in **cents per token**.
//! - `general/{provider}.json`  — model metadata including `type.primary`.
//!
//! For a fixed set of providers we ingest only text models
//! (`type.primary ∈ {chat, text}`), convert cents-per-token → USD per 1M
//! tokens (`× 10_000`), and mirror the result into `model_pricing` via a
//! per-provider **delete-and-replace** transaction. That table is the source
//! of truth both the Postgres cost trigger (`calculate_token_cost`) and
//! [`crate::DbPricing`] read.
//!
//! Failure is soft: a fetch/parse error for a provider leaves that provider's
//! existing rows untouched, so we never end up with fewer prices than before.

use std::collections::HashMap;
use std::time::Duration;

use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgPool;

type SyncResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PORTKEY_BASE_URL: &str = "https://configs.portkey.ai";

/// Delay before the first sync so it never competes with boot-critical work.
const INITIAL_DELAY: Duration = Duration::from_secs(10);

/// `(portkey file stem, internal `provider` column value)`.
///
/// Gemini lives in Portkey's `google.json`, but we store it under `gemini` to
/// match the provider string our `token_usage` rows are written with (the SQL
/// cost trigger keys on an exact `(provider, model)` match).
const PROVIDERS: &[(&str, &str)] = &[
    ("openai", "openai"),
    ("anthropic", "anthropic"),
    ("google", "gemini"),
    ("deepseek", "deepseek"),
    ("groq", "groq"),
];

/// `type.primary` values we treat as text models. Everything else
/// (`embedding`, `image`, `audio`, …) is skipped.
const TEXT_PRIMARY_TYPES: &[&str] = &["chat", "text"];

/// Portkey stores cents-per-token; our table stores USD per 1M tokens.
/// `usd_per_1m = cents_per_token × 10_000`.
const CENTS_PER_TOKEN_TO_USD_PER_1M: f64 = 10_000.0;

/// Background worker: one sync shortly after boot, then every `interval_secs`.
/// Spawned once at server startup when `MODEL_PRICING_SYNC_ENABLED` is on.
pub async fn run(db: PgPool, http: reqwest::Client, interval_secs: u64) {
    tokio::time::sleep(INITIAL_DELAY).await;
    // interval() panics on a zero period; floor at 60s.
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(60)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await; // fires immediately on the first call
        sync_all(&db, &http).await;
    }
}

/// Sync every configured provider. Best-effort: a failing provider is logged
/// and skipped, leaving its existing rows in place.
pub async fn sync_all(db: &PgPool, http: &reqwest::Client) {
    let mut total = 0usize;
    for (source, internal) in PROVIDERS {
        match sync_provider(db, http, source, internal).await {
            Ok(n) => {
                total += n;
                tracing::info!(
                    provider = internal,
                    models = n,
                    "synced model pricing from portkey"
                );
            }
            Err(e) => tracing::warn!(
                provider = internal,
                error = %e,
                "model pricing sync failed; keeping existing rows"
            ),
        }
    }
    tracing::info!(total_models = total, "model pricing sync complete");
}

async fn sync_provider(
    db: &PgPool,
    http: &reqwest::Client,
    source: &str,
    internal: &str,
) -> SyncResult<usize> {
    let text_models = fetch_text_models(http, source).await?;
    if text_models.is_empty() {
        // A malformed or empty `general` file must never wipe good rows.
        return Ok(0);
    }
    let prices = fetch_prices(http, source).await?;

    let rows: Vec<PricingRow> = text_models
        .iter()
        .filter_map(|model| prices.get(model).map(|entry| (model, entry)))
        .filter_map(|(model, entry)| PricingRow::from_portkey(model, entry))
        .collect();
    if rows.is_empty() {
        return Ok(0);
    }

    let mut tx = db.begin().await?;
    // Replace this provider's rows wholesale — the dataset is small and fully
    // Portkey-owned, so mirroring is simpler and safer than row-wise diffing.
    sqlx::query("DELETE FROM model_pricing WHERE provider = $1")
        .bind(internal)
        .execute(&mut *tx)
        .await?;
    // Older seed migrations wrote Gemini rows under the `google` provider;
    // fold them into `gemini` so the legacy spelling can't shadow the sync.
    if internal == "gemini" {
        sqlx::query("DELETE FROM model_pricing WHERE provider = 'google'")
            .execute(&mut *tx)
            .await?;
    }
    for row in &rows {
        sqlx::query(
            r#"INSERT INTO model_pricing
                 (provider, model, input_price_per_1m, output_price_per_1m,
                  cache_creation_price_per_1m, cache_read_price_per_1m, notes)
               VALUES ($1, $2, $3, $4, $5, $6, 'synced from portkey')"#,
        )
        .bind(internal)
        .bind(&row.model)
        .bind(row.input)
        .bind(row.output)
        .bind(row.cache_creation)
        .bind(row.cache_read)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

/// Names of models in `general/{source}.json` whose `type.primary` is a text
/// type. The `default` pseudo-entry is skipped.
async fn fetch_text_models(http: &reqwest::Client, source: &str) -> SyncResult<Vec<String>> {
    let url = format!("{PORTKEY_BASE_URL}/general/{source}.json");
    let map = get_json_map(http, &url).await?;
    Ok(map
        .into_iter()
        .filter(|(name, entry)| name != "default" && is_text_model(entry))
        .map(|(name, _)| name)
        .collect())
}

async fn fetch_prices(http: &reqwest::Client, source: &str) -> SyncResult<HashMap<String, Value>> {
    let url = format!("{PORTKEY_BASE_URL}/pricing/{source}.json");
    get_json_map(http, &url).await
}

async fn get_json_map(http: &reqwest::Client, url: &str) -> SyncResult<HashMap<String, Value>> {
    Ok(http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json::<HashMap<String, Value>>()
        .await?)
}

/// Community data can carry odd entries, so navigate the JSON leniently rather
/// than deriving a rigid struct that would fail the whole file on one bad row.
fn is_text_model(entry: &Value) -> bool {
    entry
        .get("type")
        .and_then(|t| t.get("primary"))
        .and_then(Value::as_str)
        .is_some_and(|primary| TEXT_PRIMARY_TYPES.contains(&primary))
}

/// One `model_pricing` row derived from a Portkey pricing entry, already
/// converted to USD per 1M tokens.
struct PricingRow {
    model: String,
    input: Decimal,
    output: Decimal,
    cache_creation: Option<Decimal>,
    cache_read: Option<Decimal>,
}

impl PricingRow {
    fn from_portkey(model: &str, entry: &Value) -> Option<Self> {
        let payg = entry.get("pricing_config")?.get("pay_as_you_go")?;
        // Skip models that don't price both request and response tokens —
        // input/output are NOT NULL in the schema.
        let input = usd_per_1m(payg, "request_token")?;
        let output = usd_per_1m(payg, "response_token")?;
        Some(Self {
            model: model.to_string(),
            input,
            output,
            cache_creation: usd_per_1m(payg, "cache_write_input_token"),
            cache_read: usd_per_1m(payg, "cache_read_input_token"),
        })
    }
}

/// `pay_as_you_go.{key}.price` (cents/token) → USD per 1M tokens, rounded to
/// the table's `DECIMAL(10, 4)` scale. `None` if the field is absent.
fn usd_per_1m(payg: &Value, key: &str) -> Option<Decimal> {
    let cents_per_token = payg.get(key)?.get("price")?.as_f64()?;
    let usd_per_1m = cents_per_token * CENTS_PER_TOKEN_TO_USD_PER_1M;
    Decimal::from_f64_retain(usd_per_1m).map(|d| d.round_dp(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_cents_per_token_to_usd_per_1m() {
        // deepseek-chat from Portkey: request 0.000014 c/tok → $0.14 / 1M.
        let entry = serde_json::json!({
            "pricing_config": {
                "pay_as_you_go": {
                    "request_token": { "price": 0.000014 },
                    "response_token": { "price": 0.000028 },
                    "cache_read_input_token": { "price": 0.00000028 }
                }
            }
        });
        let row = PricingRow::from_portkey("deepseek-chat", &entry).unwrap();
        assert_eq!(
            row.input,
            Decimal::from_f64_retain(0.14).unwrap().round_dp(4)
        );
        assert_eq!(
            row.output,
            Decimal::from_f64_retain(0.28).unwrap().round_dp(4)
        );
        assert_eq!(
            row.cache_read,
            Some(Decimal::from_f64_retain(0.0028).unwrap().round_dp(4))
        );
        // cache_write absent → NULL, not zero.
        assert_eq!(row.cache_creation, None);
    }

    #[test]
    fn skips_models_missing_request_or_response_price() {
        let entry = serde_json::json!({
            "pricing_config": { "pay_as_you_go": { "request_token": { "price": 0.00001 } } }
        });
        assert!(PricingRow::from_portkey("no-output", &entry).is_none());
    }

    #[test]
    fn text_filter_matches_only_chat_and_text() {
        assert!(is_text_model(
            &serde_json::json!({ "type": { "primary": "chat" } })
        ));
        assert!(is_text_model(
            &serde_json::json!({ "type": { "primary": "text" } })
        ));
        assert!(!is_text_model(
            &serde_json::json!({ "type": { "primary": "embedding" } })
        ));
        assert!(!is_text_model(
            &serde_json::json!({ "type": { "primary": "image" } })
        ));
        assert!(!is_text_model(&serde_json::json!({ "params": [] })));
    }
}
