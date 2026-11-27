//! Admin API for custom LLM providers (`custom_providers` table).
//!
//! An admin registers an endpoint by Base URL + API key, plus the wire dialect it
//! speaks (`kind`: plain OpenAI-compatible, or Azure OpenAI). On create the server
//! fetches its model list into `provider_models` (reusing the LLM router's catalog
//! sync — no second fetcher), and the background catalog-sync loop keeps it fresh. The
//! discovered models then flow into the LLM config screen exactly like the built-in
//! providers'. The endpoint can be chat-tested before registering via
//! `POST /custom-providers/test`, which stores nothing.
//!
//! Every URL and credential header here comes from
//! [`ProviderDialect`](nasiko_llm_router::providers::ProviderDialect), the same seam
//! the dispatch path uses — so what the probe reaches at registration is exactly what
//! a call will reach later.
//!
//! - `GET    /api/custom-providers`            — list (key masked). Any authenticated user.
//! - `GET    /api/custom-providers/{id}/models`— discovered models for the provider.
//! - `POST   /api/custom-providers`            — register (superuser).
//! - `POST   /api/custom-providers/test`       — probe an endpoint without storing (superuser).
//! - `PATCH  /api/custom-providers/{id}`       — update (superuser).
//! - `DELETE /api/custom-providers/{id}`       — soft delete, blocked if referenced (superuser).
//! - `POST   /api/custom-providers/{id}/sync`  — refresh the model list now (superuser).
//!
//! Encryption uses `SecretsCrypto::for_platform_settings()` — the same scope the LLM
//! router's resolver decrypts with — so a key stored here is readable on the dispatch
//! path with no key handoff.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use nasiko_llm_router::providers::{
    KIND_AZURE_OPENAI, KIND_BEDROCK_CONVERSE, KIND_OPENAI, ProviderDialect,
};
use nasiko_secrets::SecretsCrypto;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::Claims;
use crate::auth::rbac::require_superuser;
use crate::mcp::ApiResponse;
use crate::state::AppState;

/// Built-in provider labels a custom provider may never shadow — they route through
/// dedicated clients, so a custom row under one of these names would be unreachable
/// and would collide with the built-in dispatch path.
const RESERVED_LABELS: &[&str] = &["openai", "anthropic", "gemini"];

/// Upper bound on the chat-test probe so a slow/hung endpoint can't stall the request.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

pub fn router() -> Router<AppState> {
    // Mutations are superuser-only (platform-wide config), matching model_registry::router().
    let write = Router::new()
        .route("/custom-providers", post(create))
        .route("/custom-providers/test", post(test_endpoint))
        .route(
            "/custom-providers/{id}",
            axum::routing::patch(update).delete(delete_provider),
        )
        .route("/custom-providers/{id}/sync", post(sync_now))
        .layer(middleware::from_fn(require_superuser));

    Router::new()
        .route("/custom-providers", get(list))
        .route("/custom-providers/{id}/models", get(list_models))
        .merge(write)
}

/// A provider row as returned to clients — never carries the api key.
#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub(crate) struct ProviderView {
    pub id: Uuid,
    pub label: String,
    pub display_name: String,
    pub base_url: String,
    /// Wire dialect: `openai` or `azure-openai`.
    pub kind: String,
    /// Azure `api-version`; `None` for plain OpenAI-compatible endpoints.
    pub api_version: Option<String>,
    pub default_model: Option<String>,
    pub catalog_sync_enabled: bool,
    /// Whether an encrypted key is stored (the key itself is never returned).
    pub api_key_set: bool,
    pub last_sync_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_sync_status: Option<String>,
    pub last_sync_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

const VIEW_COLS: &str = "id, label, display_name, base_url, kind, api_version, default_model, \
     catalog_sync_enabled, (encrypted_api_key <> '') AS api_key_set, \
     last_sync_at, last_sync_status, last_sync_error, created_at";

#[derive(Deserialize, ToSchema)]
pub(crate) struct CreateRequest {
    pub display_name: String,
    pub base_url: String,
    /// Wire dialect: `openai` (default) or `azure-openai`.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Azure `api-version` (e.g. `2024-10-21`). Required when `kind` is
    /// `azure-openai`, ignored otherwise.
    #[serde(default)]
    pub api_version: Option<String>,
    pub api_key: String,
    #[serde(default = "default_true")]
    pub catalog_sync_enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_kind() -> String {
    KIND_OPENAI.to_string()
}

/// The `api_version` column value for a dialect — `None` for anything but Azure, so
/// the column stays null wherever it is meaningless.
fn azure_api_version(dialect: &ProviderDialect) -> Option<&str> {
    match dialect {
        ProviderDialect::AzureOpenAi { api_version } => Some(api_version),
        ProviderDialect::OpenAi | ProviderDialect::BedrockConverse => None,
    }
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct UpdateRequest {
    pub display_name: Option<String>,
    pub base_url: Option<String>,
    /// Azure `api-version`. The dialect (`kind`) itself is immutable — switching it
    /// would silently repoint every config on this label at a different URL shape.
    pub api_version: Option<String>,
    /// A new key rotates the stored credential; omitted ⇒ the existing key is kept.
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub catalog_sync_enabled: Option<bool>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn internal(context: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!(%e, "custom_providers: {context}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

/// Auto-generate an internal label (slug) from a display name. The label is the
/// join key across `provider_models`, `model_registry`, `token_usage`, etc. — it
/// must be lowercase alphanumeric with internal hyphens, 2–40 chars.
fn slugify(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse runs of hyphens.
    let mut collapsed = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push(c);
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    // Trim leading/trailing hyphens and truncate to 40 chars.
    let trimmed = collapsed.trim_matches('-');
    let truncated = if trimmed.len() > 40 {
        &trimmed[..40]
    } else {
        trimmed
    };
    let truncated = truncated.trim_end_matches('-');
    if truncated.len() < 2 {
        // Fallback for very short or all-special-char names.
        format!("custom-{}", &Uuid::new_v4().to_string()[..8])
    } else {
        truncated.to_string()
    }
}

/// The dialect for a create/test request, or a client-facing reason why not. An
/// unknown kind and a missing Azure `api-version` are both 400s here rather than a
/// constraint violation surfacing as a 500 from the insert.
fn validate_dialect(kind: &str, api_version: Option<&str>) -> Result<ProviderDialect, String> {
    let api_version = api_version.map(str::trim).filter(|v| !v.is_empty());
    match kind.trim() {
        KIND_OPENAI => Ok(ProviderDialect::OpenAi),
        KIND_AZURE_OPENAI => match api_version {
            Some(v) => Ok(ProviderDialect::AzureOpenAi {
                api_version: v.to_string(),
            }),
            None => Err(format!(
                "api_version is required for kind '{KIND_AZURE_OPENAI}' (e.g. 2024-10-21)"
            )),
        },
        KIND_BEDROCK_CONVERSE => Ok(ProviderDialect::BedrockConverse),
        other => Err(format!(
            "unknown kind '{other}' (expected '{KIND_OPENAI}', '{KIND_AZURE_OPENAI}', or '{KIND_BEDROCK_CONVERSE}')"
        )),
    }
}

#[derive(Deserialize, ToSchema)]
pub(crate) struct TestRequest {
    pub base_url: String,
    pub api_key: String,
    /// Wire dialect: `openai` (default) or `azure-openai`.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Azure `api-version`; required when `kind` is `azure-openai`.
    #[serde(default)]
    pub api_version: Option<String>,
    /// Optional model to chat-test. When absent, only the model list is fetched.
    #[serde(default)]
    pub model: Option<String>,
}

/// Test a custom provider endpoint without storing anything. Probes chat (if a model
/// is given) and fetches the model list. Superuser only.
pub(crate) async fn test_endpoint(
    State(state): State<AppState>,
    _claims: Claims,
    Json(body): Json<TestRequest>,
) -> Response {
    let dialect = match validate_dialect(&body.kind, body.api_version.as_deref()) {
        Ok(d) => d,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let base_url = dialect.normalize_base(&body.base_url);
    if base_url.is_empty() || body.api_key.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "base_url and api_key are required");
    }

    // Chat test (optional — only when a model is provided).
    let chat_ok = if let Some(ref model) = body.model {
        match probe_chat(
            &state.http_client,
            &dialect,
            &base_url,
            &body.api_key,
            model,
        )
        .await
        {
            Ok(()) => true,
            Err(reason) => {
                return ApiResponse::ok(
                    json!({ "chat_ok": false, "chat_error": reason, "models": [] }),
                    "Chat test failed",
                )
                .into_response();
            }
        }
    } else {
        false // not tested
    };

    // Fetch model list.
    let models = fetch_model_list(&state.http_client, &dialect, &base_url, &body.api_key).await;

    ApiResponse::ok(
        json!({ "chat_ok": chat_ok, "models": models }),
        "Endpoint test complete",
    )
    .into_response()
}

/// Fetch the model list from the dialect's listing endpoint.
async fn fetch_model_list(
    http: &reqwest::Client,
    dialect: &ProviderDialect,
    base_url: &str,
    api_key: &str,
) -> Vec<String> {
    let url = dialect.models_url(base_url);
    let resp = match dialect
        .authorize(http.get(&url), api_key)
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match dialect {
        // Bedrock control-plane: {"modelSummaries": [{"modelId": …, …}]}
        // Only include ACTIVE models that support the Converse API.
        ProviderDialect::BedrockConverse => body
            .get("modelSummaries")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|m| {
                        let active = m.pointer("/modelLifecycle/status").and_then(|s| s.as_str())
                            == Some("ACTIVE");
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
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        // OpenAI-compatible / Azure: {"data": [{"id": …}]}
        _ => body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Chat-test the endpoint with one tiny chat completion. A model listing answering
/// proves nothing about chat, and a later parse error is non-retryable (hard 500), so
/// a failed test is a 400. Returns a client-facing reason on failure.
///
/// No token cap is sent: `max_tokens` is rejected outright by reasoning models, and
/// `max_completion_tokens` by older Azure api-versions, so either one would fail
/// endpoints that chat perfectly well. A single-word prompt is cheap uncapped.
async fn probe_chat(
    http: &reqwest::Client,
    dialect: &ProviderDialect,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<(), String> {
    match dialect {
        ProviderDialect::BedrockConverse => {
            probe_chat_converse(http, base_url, api_key, model).await
        }
        _ => probe_chat_openai(http, dialect, base_url, api_key, model).await,
    }
}

/// Probe an OpenAI-compatible chat endpoint.
async fn probe_chat_openai(
    http: &reqwest::Client,
    dialect: &ProviderDialect,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<(), String> {
    let url = dialect.chat_url(base_url, model);
    let resp = dialect
        .authorize(http.post(&url), api_key)
        .timeout(PROBE_TIMEOUT)
        .json(&json!({
            "model": model,
            "messages": [{ "role": "user", "content": "ping" }],
        }))
        .send()
        .await
        .map_err(|e| format!("chat test request failed: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "chat test returned {status}: {}",
            body_snippet(&body)
        ));
    }
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) if v.get("choices").is_some() => Ok(()),
        Ok(_) => Err("chat test reply did not contain `choices` (not OpenAI-compatible)".into()),
        Err(e) => Err(format!("chat test reply was not valid JSON: {e}")),
    }
}

/// Probe a Bedrock Converse endpoint. Tries the raw model ID first; if Bedrock
/// rejects it (INFERENCE_PROFILE model), retries with the region prefix.
async fn probe_chat_converse(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<(), String> {
    use nasiko_llm_router::providers::bedrock_converse::{converse_model_id, prefixed_model_id};

    let model_id = converse_model_id(model, base_url);
    let converse_body = json!({
        "messages": [{ "role": "user", "content": [{ "text": "ping" }] }],
        "inferenceConfig": { "maxTokens": 20 }
    });

    let url = format!(
        "{}/model/{}/converse",
        base_url.trim_end_matches('/'),
        model_id
    );
    let resp = http
        .post(&url)
        .bearer_auth(api_key)
        .timeout(PROBE_TIMEOUT)
        .json(&converse_body)
        .send()
        .await
        .map_err(|e| format!("chat test request failed: {e}"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // Retry with region prefix for INFERENCE_PROFILE models.
        let prefixed = prefixed_model_id(model, base_url);
        if prefixed != model_id {
            let url2 = format!(
                "{}/model/{}/converse",
                base_url.trim_end_matches('/'),
                prefixed
            );
            let resp2 = http
                .post(&url2)
                .bearer_auth(api_key)
                .timeout(PROBE_TIMEOUT)
                .json(&converse_body)
                .send()
                .await
                .map_err(|e| format!("chat test request failed (prefixed): {e}"))?;
            let status2 = resp2.status();
            let body2 = resp2.text().await.unwrap_or_default();
            if !status2.is_success() {
                return Err(format!(
                    "chat test returned {status2}: {}",
                    body_snippet(&body2)
                ));
            }
            return validate_converse_response(&body2);
        }
        return Err(format!(
            "chat test returned {status}: {}",
            body_snippet(&body)
        ));
    }
    validate_converse_response(&body)
}

fn validate_converse_response(body: &str) -> Result<(), String> {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) if v.pointer("/output/message").is_some() => Ok(()),
        Ok(_) => Err(
            "chat test reply did not contain `output.message` (not Bedrock Converse shape)".into(),
        ),
        Err(e) => Err(format!("chat test reply was not valid JSON: {e}")),
    }
}

/// First 200 chars of an upstream error body, for a client-facing message.
fn body_snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

/// List all active custom providers (keys masked). Any authenticated user, so the
/// LLM config screen can populate its provider selector.
pub(crate) async fn list(State(state): State<AppState>, _claims: Claims) -> Response {
    let rows = sqlx::query_as::<_, ProviderView>(&format!(
        "SELECT {VIEW_COLS} FROM custom_providers WHERE deleted_at IS NULL ORDER BY display_name"
    ))
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(r) => ApiResponse::ok(json!(r), "Custom providers retrieved").into_response(),
        Err(e) => internal("list", e),
    }
}

/// The discovered models for one provider (from `provider_models`, keyed by label).
pub(crate) async fn list_models(
    State(state): State<AppState>,
    _claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let label: Option<(String,)> = match sqlx::query_as(
        "SELECT label FROM custom_providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return internal("list_models label lookup", e),
    };
    let Some((label,)) = label else {
        return err(StatusCode::NOT_FOUND, "no such custom provider");
    };
    let models: Result<Vec<(String,)>, _> =
        sqlx::query_as("SELECT model FROM provider_models WHERE provider = $1 ORDER BY model")
            .bind(&label)
            .fetch_all(&state.db)
            .await;
    match models {
        Ok(rows) => {
            let models: Vec<String> = rows.into_iter().map(|(m,)| m).collect();
            ApiResponse::ok(json!({ "models": models }), "Models retrieved").into_response()
        }
        Err(e) => internal("list_models", e),
    }
}

/// Register a custom provider. Superuser only. The internal label is auto-generated
/// from the display name; the model list is discovered by catalog sync (and can also
/// be probed first via `POST /custom-providers/test`), so no model is required here.
pub(crate) async fn create(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateRequest>,
) -> Response {
    let dialect = match validate_dialect(&body.kind, body.api_version.as_deref()) {
        Ok(d) => d,
        Err(msg) => return err(StatusCode::BAD_REQUEST, msg),
    };
    let display_name = body.display_name.trim();
    // The dialect owns URL normalization (Azure accepts both portal forms of the
    // resource URL), so what is stored is what dispatch will address.
    let base_url = dialect.normalize_base(&body.base_url);
    if display_name.is_empty() || base_url.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "display_name and base_url are required",
        );
    }
    if body.api_key.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "api_key is required");
    }

    // Auto-generate internal label from display name.
    let mut label = slugify(display_name);
    if RESERVED_LABELS.contains(&label.as_str()) {
        label = format!("{label}-custom");
    }

    let encrypted = SecretsCrypto::for_platform_settings().encrypt(body.api_key.trim());
    let created_by = match claims.user_uuid() {
        Ok(u) => u,
        Err((status, msg)) => return err(status, msg),
    };

    // Try insert; on label collision, append a suffix.
    let mut id: Option<Uuid> = None;
    for suffix in 0..10 {
        let candidate = if suffix == 0 {
            label.clone()
        } else {
            format!("{label}-{suffix}")
        };
        match sqlx::query_as::<_, (Uuid,)>(
            "INSERT INTO custom_providers \
               (label, display_name, base_url, kind, api_version, encrypted_api_key, \
                catalog_sync_enabled, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(&candidate)
        .bind(display_name)
        .bind(&base_url)
        .bind(dialect.kind())
        .bind(azure_api_version(&dialect))
        .bind(&encrypted)
        .bind(body.catalog_sync_enabled)
        .bind(created_by)
        .fetch_one(&state.db)
        .await
        {
            Ok((new_id,)) => {
                label = candidate;
                id = Some(new_id);
                break;
            }
            Err(sqlx::Error::Database(dbe)) if dbe.is_unique_violation() => continue,
            Err(e) => return internal("create insert", e),
        }
    }

    let Some(id) = id else {
        return err(
            StatusCode::CONFLICT,
            format!("could not generate a unique label for '{display_name}'"),
        );
    };

    // Sync the model catalog.
    let discovered =
        match nasiko_llm_router::routing::catalog::sync_one(&state.db, &state.http_client, &label)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(%e, %label, "custom_providers: initial catalog sync failed");
                0
            }
        };

    // Sync pricing from Portkey so the new provider has cost data immediately
    // (the background loop runs every 24h — too long to wait).
    let priced = nasiko_llm_router::routing::pricing_sync::sync_one_provider(
        &state.db,
        &state.http_client,
        &label,
        &base_url,
    )
    .await;

    ApiResponse::created(
        json!({ "id": id, "label": label, "discovered_models": discovered, "priced_models": priced }),
        "Custom provider registered",
    )
    .into_response()
}

/// Update a custom provider. Superuser only. A provided `api_key` rotates the stored
/// credential; the resolver reads the row per request, so a rotation takes effect on
/// the next call with no restart. `kind` is deliberately not updatable — changing the
/// dialect repoints every config on this label at a different URL shape, which is a
/// delete-and-re-register, not an edit.
pub(crate) async fn update(
    State(state): State<AppState>,
    _claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRequest>,
) -> Response {
    // The dialect is immutable, but normalizing an updated base URL needs it, so read
    // the stored kind first.
    let current: Option<(String,)> = match sqlx::query_as(
        "SELECT kind FROM custom_providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return internal("update kind lookup", e),
    };
    let Some((kind,)) = current else {
        return err(StatusCode::NOT_FOUND, "no such custom provider");
    };
    // `api_version` is validated against the stored kind: supplying one for a plain
    // OpenAI endpoint is a no-op, and blanking Azure's would violate the DB CHECK.
    let api_version = body
        .api_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if kind == KIND_AZURE_OPENAI && body.api_version.is_some() && api_version.is_none() {
        return err(
            StatusCode::BAD_REQUEST,
            format!("api_version cannot be cleared for kind '{KIND_AZURE_OPENAI}'"),
        );
    }
    let dialect = ProviderDialect::from_kind(&kind, api_version);

    // COALESCE keeps the existing value for any field left null; the key is
    // re-encrypted only when a new one is supplied.
    let encrypted = body
        .api_key
        .as_deref()
        .map(|k| SecretsCrypto::for_platform_settings().encrypt(k.trim()));
    let base_url = body.base_url.as_deref().map(|b| dialect.normalize_base(b));
    let result = sqlx::query(
        "UPDATE custom_providers SET \
           display_name = COALESCE($2, display_name), \
           base_url = COALESCE($3, base_url), \
           encrypted_api_key = COALESCE($4, encrypted_api_key), \
           default_model = COALESCE($5, default_model), \
           catalog_sync_enabled = COALESCE($6, catalog_sync_enabled), \
           api_version = COALESCE($7, api_version) \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(body.display_name.as_deref().map(str::trim))
    .bind(base_url)
    .bind(encrypted)
    .bind(body.default_model.as_deref().map(str::trim))
    .bind(body.catalog_sync_enabled)
    .bind(api_version)
    .execute(&state.db)
    .await;
    match result {
        Ok(r) if r.rows_affected() == 0 => err(StatusCode::NOT_FOUND, "no such custom provider"),
        Ok(_) => ApiResponse::ok(json!({ "id": id }), "Custom provider updated").into_response(),
        Err(e) => internal("update", e),
    }
}

/// Soft-delete a custom provider. Blocked (409) while any `llm_configs` row still
/// names it — a dangling provider name would make the resolver fail on the next call.
pub(crate) async fn delete_provider(
    State(state): State<AppState>,
    _claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let label: Option<(String,)> = match sqlx::query_as(
        "SELECT label FROM custom_providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return internal("delete label lookup", e),
    };
    let Some((label,)) = label else {
        return err(StatusCode::NOT_FOUND, "no such custom provider");
    };

    // Referential block: list the configs that still point at this provider.
    let refs: Vec<(String,)> = match sqlx::query_as(
        "SELECT name FROM llm_configs WHERE provider = $1 AND deleted_at IS NULL ORDER BY name",
    )
    .bind(&label)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return internal("delete ref check", e),
    };
    if !refs.is_empty() {
        let names: Vec<String> = refs.into_iter().map(|(n,)| n).collect();
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "message": "provider is still referenced by LLM configs; repoint them first",
                "referencing_configs": names,
            })),
        )
            .into_response();
    }

    // Soft-delete the row and drop its catalog rows so the label stops appearing as a
    // phantom provider in the model dropdown.
    if let Err(e) = sqlx::query("UPDATE custom_providers SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        return internal("delete soft-delete", e);
    }
    let _ = sqlx::query("DELETE FROM provider_models WHERE provider = $1")
        .bind(&label)
        .execute(&state.db)
        .await;
    ApiResponse::ok(json!({ "id": id }), "Custom provider deleted").into_response()
}

/// Refresh one provider's model list on demand. Superuser only.
pub(crate) async fn sync_now(
    State(state): State<AppState>,
    _claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let label: Option<(String,)> = match sqlx::query_as(
        "SELECT label FROM custom_providers WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return internal("sync label lookup", e),
    };
    let Some((label,)) = label else {
        return err(StatusCode::NOT_FOUND, "no such custom provider");
    };
    match nasiko_llm_router::routing::catalog::sync_one(&state.db, &state.http_client, &label).await
    {
        Ok(n) => {
            ApiResponse::ok(json!({ "discovered_models": n }), "Sync complete").into_response()
        }
        Err(e) => internal("sync", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_validation_rejects_unknown_kinds_and_missing_azure_version() {
        assert_eq!(
            validate_dialect(KIND_OPENAI, None),
            Ok(ProviderDialect::OpenAi)
        );
        // An api_version supplied for a plain endpoint is simply ignored.
        assert_eq!(
            validate_dialect(KIND_OPENAI, Some("2024-10-21")),
            Ok(ProviderDialect::OpenAi)
        );
        assert_eq!(
            validate_dialect(KIND_AZURE_OPENAI, Some(" 2024-10-21 ")),
            Ok(ProviderDialect::AzureOpenAi {
                api_version: "2024-10-21".into()
            })
        );
        // Azure without a version is a 400 here, not a constraint violation later.
        assert!(validate_dialect(KIND_AZURE_OPENAI, None).is_err());
        assert!(validate_dialect(KIND_AZURE_OPENAI, Some("   ")).is_err());
        assert_eq!(
            validate_dialect(KIND_BEDROCK_CONVERSE, None),
            Ok(ProviderDialect::BedrockConverse)
        );
        assert!(validate_dialect("bedrock", None).is_err());
    }

    #[test]
    fn only_azure_rows_store_an_api_version() {
        assert_eq!(azure_api_version(&ProviderDialect::OpenAi), None);
        assert_eq!(
            azure_api_version(&ProviderDialect::AzureOpenAi {
                api_version: "2024-10-21".into()
            }),
            Some("2024-10-21")
        );
    }
}
