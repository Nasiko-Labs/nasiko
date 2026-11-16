//! Provider model catalog — the set of models each provider's configured endpoint
//! actually serves, discovered live from its `GET /models` listing endpoint and synced
//! into the `provider_models` table.
//!
//! This is the foundation of tier routing: the tier registry
//! ([`super::registry::PgTierRegistry`]) derives tier→model mappings from this catalog
//! (ranked by price as the strength signal) instead of any hardcoded model list, so
//! the router only ever routes among models the upstream supports — a custom
//! OpenAI-compatible endpoint (DeepSeek, vLLM, Ollama, …) is routed among *its own*
//! models with zero configuration.
//!
//! Sync semantics: for each provider with a platform API key configured, fetch the
//! model list, upsert every listed model, and delete rows the provider no longer
//! lists (the table is catalog-owned; operator tier overrides live in
//! `model_registry`). A failed fetch leaves existing rows untouched — stale data
//! beats no data.
//!
//! Gemini is listed through its own `ListModels` shape ([`fetch_gemini_models`]):
//! `{"models": [{"name": "models/…"}], "nextPageToken": …}` with an
//! `x-goog-api-key` header. Unknown/absent catalogs degrade to no tier routing (the
//! request's own model passes through), never to a wrong model.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use nasiko_secrets::SecretsCrypto;
use sqlx::PgPool;

use crate::config::GatewayConfig;
use crate::providers::ProviderDialect;

/// A `custom_providers` row as the catalog + pricing sweeps need it: an endpoint
/// registered in the DB rather than via env config, plus the dialect it speaks. The
/// api key is already decrypted (platform-settings scope).
#[derive(Debug, Clone)]
pub(crate) struct CustomProviderEntry {
    pub label: String,
    pub base_url: String,
    pub dialect: ProviderDialect,
    pub api_key: String,
    pub default_model: Option<String>,
    pub catalog_sync_enabled: bool,
}

/// The raw `custom_providers` columns the sweeps read.
type CustomProviderRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    bool,
);

fn row_to_entry(row: CustomProviderRow) -> Option<CustomProviderEntry> {
    let (
        label,
        base_url,
        kind,
        api_version,
        encrypted_api_key,
        default_model,
        catalog_sync_enabled,
    ) = row;
    match SecretsCrypto::for_platform_settings().decrypt(&encrypted_api_key) {
        Ok(api_key) => Some(CustomProviderEntry {
            label,
            base_url,
            dialect: ProviderDialect::from_kind(&kind, api_version.as_deref()),
            api_key,
            default_model,
            catalog_sync_enabled,
        }),
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::catalog",
                label = %label, error = %e,
                "custom provider key decrypt failed — skipping this provider in the sweep"
            );
            None
        }
    }
}

/// Load every active custom provider (keys decrypted). Fail-soft: a DB error yields
/// an empty list so the built-in sweep still runs.
pub(crate) async fn load_custom_providers(db: &PgPool) -> Vec<CustomProviderEntry> {
    let rows: Vec<CustomProviderRow> = match sqlx::query_as(
        "SELECT label, base_url, kind, api_version, encrypted_api_key, default_model, \
                catalog_sync_enabled \
         FROM custom_providers WHERE deleted_at IS NULL",
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                target: "nasiko::llm_router::catalog",
                error = %e, "failed to load custom providers — sweeping built-ins only"
            );
            return Vec::new();
        }
    };
    rows.into_iter().filter_map(row_to_entry).collect()
}

/// Load one active custom provider by label (key decrypted).
async fn load_custom_provider(
    db: &PgPool,
    label: &str,
) -> Result<Option<CustomProviderEntry>, sqlx::Error> {
    let row: Option<CustomProviderRow> = sqlx::query_as(
        "SELECT label, base_url, kind, api_version, encrypted_api_key, default_model, \
                catalog_sync_enabled \
         FROM custom_providers WHERE label = $1 AND deleted_at IS NULL",
    )
    .bind(label)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(row_to_entry))
}

/// Upper bound on a `/models` fetch so a slow provider can't stall the sync loop.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// `tokio::time::interval` panics on a zero period, and
/// `MODEL_CATALOG_SYNC_INTERVAL_SECS` is operator-supplied, so floor it rather than
/// trusting the input.
const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Every provider label the router can actually route to, as `(label, API base URL)`,
/// gated on a configured platform key. Superset of [`listable_providers`]: the pricing
/// sync only needs the label and base URL (it queries Portkey, never the provider), so
/// it covers Gemini too — whereas the catalog sync can't, because Gemini's `ListModels`
/// answers `{"models": [{"name": …}]}` with a `?key=` credential rather than the
/// `{"data": [{"id": …}]}` + bearer shape [`fetch_models`] speaks.
pub(crate) fn priceable_providers(
    cfg: &GatewayConfig,
    custom: &[CustomProviderEntry],
) -> Vec<(String, String)> {
    // Built-in listable providers only (no custom) — custom entries are appended
    // below regardless of their catalog-sync toggle, since pricing is independent of
    // model listing (a private gateway usually has no Portkey price book anyway,
    // which is expected and harmless).
    let mut out: Vec<(String, String)> = listable_providers(cfg, &[])
        .into_iter()
        .map(|(label, base, _key, _dialect)| (label, base))
        .collect();
    if !cfg.platform_gemini_api_key.is_empty() {
        out.push(("gemini".to_string(), cfg.gemini_api_base.clone()));
    }
    for c in custom {
        out.push((c.label.clone(), c.base_url.clone()));
    }
    out
}

/// The providers we know how to list models for: `(provider label, API base URL, key)`
/// resolved from the gateway config. Providers without a platform key are skipped —
/// no key means the router can't call that provider anyway.
///
/// Gemini is deliberately absent — see [`priceable_providers`] for why.
///
/// `custom` carries DB-registered providers (resolved by the caller so this stays
/// pure and its unit tests need no database); each one with `catalog_sync_enabled`
/// is appended as an OpenAI-compatible endpoint.
pub(crate) fn listable_providers(
    cfg: &GatewayConfig,
    custom: &[CustomProviderEntry],
) -> Vec<(String, String, String, ProviderDialect)> {
    let mut out = Vec::new();
    if !cfg.platform_openai_api_key.is_empty() {
        // Any OpenAI-compatible endpoint (OpenAI, DeepSeek, vLLM, …) shares this shape.
        out.push((
            "openai".to_string(),
            cfg.openai_api_base.clone(),
            cfg.platform_openai_api_key.clone(),
            ProviderDialect::OpenAi,
        ));
    }
    if !cfg.platform_anthropic_api_key.is_empty() {
        out.push((
            "anthropic".to_string(),
            cfg.anthropic_api_base.clone(),
            cfg.platform_anthropic_api_key.clone(),
            // Anthropic's listing is OpenAI-shaped apart from its credential header,
            // which `fetch_models` applies by provider name.
            ProviderDialect::OpenAi,
        ));
    }
    for c in custom {
        if c.catalog_sync_enabled {
            out.push((
                c.label.clone(),
                c.base_url.clone(),
                c.api_key.clone(),
                c.dialect.clone(),
            ));
        }
    }
    out
}

/// Both the OpenAI-compatible and Anthropic list endpoints answer
/// `{"data": [{"id": "<model>", …}, …]}`.
fn parse_models_response(body: &serde_json::Value) -> HashSet<String> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Name fragments of Gemini models that list `generateContent` but are not chat
/// models (speech, image generation, embeddings, live audio, computer use).
const GEMINI_NON_CHAT_MARKERS: &[&str] = &[
    "-tts",
    "-image",
    "imagen",
    "embedding",
    "-live",
    "native-audio",
    "computer-use",
    "aqa",
];

/// One page of Gemini `ListModels`:
/// `{"models": [{"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent", …]}], "nextPageToken": "…"}`.
/// Keeps chat-capable Gemini models only (`generateContent`, `gemini-` prefix, no
/// non-chat marker) with the `models/` prefix stripped. Returns the next page token.
fn parse_gemini_models(body: &serde_json::Value) -> (HashSet<String>, Option<String>) {
    let models: HashSet<String> = body
        .get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| {
                    m.get("supportedGenerationMethods")
                        .and_then(|v| v.as_array())
                        .is_some_and(|methods| {
                            methods
                                .iter()
                                .any(|x| x.as_str() == Some("generateContent"))
                        })
                })
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .map(|name| name.strip_prefix("models/").unwrap_or(name))
                .filter(|name| {
                    name.starts_with("gemini-")
                        && !GEMINI_NON_CHAT_MARKERS
                            .iter()
                            .any(|mark| name.contains(mark))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let next: Option<String> = body
        .get("nextPageToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    (models, next)
}

/// Upper bound on `ListModels` pages so a looping token can't stall the sync.
const GEMINI_MAX_PAGES: usize = 20;

/// Fetch every page of Gemini `ListModels` with `api_key`. `None` on any failure —
/// callers keep existing rows. `pub` so the server can list models for a user's own
/// key (BYOK on planes without a platform Gemini key).
pub async fn fetch_gemini_models(
    http: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Option<HashSet<String>> {
    let url = format!("{}/models", base.trim_end_matches('/'));
    let mut all: HashSet<String> = HashSet::new();
    let mut page_token: Option<String> = None;
    for _ in 0..GEMINI_MAX_PAGES {
        let mut req = http
            .get(&url)
            .timeout(FETCH_TIMEOUT)
            .header("x-goog-api-key", api_key)
            .query(&[("pageSize", "1000")]);
        if let Some(token) = &page_token {
            req = req.query(&[("pageToken", token.as_str())]);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::catalog",
                    provider = "gemini", error = %e,
                    "model catalog sync: ListModels fetch failed — keeping existing rows"
                );
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(
                target: "nasiko::llm_router::catalog",
                provider = "gemini", status = %resp.status(),
                "model catalog sync: ListModels returned non-success — keeping existing rows"
            );
            return None;
        }
        let body: serde_json::Value = resp.json().await.ok()?;
        let (models, next) = parse_gemini_models(&body);
        all.extend(models);
        match next {
            Some(token) => page_token = Some(token),
            None => return Some(all),
        }
    }
    Some(all)
}

/// Parse the Bedrock control-plane `ListFoundationModels` response:
/// `{"modelSummaries": [{"modelId": "…", "inferenceAPIsSupported": {"converse": {"sync": true}}, …}]}`.
/// Only models that support the Converse API and are ACTIVE are included.
fn parse_bedrock_foundation_models(body: &serde_json::Value) -> HashSet<String> {
    body.get("modelSummaries")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|m| {
                    // Only ACTIVE models.
                    let active = m.pointer("/modelLifecycle/status").and_then(|s| s.as_str())
                        == Some("ACTIVE");
                    // Only models that support the Converse API.
                    let converse_sync = m
                        .pointer("/inferenceAPIsSupported/converse/sync")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let converse_stream = m
                        .pointer("/inferenceAPIsSupported/converse/streaming")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    active && (converse_sync || converse_stream)
                })
                .filter_map(|m| {
                    m.get("modelId")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fetch one provider's model list. `None` on any failure — callers leave existing
/// rows untouched.
async fn fetch_models(
    http: &reqwest::Client,
    provider: &str,
    url: &str,
    api_key: &str,
    dialect: &ProviderDialect,
) -> Option<HashSet<String>> {
    let req = http.get(url).timeout(FETCH_TIMEOUT);
    let req = match provider {
        // Anthropic is a built-in whose listing takes its own credential headers.
        "anthropic" => req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        _ => dialect.authorize(req, api_key),
    };
    let resp = req
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::catalog",
                provider = %provider, error = %e,
                "model catalog sync: /models fetch failed — keeping existing rows"
            );
            e
        })
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(
            target: "nasiko::llm_router::catalog",
            provider = %provider, status = %resp.status(),
            "model catalog sync: /models returned non-success — keeping existing rows"
        );
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    // Bedrock's control-plane API has a different response shape from the
    // OpenAI-compatible `/models` endpoint the other dialects use.
    let models = match dialect {
        ProviderDialect::BedrockConverse => parse_bedrock_foundation_models(&body),
        _ => parse_models_response(&body),
    };
    Some(models)
}

/// Replace one provider's catalog rows with `models` (upsert + delete-stale).
async fn sync_provider(
    db: &PgPool,
    provider: &str,
    models: &HashSet<String>,
) -> Result<(), sqlx::Error> {
    let models: Vec<&str> = models.iter().map(String::as_str).collect();
    sqlx::query(
        "INSERT INTO provider_models (provider, model, last_seen_at) \
         SELECT $1, m, now() FROM unnest($2::text[]) AS m \
         ON CONFLICT (provider, model) DO UPDATE SET last_seen_at = now()",
    )
    .bind(provider)
    .bind(&models)
    .execute(db)
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM provider_models WHERE provider = $1 AND NOT (model = ANY($2::text[]))",
    )
    .bind(provider)
    .bind(&models)
    .execute(db)
    .await?
    .rows_affected();
    tracing::info!(
        target: "nasiko::llm_router::catalog",
        provider = %provider, listed = models.len(), stale_deleted = deleted,
        "model catalog sync: provider catalog updated"
    );
    Ok(())
}

/// Why a provider's catalog was not refreshed. `Unavailable`/`NoModels` both keep
/// existing rows (stale data beats no data); `Db` means the fetch worked but the
/// write failed.
enum SyncSkip {
    /// The `/models` fetch failed (network/HTTP/parse) — already logged by [`fetch_models`].
    Unavailable,
    /// The endpoint answered but listed nothing usable (a non-OpenAI `/models` shape).
    NoModels,
    /// The catalog fetched but the DB write failed.
    Db(sqlx::Error),
}

/// Fetch one provider's `/models` and replace its catalog rows. On success returns
/// `(count, models)` so the caller can validate a default model against the fresh set.
async fn fetch_and_sync(
    db: &PgPool,
    http: &reqwest::Client,
    provider: &str,
    base: &str,
    key: &str,
    dialect: &ProviderDialect,
) -> Result<(usize, HashSet<String>), SyncSkip> {
    let url = dialect.models_url(base);
    let Some(models) = fetch_models(http, provider, &url, key, dialect).await else {
        return Err(SyncSkip::Unavailable);
    };
    if models.is_empty() {
        return Err(SyncSkip::NoModels);
    }
    sync_provider(db, provider, &models)
        .await
        .map_err(SyncSkip::Db)?;
    Ok((models.len(), models))
}

/// Record a custom provider's last-sync health (`custom_providers.last_sync_*`) so the
/// UI can show whether it is healthy and when it was last checked. Fail-soft.
async fn record_sync_status(db: &PgPool, label: &str, status: &str, error: Option<&str>) {
    if let Err(e) = sqlx::query(
        "UPDATE custom_providers \
         SET last_sync_at = now(), last_sync_status = $2, last_sync_error = $3 \
         WHERE label = $1 AND deleted_at IS NULL",
    )
    .bind(label)
    .bind(status)
    .bind(error)
    .execute(db)
    .await
    {
        tracing::warn!(
            target: "nasiko::llm_router::catalog",
            label = %label, error = %e, "failed to record custom provider sync status"
        );
    }
}

/// The `last_sync_error` warning to flag when a provider's `default_model` is no
/// longer in its live catalog — the last-resort fallback would point at a dead model.
/// We warn loudly and surface it in the UI, but never silently repoint an operator's
/// choice (that is worse than telling them). `None` when the default is still served.
fn default_model_error(entry: &CustomProviderEntry, models: &HashSet<String>) -> Option<String> {
    let Some(ref default_model) = entry.default_model else {
        return None; // No default model set — nothing to warn about.
    };
    if models.contains(default_model) {
        return None;
    }
    tracing::warn!(
        target: "nasiko::llm_router::catalog",
        label = %entry.label, default_model = %default_model,
        "custom provider default_model is no longer served by its endpoint — the \
         last-resort fallback will fail; update it"
    );
    Some(format!(
        "default model '{default_model}' is no longer served by this endpoint",
    ))
}

/// Sync one custom provider's catalog and record its health. Returns the number of
/// models discovered (0 for a failed/unsupported listing, both of which keep existing
/// rows). Errors only when the catalog fetched but the DB write failed.
async fn sync_and_record(
    db: &PgPool,
    http: &reqwest::Client,
    entry: &CustomProviderEntry,
) -> Result<usize, sqlx::Error> {
    match fetch_and_sync(
        db,
        http,
        &entry.label,
        &entry.base_url,
        &entry.api_key,
        &entry.dialect,
    )
    .await
    {
        Ok((n, models)) => {
            let default_error = default_model_error(entry, &models);
            record_sync_status(db, &entry.label, "ok", default_error.as_deref()).await;
            Ok(n)
        }
        Err(SyncSkip::Unavailable) => {
            record_sync_status(
                db,
                &entry.label,
                "failed",
                Some("model listing fetch failed"),
            )
            .await;
            Ok(0)
        }
        Err(SyncSkip::NoModels) => {
            record_sync_status(
                db,
                &entry.label,
                "unsupported",
                Some("endpoint returned no usable models; enter model names manually"),
            )
            .await;
            Ok(0)
        }
        Err(SyncSkip::Db(e)) => Err(e),
    }
}

/// Refresh a single custom provider's model catalog on demand (registration and the
/// "Sync now" button). Returns the number of models discovered — 0 when the listing
/// failed or is unsupported (the manual-entry case). An unknown label is `Ok(0)`.
pub async fn sync_one(
    db: &PgPool,
    http: &reqwest::Client,
    label: &str,
) -> Result<usize, sqlx::Error> {
    match load_custom_provider(db, label).await? {
        Some(entry) => sync_and_record(db, http, &entry).await,
        None => Ok(0),
    }
}

/// One sync pass over every listable provider (built-in + DB-registered custom).
/// Returns the number of providers successfully synced.
pub async fn sync_once(db: &PgPool, http: &reqwest::Client, cfg: &GatewayConfig) -> usize {
    let mut synced = 0;
    // Built-in providers: no last_sync bookkeeping (their health is env config).
    for (provider, base, key, dialect) in listable_providers(cfg, &[]) {
        match fetch_and_sync(db, http, &provider, &base, &key, &dialect).await {
            Ok(_) => synced += 1,
            Err(SyncSkip::Unavailable) => {} // already logged by fetch_models
            Err(SyncSkip::NoModels) => tracing::warn!(
                target: "nasiko::llm_router::catalog",
                provider = %provider,
                "model catalog sync: provider listed zero models — keeping existing rows"
            ),
            Err(SyncSkip::Db(e)) => tracing::warn!(
                target: "nasiko::llm_router::catalog",
                provider = %provider, error = %e, "model catalog sync: DB write failed"
            ),
        }
    }
    // Gemini: its own ListModels shape and credential header.
    if !cfg.platform_gemini_api_key.is_empty() {
        match fetch_gemini_models(http, &cfg.gemini_api_base, &cfg.platform_gemini_api_key).await {
            Some(models) if !models.is_empty() => {
                match sync_provider(db, "gemini", &models).await {
                    Ok(()) => synced += 1,
                    Err(e) => tracing::warn!(
                        target: "nasiko::llm_router::catalog",
                        provider = "gemini", error = %e, "model catalog sync: DB write failed"
                    ),
                }
            }
            Some(_) => tracing::warn!(
                target: "nasiko::llm_router::catalog",
                provider = "gemini",
                "model catalog sync: provider listed zero models — keeping existing rows"
            ),
            None => {} // already logged by fetch_gemini_models
        }
    }
    // Custom providers: sync + record health + default_model check.
    for c in load_custom_providers(db).await {
        if !c.catalog_sync_enabled {
            continue;
        }
        match sync_and_record(db, http, &c).await {
            Ok(n) if n > 0 => synced += 1,
            Ok(_) => {} // failed/unsupported outcome already recorded on the row
            Err(e) => tracing::warn!(
                target: "nasiko::llm_router::catalog",
                label = %c.label, error = %e, "model catalog sync: custom provider DB write failed"
            ),
        }
    }
    synced
}

/// Spawn the background catalog-sync loop: an immediate sync at startup, then every
/// `MODEL_CATALOG_SYNC_INTERVAL_SECS` (default 24 h, floored at [`MIN_INTERVAL`]). The
/// task logs and continues on failure; it never panics and never blocks serving.
///
/// Takes the already-resolved [`GatewayConfig`] rather than re-reading the environment,
/// so the router's effective config is decided once at the composition root. Whether to
/// spawn at all is the caller's decision (`MODEL_CATALOG_SYNC_ENABLED`) — this loop
/// reaches the network on its first tick.
pub fn spawn_sync(db: PgPool, http: reqwest::Client, cfg: Arc<GatewayConfig>) {
    let interval = Duration::from_secs(cfg.model_catalog_sync_interval_secs).max(MIN_INTERVAL);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        // A pass that overruns the period must not then fire back-to-back.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // interval's first tick completes immediately → sync at startup.
            tick.tick().await;
            let synced = sync_once(&db, &http, &cfg).await;
            tracing::info!(
                target: "nasiko::llm_router::catalog",
                providers_synced = synced, interval_secs = interval.as_secs(),
                "model catalog sync pass complete"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_and_anthropic_list_shape() {
        let body = serde_json::json!({
            "data": [
                {"id": "deepseek-v4-pro", "object": "model"},
                {"id": "deepseek-v4-flash", "object": "model"}
            ]
        });
        let models = parse_models_response(&body);
        assert!(models.contains("deepseek-v4-pro"));
        assert!(models.contains("deepseek-v4-flash"));
        assert!(!models.contains("gpt-5.4"));
    }

    #[test]
    fn parses_gemini_list_models_keeping_chat_models_only() {
        let body = serde_json::json!({
            "models": [
                {"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-4-argon", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-2.5-flash-preview-tts", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-2.5-flash-image", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"]},
                {"name": "models/gemma-4-26b-a4b-it", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-x", "supportedGenerationMethods": ["countTokens"]}
            ],
            "nextPageToken": "abc"
        });
        let (models, next) = parse_gemini_models(&body);
        let mut got: Vec<String> = models.into_iter().collect();
        got.sort();
        assert_eq!(got, vec!["gemini-2.5-flash", "gemini-4-argon"]);
        assert_eq!(next.as_deref(), Some("abc"));
    }

    #[test]
    fn gemini_last_page_has_no_token() {
        let (models, next) =
            parse_gemini_models(&serde_json::json!({"models": [], "nextPageToken": ""}));
        assert!(models.is_empty());
        assert!(next.is_none());
    }

    #[test]
    fn parse_garbage_yields_empty_set() {
        assert!(parse_models_response(&serde_json::json!({"nope": 1})).is_empty());
        assert!(parse_models_response(&serde_json::json!(null)).is_empty());
    }

    fn entry(label: &str, sync_enabled: bool) -> CustomProviderEntry {
        CustomProviderEntry {
            label: label.into(),
            base_url: format!("https://{label}.internal/v1"),
            dialect: ProviderDialect::OpenAi,
            api_key: format!("sk-{label}"),
            default_model: Some("m".into()),
            catalog_sync_enabled: sync_enabled,
        }
    }

    #[test]
    fn listable_providers_require_platform_keys() {
        let cfg = GatewayConfig::default();
        assert!(listable_providers(&cfg, &[]).is_empty());

        let cfg = GatewayConfig {
            platform_openai_api_key: "sk-test".into(),
            openai_api_base: "https://api.deepseek.com/v1".into(),
            ..GatewayConfig::default()
        };
        let providers = listable_providers(&cfg, &[]);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].0, "openai");
        assert_eq!(providers[0].1, "https://api.deepseek.com/v1");
    }

    #[test]
    fn listable_providers_includes_only_sync_enabled_custom_rows() {
        let cfg = GatewayConfig::default(); // no built-in keys
        let custom = vec![entry("my-gateway", true), entry("paused-gw", false)];
        let providers = listable_providers(&cfg, &custom);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].0, "my-gateway");
        assert_eq!(providers[0].1, "https://my-gateway.internal/v1");
        assert_eq!(providers[0].2, "sk-my-gateway");
    }

    #[test]
    fn priceable_providers_add_gemini_but_listable_does_not() {
        let cfg = GatewayConfig {
            platform_gemini_api_key: "gk-test".into(),
            ..GatewayConfig::default()
        };
        // Gemini is priceable (Portkey knows it) but not listable (its /models
        // shape and credential differ from what `fetch_models` speaks).
        assert!(listable_providers(&cfg, &[]).is_empty());
        let priceable = priceable_providers(&cfg, &[]);
        assert_eq!(priceable.len(), 1);
        assert_eq!(priceable[0].0, "gemini");
        assert_eq!(priceable[0].1, cfg.gemini_api_base);
    }

    #[test]
    fn priceable_providers_is_empty_without_keys() {
        assert!(priceable_providers(&GatewayConfig::default(), &[]).is_empty());
    }

    #[test]
    fn priceable_providers_includes_custom_regardless_of_sync_toggle() {
        // Pricing is independent of the catalog-sync toggle, so even a paused
        // provider is tried against Portkey (usually finds nothing — harmless).
        let cfg = GatewayConfig::default();
        let custom = vec![entry("my-gateway", false)];
        let priceable = priceable_providers(&cfg, &custom);
        assert_eq!(priceable.len(), 1);
        assert_eq!(priceable[0].0, "my-gateway");
    }

    #[test]
    fn priceable_providers_covers_every_configured_label() {
        let cfg = GatewayConfig {
            platform_openai_api_key: "sk-test".into(),
            platform_anthropic_api_key: "ak-test".into(),
            platform_gemini_api_key: "gk-test".into(),
            ..GatewayConfig::default()
        };
        let labels: Vec<String> = priceable_providers(&cfg, &[])
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert_eq!(labels, vec!["openai", "anthropic", "gemini"]);
    }

    #[test]
    fn default_model_error_flags_missing_only() {
        let e = entry("my-gateway", true);
        let mut models = HashSet::new();
        models.insert("other-model".to_string());
        assert!(default_model_error(&e, &models).is_some());
        models.insert("m".to_string());
        assert!(default_model_error(&e, &models).is_none());
    }

    #[test]
    fn parses_bedrock_foundation_models_response() {
        let body = serde_json::json!({
            "modelSummaries": [
                {
                    "modelId": "openai.gpt-6-astra",
                    "modelLifecycle": { "status": "ACTIVE" },
                    "inferenceAPIsSupported": { "converse": { "sync": true, "streaming": true } }
                },
                {
                    "modelId": "deepseek.v3.2",
                    "modelLifecycle": { "status": "ACTIVE" },
                    "inferenceAPIsSupported": { "converse": { "sync": true, "streaming": true } }
                },
                {
                    "modelId": "stability.sd3-5-large-v1:0",
                    "modelLifecycle": { "status": "ACTIVE" },
                    "inferenceAPIsSupported": { "converse": { "sync": false, "streaming": false } }
                },
                {
                    "modelId": "old.deprecated-model",
                    "modelLifecycle": { "status": "LEGACY" },
                    "inferenceAPIsSupported": { "converse": { "sync": true, "streaming": true } }
                }
            ]
        });
        let models = parse_bedrock_foundation_models(&body);
        assert_eq!(models.len(), 2);
        assert!(models.contains("openai.gpt-6-astra"));
        assert!(models.contains("deepseek.v3.2"));
        // Image model (no converse) and legacy model are excluded.
        assert!(!models.contains("stability.sd3-5-large-v1:0"));
        assert!(!models.contains("old.deprecated-model"));
    }

    #[test]
    fn parse_bedrock_garbage_yields_empty_set() {
        assert!(parse_bedrock_foundation_models(&serde_json::json!({"nope": 1})).is_empty());
        assert!(parse_bedrock_foundation_models(&serde_json::json!(null)).is_empty());
    }
}
