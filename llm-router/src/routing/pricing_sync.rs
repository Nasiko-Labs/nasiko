//! Pricing sync from the Portkey models database (`github.com/Portkey-AI/models`,
//! MIT) — a community-maintained price book for 2,000+ models across 40+ providers,
//! served over a free, no-auth JSON API. Provider list endpoints don't expose pricing,
//! so without this the platform is stuck hand-seeding `model_pricing` rows that go
//! stale (the "stale by Friday" problem).
//!
//! For every provider with a platform key configured (same set as the model-catalog
//! sync), we resolve the upstream's Portkey slug — env override
//! `PORTKEY_PROVIDER_<LABEL>`, then a host mapping of the configured base URL
//! (`api.deepseek.com` → `deepseek`), then the label itself — fetch
//! `{PORTKEY_PRICING_BASE_URL}/pricing/{slug}.json`, and upsert into `model_pricing`
//! with real price history: a model whose prices changed gets its current row closed
//! (`effective_until`) and a new row opened; unchanged prices are left untouched, so a
//! sync is a no-op when nothing moved.
//!
//! Rows are written under the router's own provider label (e.g. `openai` even when the
//! upstream is DeepSeek) so exact `(provider, model)` cost lookups hit; the model-only
//! fallback keeps them visible to the other pricing paths. Curated seed rows stay as
//! the offline baseline — a failed sync changes nothing.
//!
//! Unit conversion: Portkey prices are **cents per token**; `model_pricing` is USD
//! per 1M tokens — multiply by 10,000.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::config::GatewayConfig;

/// Upper bound on a pricing fetch.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Delay before the first sync so it never competes with boot-critical work.
const INITIAL_DELAY: Duration = Duration::from_secs(10);

/// `tokio::time::interval` panics on a zero period, and `PRICING_SYNC_INTERVAL_SECS`
/// is operator-supplied, so floor it rather than trusting the input.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Default Portkey pricing API base (no auth required).
const DEFAULT_PRICING_BASE: &str = "https://configs.portkey.ai";

/// OpenRouter's own model catalog (public, no auth) — it publishes live per-token
/// pricing directly, so unlike the other spokes this needs no Portkey slug lookup.
const DEFAULT_OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// One model's converted prices, USD per 1M tokens. Cache columns are
/// both-or-neither (see [`ModelPrices::cache`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrices {
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub cache_creation_per_1m: Option<f64>,
    pub cache_creation_1h_per_1m: Option<f64>,
    pub cache_read_per_1m: Option<f64>,
}

impl ModelPrices {
    /// From Portkey's `pay_as_you_go` object (cents per token). `None` when the
    /// required input/output prices are missing.
    fn from_pay_as_you_go(payg: &serde_json::Value) -> Option<Self> {
        let cents_to_usd_per_1m = |v: &serde_json::Value| {
            v.get("price")
                .and_then(|p| p.as_f64())
                .map(|c| c * 10_000.0)
                .filter(|rate| rate.is_finite() && *rate >= 0.0 && *rate < 1_000_000.0)
        };
        let input = cents_to_usd_per_1m(payg.get("request_token")?)?;
        let output = cents_to_usd_per_1m(payg.get("response_token")?)?;
        Some(Self {
            input_per_1m: round4(input),
            output_per_1m: round4(output),
            cache_creation_1h_per_1m: cents_to_usd_per_1m(
                &payg["additional_units"]["cache_write_1h"],
            )
            .map(round4),
            ..Self::cache(
                cents_to_usd_per_1m(&payg["cache_write_input_token"]),
                cents_to_usd_per_1m(&payg["cache_read_input_token"]),
            )
        })
    }

    /// Absence means unknown, not free. Each cache class resolves independently.
    fn cache(write: Option<f64>, read: Option<f64>) -> Self {
        let creation = write.map(round4);
        let read = read.map(round4);
        Self {
            input_per_1m: 0.0,
            output_per_1m: 0.0,
            cache_creation_per_1m: creation,
            cache_creation_1h_per_1m: None,
            cache_read_per_1m: read,
        }
    }
}

/// model_pricing is DECIMAL(10,4) — round to 4dp so change detection compares what
/// would actually be stored (no churn from sub-4dp noise).
fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Resolve the Portkey pricing slug for one of our provider labels: explicit env
/// override (`PORTKEY_PROVIDER_OPENAI=deepseek`), then a host mapping of the
/// configured base URL, then the label itself (correct for canonical endpoints).
///
/// The host mapping is what lets a DB-registered provider price itself with no
/// operator configuration, which is the common case: a label is chosen for the
/// UI, not to match an upstream price book, so falling through to it usually
/// misses. A Bedrock provider labelled `aws-bedrock` asked Portkey for
/// `aws-bedrock.json` and got a 403 — the book is served as `bedrock.json` — so
/// every Bedrock model went unpriced, and its traffic was costed against
/// whichever other book happened to carry the same model name.
fn portkey_slug(label: &str, api_base: &str) -> String {
    if let Some(slug) = slug_from_env(label) {
        return slug;
    }
    let host = reqwest::Url::parse(api_base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    // Matched by suffix rather than exact host: both live on hostnames that vary
    // per customer (Azure) or per region (Bedrock).
    if host.ends_with(".openai.azure.com") {
        return "azure-openai".to_string();
    }
    if host.ends_with(".amazonaws.com") {
        return "bedrock".to_string();
    }
    match host.as_str() {
        "api.openai.com" => "openai",
        "api.deepseek.com" => "deepseek",
        "api.anthropic.com" => "anthropic",
        "generativelanguage.googleapis.com" => "google",
        "api.mistral.ai" => "mistral-ai",
        "api.groq.com" => "groq",
        "api.together.xyz" => "together-ai",
        "api.x.ai" => "x-ai",
        "api.tokenfactory.nebius.com" => "nebius",
        _ => label,
    }
    .to_string()
}

/// `PORTKEY_PROVIDER_<LABEL>`, with hyphens folded to underscores.
///
/// Labels routinely contain hyphens (`aws-bedrock`, `nebius-token-factory`), and
/// `PORTKEY_PROVIDER_AWS-BEDROCK` is not a name a shell can export — so the
/// override was unusable for exactly the providers most likely to need it. The
/// hyphenated spelling is still accepted: a container runtime can set it, and an
/// operator may already have.
fn slug_from_env(label: &str) -> Option<String> {
    let upper = label.to_ascii_uppercase();
    [upper.replace('-', "_"), upper]
        .iter()
        .filter_map(|name| std::env::var(format!("PORTKEY_PROVIDER_{name}")).ok())
        .find(|slug| !slug.is_empty())
}

/// Fetch and convert one provider's price book. `None` on any failure (fail open —
/// existing pricing rows stay).
async fn fetch_price_book(
    http: &reqwest::Client,
    pricing_base: &str,
    slug: &str,
) -> Option<HashMap<String, ModelPrices>> {
    let url = format!("{pricing_base}/pricing/{slug}.json");
    let body: serde_json::Value = http
        .get(&url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                slug = %slug, error = %e,
                "pricing sync: fetch failed — keeping existing pricing"
            );
            e
        })
        .ok()?
        .json()
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                slug = %slug, error = %e,
                "pricing sync: response parse failed — keeping existing pricing"
            );
            e
        })
        .ok()?;
    let map = body.as_object()?;
    let prices: HashMap<String, ModelPrices> = map
        .iter()
        // `default` is Portkey's per-provider fallback stanza, not a model anyone can
        // call; ingesting it writes a bogus `model = 'default'` price row.
        .filter(|(model, _)| model.as_str() != "default")
        .filter_map(|(model, entry)| {
            let payg = &entry["pricing_config"]["pay_as_you_go"];
            ModelPrices::from_pay_as_you_go(payg).map(|p| (model.clone(), p))
        })
        .collect();
    tracing::info!(
        target: "nasiko::llm_router::pricing_sync",
        slug = %slug, models = prices.len(),
        "pricing sync: fetched price book"
    );
    Some(prices)
}

/// OpenRouter's `pricing` object is USD **per token** as decimal strings (e.g.
/// `"0.00000003"`), not cents like Portkey — multiply by 1e6 for USD per 1M tokens.
/// Some entries (its own meta/auto-routers) report a `"-1"` sentinel for "variable,
/// not a fixed rate" — those aren't real prices, so they're excluded rather than
/// stored as a nonsensical negative cost.
fn openrouter_prices_from_pricing(pricing: &serde_json::Value) -> Option<ModelPrices> {
    let per_token = |key: &str| -> Option<f64> { pricing.get(key)?.as_str()?.parse().ok() };
    let input = per_token("prompt")? * 1_000_000.0;
    let output = per_token("completion")? * 1_000_000.0;
    if input < 0.0 || output < 0.0 {
        return None;
    }
    Some(ModelPrices {
        input_per_1m: round4(input),
        output_per_1m: round4(output),
        cache_creation_per_1m: None,
        cache_creation_1h_per_1m: None,
        cache_read_per_1m: None,
    })
}

/// Fetch OpenRouter's full model catalog and convert every model's pricing. `None`
/// on any failure (fail open — existing pricing rows stay).
async fn fetch_openrouter_catalog(
    http: &reqwest::Client,
    models_url: &str,
) -> Option<HashMap<String, ModelPrices>> {
    let body: serde_json::Value = http
        .get(models_url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                error = %e,
                "openrouter pricing sync: fetch failed — keeping existing pricing"
            );
            e
        })
        .ok()?
        .json()
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                error = %e,
                "openrouter pricing sync: response parse failed — keeping existing pricing"
            );
            e
        })
        .ok()?;
    let models = body.get("data")?.as_array()?;
    let prices: HashMap<String, ModelPrices> = models
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let priced = openrouter_prices_from_pricing(m.get("pricing")?)?;
            Some((id.to_string(), priced))
        })
        .collect();
    tracing::info!(
        target: "nasiko::llm_router::pricing_sync",
        models = prices.len(),
        "openrouter pricing sync: fetched catalog"
    );
    Some(prices)
}

/// Current active prices per model (latest effective row, any provider label — a
/// model whose seed row already matches needs no new row).
async fn current_prices(
    db: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    label: &str,
    models: &[&str],
) -> Result<HashMap<String, ModelPrices>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        model: String,
        input: f64,
        output: f64,
        cache_creation: Option<f64>,
        cache_creation_1h: Option<f64>,
        cache_read: Option<f64>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        r#"SELECT DISTINCT ON (model)
                  model,
                  input_price_per_1m::float8 AS input,
                  output_price_per_1m::float8 AS output,
                  cache_creation_price_per_1m::float8 AS cache_creation,
                  cache_creation_1h_price_per_1m::float8 AS cache_creation_1h,
                  cache_read_price_per_1m::float8 AS cache_read
           FROM model_pricing
           WHERE model = ANY($1::text[]) AND provider = $2 AND effective_until IS NULL
           ORDER BY model, effective_from DESC"#,
    )
    .bind(models)
    .bind(label)
    .fetch_all(&mut **db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.model,
                ModelPrices {
                    input_per_1m: r.input,
                    output_per_1m: r.output,
                    cache_creation_per_1m: r.cache_creation,
                    cache_creation_1h_per_1m: r.cache_creation_1h,
                    cache_read_per_1m: r.cache_read,
                },
            )
        })
        .collect())
}

/// Whether a row was written by us (and may be replaced by a newer book) or by an
/// operator (and must be preserved — a negotiated rate no public book carries).
///
/// `seed:` is the prefix every migration-seeded row carries, so correcting a seed
/// rate needs no new literal here. The two bare sentences below predate it and
/// still exist in deployed databases.
fn sync_managed_note(note: Option<&str>) -> bool {
    note.is_some_and(|note| {
        note.starts_with("portkey pricing sync")
            || note.starts_with("openrouter pricing sync")
            || note.starts_with("seed:")
            || note == "boot seed (static list)"
            || matches!(
                note,
                "Claude Opus 5 - rate carried forward from Opus 4, verify"
                    | "Claude Sonnet 5 - rate carried forward from Sonnet 4, verify"
            )
    })
}

/// Sync one provider label's price book into `model_pricing`, returning the number of
/// rows inserted (price changes + newly known models).
async fn sync_label(
    db: &PgPool,
    label: &str,
    book: &HashMap<String, ModelPrices>,
    source: &str,
) -> Result<usize, sqlx::Error> {
    let mut inserted = 0;
    for (model, new) in book {
        let mut tx = db.begin().await?;
        // Serialize overlapping boot/on-register passes before reading current prices.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("pricing-sync:{label}:{model}"))
            .execute(&mut *tx)
            .await?;
        let current = current_prices(&mut tx, label, &[model.as_str()]).await?;
        let notes: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT notes FROM model_pricing WHERE provider=$1 AND model=$2 AND effective_until IS NULL",
        ).bind(label).bind(model).fetch_all(&mut *tx).await?;
        if notes.iter().any(|note| !sync_managed_note(note.as_deref())) {
            tracing::debug!(label, model, "pricing sync: preserving operator price row");
            tx.commit().await?;
            continue;
        }
        if current.get(model) == Some(new) {
            tx.commit().await?;
            continue;
        }
        // Close this label's active row for the model (history), then open the new
        // one. Other labels' rows are left alone — model-only lookups order by
        // effective_from DESC, so this newer row wins.
        //
        // Both statements in one transaction, so a concurrent cost lookup never sees
        // the model with no active row. `now()` is the transaction timestamp, so the
        // closed row's `effective_until` equals the new row's `effective_from`
        // exactly — the point-in-time lookup in `calculate_token_cost` has no gap to
        // fall into. Per model rather than per label: a failure part-way through a
        // price book keeps the changes already applied instead of discarding them,
        // and no single transaction holds locks across ~2000 models.
        sqlx::query(
            "UPDATE model_pricing SET effective_until = now() \
             WHERE provider = $1 AND model = $2 AND effective_until IS NULL",
        )
        .bind(label)
        .bind(model)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO model_pricing \
             (provider, model, input_price_per_1m, output_price_per_1m, \
              cache_creation_price_per_1m, cache_read_price_per_1m, notes, cache_creation_1h_price_per_1m) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(label)
        .bind(model)
        .bind(new.input_per_1m)
        .bind(new.output_per_1m)
        .bind(new.cache_creation_per_1m)
        .bind(new.cache_read_per_1m)
        .bind(source)
        .bind(new.cache_creation_1h_per_1m)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        inserted += 1;
    }
    Ok(inserted)
}

/// Sync pricing for a single provider by label and base URL. Resolves the Portkey
/// slug, fetches the price book, and upserts into `model_pricing`. Returns the
/// number of rows inserted. Called at registration time so a newly added custom
/// provider has prices immediately — without waiting for the 24h background loop.
pub async fn sync_one_provider(
    db: &PgPool,
    http: &reqwest::Client,
    label: &str,
    api_base: &str,
) -> usize {
    let pricing_base = std::env::var("PORTKEY_PRICING_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_PRICING_BASE.to_string());
    let slug = portkey_slug(label, api_base);
    match sync_provider_prices(db, http, label, api_base, &pricing_base).await {
        Ok(n) => {
            tracing::info!(
                target: "nasiko::llm_router::pricing_sync",
                label = %label, slug = %slug, rows_inserted = n,
                "pricing sync (on-register): price book applied"
            );
            n
        }
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                label = %label, error = %e,
                "pricing sync (on-register): DB write failed"
            );
            0
        }
    }
}

/// Sync a provider using an explicitly supplied public price-book endpoint.
/// Returns an error on fetch failure so verification cannot mistake failure for a no-op.
pub async fn sync_provider_prices(
    db: &PgPool,
    http: &reqwest::Client,
    label: &str,
    api_base: &str,
    pricing_base: &str,
) -> Result<usize, String> {
    let slug = portkey_slug(label, api_base);
    let book = fetch_price_book(http, pricing_base, &slug)
        .await
        .ok_or("price book fetch failed")?;
    if book.is_empty() {
        return Err("price book contains no valid models".into());
    }
    let source = format!(
        "portkey pricing sync: {pricing_base}/pricing/{slug}.json; fetched_at={}",
        chrono::Utc::now().to_rfc3339()
    );
    sync_label(db, label, &book, &source)
        .await
        .map_err(|error| error.to_string())
}

/// One pricing-sync pass over every configured provider. Returns rows inserted.
pub async fn sync_once(db: &PgPool, http: &reqwest::Client, cfg: &GatewayConfig) -> usize {
    let pricing_base = std::env::var("PORTKEY_PRICING_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_PRICING_BASE.to_string());
    let mut inserted = 0;
    // Include DB-registered custom providers so a private gateway with a Portkey
    // price book gets real prices; most have none, which is expected and harmless.
    let custom = super::catalog::load_custom_providers(db).await;
    let mut providers = super::catalog::priceable_providers(cfg, &custom);
    // Public price books need no inference key, so coverage must not be gated on
    // one. This was conditional on a Claude coding-agent row existing, which made
    // the Anthropic book's coverage depend on *when* the integration was installed
    // relative to a pass: install it a minute after boot and every Claude call was
    // priced from the offline seed until the next tick, 24h later. Unconditional
    // matches how OpenRouter's book is already fetched below.
    if !providers.iter().any(|(label, _)| label == "anthropic") {
        providers.push(("anthropic".into(), "https://api.anthropic.com".into()));
    }
    for (label, api_base) in providers {
        let slug = portkey_slug(&label, &api_base);
        let Some(book) = fetch_price_book(http, &pricing_base, &slug).await else {
            continue;
        };
        let source = format!(
            "portkey pricing sync: {pricing_base}/pricing/{slug}.json; fetched_at={}",
            chrono::Utc::now().to_rfc3339()
        );
        match sync_label(db, &label, &book, &source).await {
            Ok(n) => {
                inserted += n;
                tracing::info!(
                    target: "nasiko::llm_router::pricing_sync",
                    label = %label, slug = %slug, rows_inserted = n,
                    "pricing sync: provider price book applied"
                );
            }
            Err(e) => tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                label = %label, error = %e,
                "pricing sync: DB write failed"
            ),
        }
    }

    // OpenRouter publishes its own catalog+pricing (public, no auth) — always synced
    // regardless of whether a platform key is configured, so the model picker is
    // populated even before an operator has wired up billing for it.
    let openrouter_models_url = std::env::var("OPENROUTER_MODELS_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENROUTER_MODELS_URL.to_string());
    if let Some(book) = fetch_openrouter_catalog(http, &openrouter_models_url).await {
        match sync_label(db, "openrouter", &book, "openrouter pricing sync").await {
            Ok(n) => {
                inserted += n;
                tracing::info!(
                    target: "nasiko::llm_router::pricing_sync",
                    rows_inserted = n,
                    "openrouter pricing sync: catalog applied"
                );
            }
            Err(e) => tracing::warn!(
                target: "nasiko::llm_router::pricing_sync",
                error = %e,
                "openrouter pricing sync: DB write failed"
            ),
        }
    }

    inserted
}

/// Spawn the background pricing-sync loop: a first sync shortly after boot, then every
/// `PRICING_SYNC_INTERVAL_SECS` (default 24h — prices move slowly, floored at
/// [`MIN_INTERVAL`]). Fail-open; never panics, never blocks serving.
///
/// Takes the already-resolved [`GatewayConfig`] rather than re-reading the environment,
/// so the router's effective config is decided once at the composition root. Whether to
/// spawn at all is the caller's decision (`MODEL_PRICING_SYNC_ENABLED`) — this loop
/// reaches the network on its first tick.
pub fn spawn_sync(db: PgPool, http: reqwest::Client, cfg: Arc<GatewayConfig>) {
    let interval = Duration::from_secs(cfg.pricing_sync_interval_secs).max(MIN_INTERVAL);
    tokio::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;
        let mut tick = tokio::time::interval(interval);
        // A pass that overruns the period must not then fire back-to-back.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // interval's first tick completes immediately → sync once boot has settled.
            tick.tick().await;
            let inserted = sync_once(&db, &http, &cfg).await;
            tracing::info!(
                target: "nasiko::llm_router::pricing_sync",
                rows_inserted = inserted, interval_secs = interval.as_secs(),
                "pricing sync pass complete"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_cents_per_token_to_usd_per_1m() {
        // DeepSeek v4-flash as served by Portkey: 1.4e-05 ¢/tok in → $0.14/1M.
        let payg = json!({
            "request_token": {"price": 1.4e-05},
            "response_token": {"price": 2.8e-05},
            "cache_write_input_token": {"price": 0},
            "cache_read_input_token": {"price": 2.8e-07}
        });
        let p = ModelPrices::from_pay_as_you_go(&payg).unwrap();
        assert_eq!(p.input_per_1m, 0.14);
        assert_eq!(p.output_per_1m, 0.28);
        assert_eq!(p.cache_creation_per_1m, Some(0.0));
        assert_eq!(p.cache_read_per_1m, Some(0.0028));
    }

    #[test]
    fn missing_cache_prices_remain_unknown_independently() {
        let payg = json!({
            "request_token": {"price": 0.00025},
            "response_token": {"price": 0.001}
        });
        let p = ModelPrices::from_pay_as_you_go(&payg).unwrap();
        assert_eq!(p.cache_creation_per_1m, None);
        assert_eq!(p.cache_read_per_1m, None);

        // A listed read price does not establish that writes are free.
        let payg = json!({
            "request_token": {"price": 0.00025},
            "response_token": {"price": 0.001},
            "cache_read_input_token": {"price": 0.000025}
        });
        let p = ModelPrices::from_pay_as_you_go(&payg).unwrap();
        assert_eq!(p.cache_creation_per_1m, None);
        assert_eq!(p.cache_read_per_1m, Some(0.25));
    }

    #[test]
    fn one_hour_write_rate_is_preserved_from_the_upstream_book() {
        let prices = ModelPrices::from_pay_as_you_go(&json!({
            "request_token": {"price": 0.0005}, "response_token": {"price": 0.0025},
            "cache_write_input_token": {"price": 0.000625},
            "cache_read_input_token": {"price": 0.00005},
            "additional_units": {"cache_write_1h": {"price": 0.001}}
        }))
        .unwrap();
        assert_eq!(prices.cache_creation_per_1m, Some(6.25));
        assert_eq!(prices.cache_creation_1h_per_1m, Some(10.0));
        assert_eq!(prices.cache_read_per_1m, Some(0.5));
    }

    #[test]
    fn invalid_prices_are_not_imported_as_rates() {
        for value in [-1.0, 1e100] {
            assert!(
                ModelPrices::from_pay_as_you_go(&json!({
                    "request_token": {"price": value}, "response_token": {"price": 0.001}
                }))
                .is_none()
            );
        }
    }

    #[test]
    fn missing_input_or_output_price_skips_the_model() {
        assert!(
            ModelPrices::from_pay_as_you_go(&json!({"response_token": {"price": 1}})).is_none()
        );
        assert!(ModelPrices::from_pay_as_you_go(&json!({})).is_none());
    }

    #[test]
    fn slug_resolution_prefers_env_then_host_then_label() {
        // Host mapping: DeepSeek behind the openai label.
        assert_eq!(
            portkey_slug("openai", "https://api.deepseek.com/v1"),
            "deepseek"
        );
        assert_eq!(
            portkey_slug("anthropic", "https://api.anthropic.com/v1"),
            "anthropic"
        );
        // Unknown host falls back to the label.
        assert_eq!(portkey_slug("openai", "http://localhost:9100/v1"), "openai");
        // Env override wins.
        unsafe { std::env::set_var("PORTKEY_PROVIDER_OPENAI", "azure-openai") };
        assert_eq!(
            portkey_slug("openai", "https://api.openai.com/v1"),
            "azure-openai"
        );
        unsafe { std::env::remove_var("PORTKEY_PROVIDER_OPENAI") };
    }

    #[test]
    fn bedrock_resolves_by_host_whatever_the_provider_is_labelled() {
        // The label is chosen for the UI, so the fallback is wrong here: Portkey
        // serves the book as `bedrock.json` and 403s on anything else, which left
        // every Bedrock model unpriced.
        for label in ["aws-bedrock", "amazon-bedrock", "my-bedrock-gateway"] {
            assert_eq!(
                portkey_slug(label, "https://bedrock-runtime.us-east-1.amazonaws.com"),
                "bedrock",
                "label {label} did not resolve by host"
            );
        }
        // Region is part of the host, and one book covers them all.
        assert_eq!(
            portkey_slug(
                "aws-bedrock",
                "https://bedrock-runtime.eu-west-1.amazonaws.com/v1"
            ),
            "bedrock"
        );
    }

    #[test]
    fn a_hyphenated_label_can_be_overridden_from_the_environment() {
        // `PORTKEY_PROVIDER_AWS-BEDROCK` is not exportable from a shell, so the
        // underscored spelling is what an operator can actually set.
        unsafe { std::env::set_var("PORTKEY_PROVIDER_AWS_BEDROCK", "bedrock") };
        assert_eq!(
            portkey_slug("aws-bedrock", "https://example.invalid/v1"),
            "bedrock"
        );
        unsafe { std::env::remove_var("PORTKEY_PROVIDER_AWS_BEDROCK") };

        // The hyphenated name still works where a runtime can set it.
        unsafe { std::env::set_var("PORTKEY_PROVIDER_AWS-BEDROCK", "bedrock") };
        assert_eq!(
            portkey_slug("aws-bedrock", "https://example.invalid/v1"),
            "bedrock"
        );
        unsafe { std::env::remove_var("PORTKEY_PROVIDER_AWS-BEDROCK") };
    }

    #[test]
    fn an_empty_override_does_not_shadow_the_host_mapping() {
        // A label of its own: env vars are process-global, so two tests sharing
        // one race when the suite runs in parallel.
        unsafe { std::env::set_var("PORTKEY_PROVIDER_SPARE_BEDROCK", "") };
        assert_eq!(
            portkey_slug(
                "spare-bedrock",
                "https://bedrock-runtime.us-east-1.amazonaws.com"
            ),
            "bedrock"
        );
        unsafe { std::env::remove_var("PORTKEY_PROVIDER_SPARE_BEDROCK") };
    }

    #[test]
    fn openrouter_converts_per_token_usd_strings_to_usd_per_1m() {
        // OpenRouter's own wire shape: decimal-string USD per token, not cents.
        let pricing = json!({"prompt": "0.00000015", "completion": "0.0000006"});
        let p = openrouter_prices_from_pricing(&pricing).unwrap();
        assert_eq!(p.input_per_1m, 0.15);
        assert_eq!(p.output_per_1m, 0.6);
        assert_eq!(p.cache_creation_per_1m, None);
        assert_eq!(p.cache_read_per_1m, None);
    }

    #[test]
    fn openrouter_free_model_prices_as_zero() {
        let pricing = json!({"prompt": "0", "completion": "0"});
        let p = openrouter_prices_from_pricing(&pricing).unwrap();
        assert_eq!(p.input_per_1m, 0.0);
        assert_eq!(p.output_per_1m, 0.0);
    }

    #[test]
    fn openrouter_variable_pricing_sentinel_is_excluded() {
        // OpenRouter's own meta/auto-routers report "-1" for "not a fixed rate".
        let pricing = json!({"prompt": "-1", "completion": "-1"});
        assert!(openrouter_prices_from_pricing(&pricing).is_none());
    }

    #[test]
    fn openrouter_missing_or_unparseable_price_skips_the_model() {
        assert!(openrouter_prices_from_pricing(&json!({"completion": "0.001"})).is_none());
        assert!(
            openrouter_prices_from_pricing(&json!({"prompt": "abc", "completion": "0.001"}))
                .is_none()
        );
    }

    #[tokio::test]
    async fn fetch_openrouter_catalog_parses_models_and_skips_bad_entries() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "data": [
                        {"id": "openai/gpt-4o-mini", "pricing": {"prompt": "0.00000015", "completion": "0.0000006"}},
                        {"id": "openrouter/auto", "pricing": {"prompt": "-1", "completion": "-1"}},
                        {"id": "vendor/no-pricing"}
                    ]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let http = reqwest::Client::new();
        let book = fetch_openrouter_catalog(&http, &format!("{}/models", server.url()))
            .await
            .unwrap();
        assert_eq!(book.len(), 1);
        assert_eq!(book["openai/gpt-4o-mini"].input_per_1m, 0.15);
    }

    #[tokio::test]
    async fn fetch_openrouter_catalog_returns_none_on_http_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/models")
            .with_status(500)
            .create_async()
            .await;
        let http = reqwest::Client::new();
        assert!(
            fetch_openrouter_catalog(&http, &format!("{}/models", server.url()))
                .await
                .is_none()
        );
    }
}
