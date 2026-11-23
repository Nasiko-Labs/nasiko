//! `/api/llm-configs` — a per-user library of reusable LLM routing configs.
//!
//! A user creates named configs once and attaches them to their own agents (the attach side
//! lives in [`crate::agents::llm_config`]). Resolution for an agent is: attached config → the
//! owner's default config → none (platform defaults). Ownership is per-user: every row is
//! scoped to `created_by`, so a config's referenced API-key secret always lives in the same
//! user's `user_secrets` store the LLM router reads from — no cross-user secret sharing.
//!
//! Mounted under `/api` behind `require_auth` (like [`crate::secrets`]); every query is scoped
//! to the caller, so a config the caller doesn't own reads as 404.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use nasiko_secrets::SecretsCrypto;
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::Claims;
use crate::embedding_change::{self, EmbeddingTarget};
use crate::mcp::ApiResponse;
use crate::state::AppState;

/// Built-in providers the LLM router routes through a dedicated client. A config may
/// also name any admin-registered custom provider — see [`provider_is_supported`].
const SUPPORTED_PROVIDERS: [&str; 4] = ["openai", "anthropic", "gemini", "openrouter"];

/// Resolve a provider identifier to the internal label stored in `llm_configs.provider`.
/// Built-in names pass through; a UUID is resolved to the custom provider's label;
/// an existing label passes through (backward compat).
async fn resolve_provider_label(db: &sqlx::PgPool, provider: &str) -> Result<String, String> {
    if SUPPORTED_PROVIDERS.contains(&provider) {
        return Ok(provider.to_string());
    }
    // Try as UUID first.
    if let Ok(uuid) = uuid::Uuid::parse_str(provider) {
        return match sqlx::query_scalar::<_, String>(
            "SELECT label FROM custom_providers WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(uuid)
        .fetch_optional(db)
        .await
        {
            Ok(Some(label)) => Ok(label),
            Ok(None) => Err(unsupported_provider_msg(provider)),
            Err(_) => Err("failed to resolve provider".to_string()),
        };
    }
    // Legacy: pass through as-is (label-based).
    Ok(provider.to_string())
}

/// Like [`resolve_provider_label`], but a label that matches no registered custom provider
/// is an error instead of passing through.
///
/// [`resolve_provider_label`]'s pass-through exists for `llm_configs.provider` rows written
/// before custom providers were a table; those labels must keep resolving. The
/// organization-wide embedding provider has no such history — it ships with this feature —
/// so it can afford to be strict, and it should be: it is one row for the whole
/// deployment, and a typo accepted here would sit unnoticed until some unrelated agent's
/// next embedding call failed.
pub(crate) async fn require_known_provider(
    db: &sqlx::PgPool,
    provider: &str,
) -> Result<String, String> {
    let label = resolve_provider_label(db, provider).await?;
    if SUPPORTED_PROVIDERS.contains(&label.as_str()) {
        return Ok(label);
    }
    let registered = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM custom_providers WHERE label = $1 AND deleted_at IS NULL)",
    )
    .bind(&label)
    .fetch_one(db)
    .await
    .map_err(|_| "failed to resolve provider".to_string())?;
    if registered {
        Ok(label)
    } else {
        Err(unsupported_provider_msg(provider))
    }
}

/// Client-facing message for a provider name that is neither a built-in nor a
/// registered custom provider.
fn unsupported_provider_msg(provider: &str) -> String {
    format!(
        "unsupported provider '{provider}' (expected a built-in [{}] or a registered custom provider)",
        SUPPORTED_PROVIDERS.join(", ")
    )
}

/// The `llm_configs` columns returned to clients, assembled by Postgres into one JSON object.
const CONFIG_JSON: &str = "json_build_object(\
     'id', id, 'name', name, 'provider', provider, 'model', model, \
     'fallback_models', fallback_models, 'temperature', temperature, \
     'max_tokens', max_tokens, 'api_key_secret_name', api_key_secret_name, \
     'pinned', pinned, 'pinned_model', pinned_model, \
     'tier1_model', tier1_model, 'tier2_model', tier2_model, 'tier3_model', tier3_model, \
     'embedding_provider', embedding_provider, 'embedding_model', embedding_model, \
     'embedding_api_key_secret_name', embedding_api_key_secret_name, \
     'embedding_model_override', embedding_model_override, \
     'is_default', is_default, \
     'created_at', created_at, 'updated_at', updated_at)";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/llm-configs", get(list).post(create))
        .route(
            "/llm-configs/{id}",
            get(get_one).patch(update).delete(delete_config),
        )
        .route(
            "/llm-configs/{id}/default",
            post(set_default).delete(clear_default),
        )
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateLlmConfigRequest {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// Name of the caller's `user_secrets` row holding the provider API key. None ⇒ the
    /// platform-key path.
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
    /// Plaintext key to store under `api_key_secret_name` when no such secret exists yet.
    /// Required when the named secret is absent; ignored (as an upsert) when it already exists.
    #[serde(default)]
    pub secret_value: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pinned_model: Option<String>,
    /// Per-config tier→model overrides. When set, the smart router uses these instead of
    /// the global `model_registry` for this config's provider.
    #[serde(default)]
    pub tier1_model: Option<String>,
    #[serde(default)]
    pub tier2_model: Option<String>,
    #[serde(default)]
    pub tier3_model: Option<String>,
    /// Embedding destination — independent of the chat provider/model above. Set the
    /// provider and model together or not at all; the secret name is optional and means
    /// "use the platform key for this provider" when absent.
    #[serde(default)]
    pub embedding_provider: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub embedding_api_key_secret_name: Option<String>,
    /// Plaintext key to store under `embedding_api_key_secret_name` when no such secret
    /// exists yet — the embedding-side twin of `secret_value`.
    #[serde(default)]
    pub embedding_secret_value: Option<String>,
    /// When true, this config's `embedding_model` outranks the model an agent names on
    /// its own `POST /v1/embeddings` request.
    #[serde(default)]
    pub embedding_model_override: bool,
    /// Mark this as the caller's default (clears any prior default).
    #[serde(default)]
    pub is_default: bool,
    /// Accepted for client uniformity and ignored: a create has no previous embedding
    /// model, so there is never anything to confirm.
    #[serde(default)]
    pub confirm_embedding_change: bool,
}

/// Partial update — every field is optional. Absent fields keep their current value.
/// `is_default` is not touched here — use `POST /llm-configs/{id}/default`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateLlmConfigRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub fallback_models: Option<Vec<String>>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
    #[serde(default)]
    pub secret_value: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub pinned_model: Option<String>,
    #[serde(default)]
    pub tier1_model: Option<String>,
    #[serde(default)]
    pub tier2_model: Option<String>,
    #[serde(default)]
    pub tier3_model: Option<String>,
    #[serde(default)]
    pub embedding_provider: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub embedding_api_key_secret_name: Option<String>,
    #[serde(default)]
    pub embedding_secret_value: Option<String>,
    #[serde(default)]
    pub embedding_model_override: Option<bool>,
    /// Clear the embedding destination. Needed because this PATCH is `COALESCE`-partial —
    /// an absent field means "keep", so there is otherwise no way to express "unset".
    /// Clearing is itself a confirmable transition (see [`crate::embedding_change`]).
    #[serde(default)]
    pub clear_embedding_model: bool,
    /// Acknowledge that changing an already-set embedding provider/model invalidates
    /// vectors indexed with the old one. Without it such a change is rejected with 428.
    #[serde(default)]
    pub confirm_embedding_change: bool,
}

/// Validate the fields that don't need the DB. Shared by create/update.
///
/// `model` may be `None` when the user relies entirely on tier-based routing; it is required
/// only when no tier model is set (otherwise the router has no fallback).
/// How long one model check may take, so a slow provider never stalls saving a config.
const MODEL_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Why the provider refuses `model`, or `None` when it serves it or the answer is inconclusive.
///
/// One 1-token chat call. Only a 404 counts as a refusal: that is how OpenAI answers both a model
/// that does not exist and one it has retired ("has been deprecated"). Its model list still lists
/// retired models, so a list lookup would not catch them. Any other outcome (timeout, network
/// error, 401, 5xx) is not the model's fault and must never block a save.
async fn openai_model_refusal(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Option<String> {
    let resp = http
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .timeout(MODEL_CHECK_TIMEOUT)
        .json(&json!({
            "model": model,
            "messages": [{ "role": "user", "content": "ping" }],
            "max_completion_tokens": 1,
        }))
        .send()
        .await
        .map_err(|e| tracing::warn!(%model, error = %e, "llm config: model check could not run"))
        .ok()?;
    if resp.status() != StatusCode::NOT_FOUND {
        return None;
    }
    let body: Value = resp.json().await.unwrap_or_default();
    let reason = body["error"]["message"]
        .as_str()
        .unwrap_or("the provider does not serve this model");
    Some(format!("model '{model}' can't be used: {reason}"))
}

/// Refusals for the models a config names, checked against the provider it will call.
///
/// Built-in OpenAI only, with the key the router would use for a config without a stored
/// secret: a key sent with this save, else the platform key, else the user's saved key. A
/// config that points at a stored secret is not checked, so this never decrypts one.
async fn unservable_models(
    state: &AppState,
    user_id: Uuid,
    provider: &str,
    stored_secret_name: Option<&str>,
    secret_value: Option<&str>,
    models: &[&str],
) -> Vec<String> {
    if provider != "openai" {
        return Vec::new();
    }
    let api_key = match secret_value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(key) => Some(key.to_string()),
        None if stored_secret_name.is_some_and(|n| !n.trim().is_empty()) => None,
        None => match state.config.openai_api_key.clone() {
            Some(key) => Some(key),
            None => nasiko_orchestrator::user_key::saved_openai_key(&state.db, user_id).await,
        },
    };
    let Some(api_key) = api_key else {
        return Vec::new();
    };
    let base_url = state
        .config
        .openai_base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let mut distinct: Vec<&str> = models
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect();
    distinct.sort_unstable();
    distinct.dedup();
    futures::future::join_all(
        distinct
            .into_iter()
            .map(|m| openai_model_refusal(&state.http_client, &base_url, &api_key, m)),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

fn validate(
    provider: &str,
    provider_supported: bool,
    model: Option<&str>,
    pinned_model: &Option<String>,
    has_any_tier: bool,
) -> Result<(), String> {
    if !provider_supported {
        return Err(unsupported_provider_msg(provider));
    }
    match model {
        Some(m) if m.trim().is_empty() => {
            return Err("model must not be empty when provided".to_string());
        }
        None if !has_any_tier => {
            return Err(
                "either model or at least one tier model (tier1_model, tier2_model, tier3_model) is required"
                    .to_string(),
            );
        }
        _ => {}
    }
    if let Some(pm) = pinned_model
        && pm.trim().is_empty()
    {
        return Err("pinned_model must not be empty".to_string());
    }
    Ok(())
}

/// `""` is not a value. Cleared form fields and shell flags arrive as empty strings, and
/// storing one would put a routable-looking destination in the row: the resolver would
/// read `Some("")` and try to call a provider with no name. Every write of an embedding
/// column goes through this.
fn blank_to_none(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Validate the embedding pair. The DB enforces all-or-nothing too (a CHECK constraint),
/// but a constraint violation surfaces as a 500 — this turns it into an actionable 400.
///
/// The provider must be resolvable the same way the chat provider is, so built-ins and
/// registered custom providers both work and a typo is caught at write time rather than
/// as an opaque failure on the agent's first embedding call.
async fn validate_embedding(
    db: &sqlx::PgPool,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Option<String>, String> {
    let provider = provider.map(str::trim).filter(|p| !p.is_empty());
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    match (provider, model) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(
            "embedding_provider and embedding_model must be set together (an embedding model \
             with no provider cannot be routed)"
                .to_string(),
        ),
        (Some(p), Some(_)) => resolve_provider_label(db, p).await.map(Some),
    }
}

/// Ensure the caller's secret named `name` exists, storing `value` under it when supplied.
/// A referenced-but-missing secret with no value is a 400 (the resolver would otherwise fail
/// at call time). No-op when `name` is empty/None (platform-key path).
async fn ensure_secret(
    db: &sqlx::PgPool,
    user_id: Uuid,
    name: Option<&str>,
    value: Option<&str>,
) -> Result<(), (StatusCode, String)> {
    let Some(name) = name.filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if let Some(v) = value.filter(|s| !s.is_empty()) {
        let encrypted = SecretsCrypto::for_user(user_id).encrypt(v);
        sqlx::query(
            "INSERT INTO user_secrets (user_id, name, encrypted_value) VALUES ($1, $2, $3) \
             ON CONFLICT (user_id, name) DO UPDATE SET encrypted_value = $3, updated_at = now()",
        )
        .bind(user_id)
        .bind(name)
        .bind(&encrypted)
        .execute(db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to store secret '{name}': {e}"),
            )
        })?;
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM user_secrets WHERE user_id = $1 AND name = $2)",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap_or(false);
    if exists {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("secret '{name}' not found; provide secret_value to store it"),
        ))
    }
}

/// The embedding destination currently stored on a config row, read from the JSON that
/// [`fetch_config`] already returns rather than from a second query.
fn stored_embedding(config: &Value) -> EmbeddingTarget {
    let field = |k: &str| config.get(k).and_then(|v| v.as_str()).map(str::to_string);
    EmbeddingTarget::new(field("embedding_provider"), field("embedding_model"))
}

/// One config as JSON, scoped to the caller. `None` ⇒ unknown or not owned (→ 404).
async fn fetch_config(db: &sqlx::PgPool, id: Uuid, user_id: Uuid) -> Option<Value> {
    sqlx::query_scalar::<_, Value>(&format!(
        "SELECT {CONFIG_JSON} FROM llm_configs \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

// ─── GET /llm-configs ────────────────────────────────────────────────────────

/// List the caller's LLM configs, ordered by name.
#[utoipa::path(
    get,
    path = "/api/llm-configs",
    tag = "llm-router",
    responses(
        (status = 200, description = "`data` is an array of config objects, each with: \
            id, name, provider, model, fallback_models, temperature, max_tokens, \
            api_key_secret_name, pinned, pinned_model, tier1_model, tier2_model, tier3_model, \
            embedding_provider, embedding_model, embedding_api_key_secret_name, \
            embedding_model_override, is_default, created_at, updated_at", body = crate::mcp::openapi::McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn list(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let rows: Vec<Value> = sqlx::query_scalar::<_, Value>(&format!(
        "SELECT {CONFIG_JSON} FROM llm_configs \
         WHERE created_by = $1 AND deleted_at IS NULL ORDER BY name"
    ))
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    ApiResponse::ok(json!(rows), "LLM configs retrieved successfully").into_response()
}

// ─── POST /llm-configs ───────────────────────────────────────────────────────

/// Create a named LLM config owned by the caller, optionally storing the
/// referenced API-key secret and/or marking it as the caller's default.
#[utoipa::path(
    post,
    path = "/api/llm-configs",
    tag = "llm-router",
    request_body = CreateLlmConfigRequest,
    responses(
        (status = 201, description = "`data` is the created config object (id, name, provider, \
            model, fallback_models, temperature, max_tokens, api_key_secret_name, pinned, \
            pinned_model, tier1_model, tier2_model, tier3_model, embedding_provider, \
            embedding_model, embedding_api_key_secret_name, embedding_model_override, \
            is_default, created_at, updated_at)", body = crate::mcp::openapi::McpEnvelope),
        (status = 400, description = "Validation failed (name/provider/model/pinned_model/secret, \
            or embedding_provider/embedding_model not set together)"),
        (status = 401, description = "Missing or invalid session"),
        (status = 409, description = "A config with this name already exists"),
    ),
)]
pub(crate) async fn create(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<CreateLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if req.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
    }
    // Resolve provider identifier (built-in name or custom provider UUID → internal label).
    let provider_label = match resolve_provider_label(&state.db, &req.provider).await {
        Ok(label) => label,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let has_any_tier =
        req.tier1_model.is_some() || req.tier2_model.is_some() || req.tier3_model.is_some();
    if let Err(msg) = validate(
        &provider_label,
        true, // already validated by resolve_provider_label
        req.model.as_deref(),
        &req.pinned_model,
        has_any_tier,
    ) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    let embedding_provider = match validate_embedding(
        &state.db,
        req.embedding_provider.as_deref(),
        req.embedding_model.as_deref(),
    )
    .await
    {
        Ok(label) => label,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    if config_name_taken(&state.db, user_id, &req.name).await {
        return (
            StatusCode::CONFLICT,
            format!("an LLM config named '{}' already exists", req.name),
        )
            .into_response();
    }
    let named: Vec<&str> = [
        &req.model,
        &req.tier1_model,
        &req.tier2_model,
        &req.tier3_model,
        &req.pinned_model,
    ]
    .into_iter()
    .flatten()
    .map(String::as_str)
    .chain(req.fallback_models.iter().map(String::as_str))
    .collect();
    let refusals = unservable_models(
        &state,
        user_id,
        &provider_label,
        req.api_key_secret_name.as_deref(),
        req.secret_value.as_deref(),
        &named,
    )
    .await;
    if !refusals.is_empty() {
        return (StatusCode::BAD_REQUEST, refusals.join("; ")).into_response();
    }
    if let Err((code, msg)) = ensure_secret(
        &state.db,
        user_id,
        req.api_key_secret_name.as_deref(),
        req.secret_value.as_deref(),
    )
    .await
    {
        return (code, msg).into_response();
    }
    if let Err((code, msg)) = ensure_secret(
        &state.db,
        user_id,
        req.embedding_api_key_secret_name.as_deref(),
        req.embedding_secret_value.as_deref(),
    )
    .await
    {
        return (code, msg).into_response();
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return db_error("begin", e),
    };
    // Only one default per owner — clear the prior one within the same transaction.
    if req.is_default
        && let Err(e) = sqlx::query(
            "UPDATE llm_configs SET is_default = false \
             WHERE created_by = $1 AND is_default AND deleted_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
    {
        return db_error("clear default", e);
    }

    let inserted: Result<(Uuid,), _> = sqlx::query_as(
        "INSERT INTO llm_configs \
         (created_by, name, provider, model, fallback_models, temperature, max_tokens, \
          api_key_secret_name, pinned, pinned_model, tier1_model, tier2_model, tier3_model, \
          is_default, embedding_provider, embedding_model, embedding_api_key_secret_name, \
          embedding_model_override) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(&req.name)
    .bind(&provider_label)
    .bind(&req.model)
    .bind(sqlx::types::Json(&req.fallback_models))
    .bind(req.temperature)
    .bind(req.max_tokens)
    .bind(&req.api_key_secret_name)
    .bind(req.pinned)
    .bind(&req.pinned_model)
    .bind(&req.tier1_model)
    .bind(&req.tier2_model)
    .bind(&req.tier3_model)
    .bind(req.is_default)
    .bind(&embedding_provider)
    .bind(blank_to_none(&req.embedding_model))
    .bind(blank_to_none(&req.embedding_api_key_secret_name))
    .bind(req.embedding_model_override)
    .fetch_one(&mut *tx)
    .await;

    let id = match inserted {
        Ok((id,)) => id,
        Err(e) => return db_error("insert", e),
    };
    if let Err(e) = tx.commit().await {
        return db_error("commit", e);
    }
    state.llm_config_cache.clear();

    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::created(cfg, "LLM config created successfully").into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "config vanished after create",
        )
            .into_response(),
    }
}

// ─── GET /llm-configs/{id} ───────────────────────────────────────────────────

/// Fetch one of the caller's configs; configs the caller doesn't own read as 404.
#[utoipa::path(
    get,
    path = "/api/llm-configs/{id}",
    tag = "llm-router",
    params(
        ("id" = Uuid, Path, description = "LLM config id"),
    ),
    responses(
        (status = 200, description = "`data` is the config object (id, name, provider, model, \
            fallback_models, temperature, max_tokens, api_key_secret_name, pinned, pinned_model, \
            tier1_model, tier2_model, tier3_model, embedding_provider, embedding_model, \
            embedding_api_key_secret_name, embedding_model_override, is_default, created_at, \
            updated_at)",
            body = crate::mcp::openapi::McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
        (status = 404, description = "Unknown id, or not owned by the caller"),
    ),
)]
pub(crate) async fn get_one(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::ok(cfg, "LLM config retrieved successfully").into_response(),
        None => (StatusCode::NOT_FOUND, "llm config not found").into_response(),
    }
}

// ─── PATCH /llm-configs/{id} ─────────────────────────────────────────────────

/// Partially update one of the caller's configs; absent fields keep their
/// current value. Use `POST /api/llm-configs/{id}/default` to change the default.
#[utoipa::path(
    patch,
    path = "/api/llm-configs/{id}",
    tag = "llm-router",
    params(
        ("id" = Uuid, Path, description = "LLM config id"),
    ),
    request_body = UpdateLlmConfigRequest,
    responses(
        (status = 200, description = "`data` is the updated config object (id, name, provider, \
            model, fallback_models, temperature, max_tokens, api_key_secret_name, pinned, \
            pinned_model, tier1_model, tier2_model, tier3_model, embedding_provider, \
            embedding_model, embedding_api_key_secret_name, embedding_model_override, \
            is_default, created_at, updated_at)", body = crate::mcp::openapi::McpEnvelope),
        (status = 400, description = "Validation failed (name/provider/model/pinned_model/secret, \
            or embedding_provider/embedding_model not set together)"),
        (status = 401, description = "Missing or invalid session"),
        (status = 404, description = "Unknown id, or not owned by the caller"),
        (status = 409, description = "Another config with this name already exists"),
        (status = 428, description = "The embedding provider/model was already set and is being \
            changed or cleared. Vectors indexed with the previous model cannot be compared with \
            the new one. Body carries `code = embedding_change_requires_confirmation` and \
            `details.from`/`details.to`; re-send with `confirm_embedding_change: true`."),
    ),
)]
pub(crate) async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    // The 404 check already reads the whole row, so the current embedding values for the
    // §2.6 comparison come from it rather than a second query.
    let Some(current) = fetch_config(&state.db, id, user_id).await else {
        return (StatusCode::NOT_FOUND, "llm config not found").into_response();
    };
    // Validate only the fields that are present; resolve UUID → internal label.
    let resolved_provider = if let Some(ref provider) = req.provider {
        match resolve_provider_label(&state.db, provider).await {
            Ok(label) => Some(label),
            Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
        }
    } else {
        None
    };
    if let Some(model) = &req.model
        && model.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            "model must not be empty when provided",
        )
            .into_response();
    }
    if let Some(pm) = &req.pinned_model
        && pm.trim().is_empty()
    {
        return (StatusCode::BAD_REQUEST, "pinned_model must not be empty").into_response();
    }
    if req.clear_embedding_model
        && (req.embedding_provider.is_some() || req.embedding_model.is_some())
    {
        return (
            StatusCode::BAD_REQUEST,
            "clear_embedding_model cannot be combined with embedding_provider/embedding_model",
        )
            .into_response();
    }
    // A supplied provider may be a custom provider's UUID; resolve it to the stored label
    // before it is compared with, or merged into, what the row already holds.
    let embedding_model = blank_to_none(&req.embedding_model);
    let embedding_secret_name = blank_to_none(&req.embedding_api_key_secret_name);
    let embedding_provider = match blank_to_none(&req.embedding_provider) {
        Some(p) => match resolve_provider_label(&state.db, &p).await {
            Ok(label) => Some(label),
            Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
        },
        None => None,
    };

    // This PATCH is partial, so an absent field means "keep". Validation therefore has to
    // run on the row as it *will be*, not on the request alone — otherwise changing only
    // the model reads as an embedding model with no provider and is rejected, even though
    // the provider is right there in the row and is not changing.
    let current_embedding = stored_embedding(&current);
    let incoming_embedding = if req.clear_embedding_model {
        EmbeddingTarget::new(None, None)
    } else {
        EmbeddingTarget::new(
            embedding_provider
                .clone()
                .or_else(|| current_embedding.provider.clone()),
            embedding_model
                .clone()
                .or_else(|| current_embedding.model.clone()),
        )
    };
    if incoming_embedding.provider.is_some() != incoming_embedding.model.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            "embedding_provider and embedding_model must be set together (an embedding model \
             with no provider cannot be routed)",
        )
            .into_response();
    }

    // §2.6: a change to an already-set embedding provider/model is rejected unless the
    // caller confirms it. Checked *before* ensure_secret and before the UPDATE, so a
    // rejected request writes nothing at all — no secret, no touched `updated_at`.
    match embedding_change::needs_confirmation(&current_embedding, &incoming_embedding) {
        Some(change) if !req.confirm_embedding_change => {
            return embedding_change::confirmation_required(&change);
        }
        Some(change) => embedding_change::log_confirmed(
            "llm_config",
            &id.to_string(),
            &user_id.to_string(),
            &change,
        ),
        None => {}
    }
    if let Some(name) = req.name.as_deref() {
        if name.trim().is_empty() {
            return (StatusCode::BAD_REQUEST, "name must not be empty").into_response();
        }
        if config_name_taken_by_other(&state.db, user_id, name, id).await {
            return (
                StatusCode::CONFLICT,
                format!("an LLM config named '{name}' already exists"),
            )
                .into_response();
        }
    }
    // Only the models this change names; the row's other models were checked when written.
    let named: Vec<&str> = [
        &req.model,
        &req.tier1_model,
        &req.tier2_model,
        &req.tier3_model,
        &req.pinned_model,
    ]
    .into_iter()
    .flatten()
    .map(String::as_str)
    .chain(req.fallback_models.iter().flatten().map(String::as_str))
    .collect();
    let provider = resolved_provider
        .clone()
        .or_else(|| current["provider"].as_str().map(str::to_string))
        .unwrap_or_default();
    let stored_secret = req
        .api_key_secret_name
        .clone()
        .or_else(|| current["api_key_secret_name"].as_str().map(str::to_string));
    let refusals = unservable_models(
        &state,
        user_id,
        &provider,
        stored_secret.as_deref(),
        req.secret_value.as_deref(),
        &named,
    )
    .await;
    if !refusals.is_empty() {
        return (StatusCode::BAD_REQUEST, refusals.join("; ")).into_response();
    }
    if let Err((code, msg)) = ensure_secret(
        &state.db,
        user_id,
        req.api_key_secret_name.as_deref(),
        req.secret_value.as_deref(),
    )
    .await
    {
        return (code, msg).into_response();
    }
    if let Err((code, msg)) = ensure_secret(
        &state.db,
        user_id,
        req.embedding_api_key_secret_name.as_deref(),
        req.embedding_secret_value.as_deref(),
    )
    .await
    {
        return (code, msg).into_response();
    }

    let result = sqlx::query(
        "UPDATE llm_configs SET \
         name = COALESCE($3, name), \
         provider = COALESCE($4, provider), \
         model = COALESCE($5, model), \
         fallback_models = COALESCE($6, fallback_models), \
         temperature = COALESCE($7, temperature), \
         max_tokens = COALESCE($8, max_tokens), \
         api_key_secret_name = COALESCE($9, api_key_secret_name), \
         pinned = COALESCE($10, pinned), \
         pinned_model = COALESCE($11, pinned_model), \
         tier1_model = COALESCE($12, tier1_model), \
         tier2_model = COALESCE($13, tier2_model), \
         tier3_model = COALESCE($14, tier3_model), \
         embedding_provider = CASE WHEN $15 THEN NULL \
                                   ELSE COALESCE($16, embedding_provider) END, \
         embedding_model = CASE WHEN $15 THEN NULL \
                                ELSE COALESCE($17, embedding_model) END, \
         embedding_api_key_secret_name = CASE WHEN $15 THEN NULL \
                                              ELSE COALESCE($18, embedding_api_key_secret_name) \
                                         END, \
         embedding_model_override = COALESCE($19, embedding_model_override), \
         updated_at = now() \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .bind(&req.name)
    .bind(&resolved_provider)
    .bind(&req.model)
    .bind(req.fallback_models.as_ref().map(sqlx::types::Json))
    .bind(req.temperature)
    .bind(req.max_tokens)
    .bind(&req.api_key_secret_name)
    .bind(req.pinned)
    .bind(&req.pinned_model)
    .bind(&req.tier1_model)
    .bind(&req.tier2_model)
    .bind(&req.tier3_model)
    .bind(req.clear_embedding_model)
    .bind(&embedding_provider)
    .bind(&embedding_model)
    .bind(&embedding_secret_name)
    .bind(req.embedding_model_override)
    .execute(&state.db)
    .await;

    if let Err(e) = result {
        return db_error("update", e);
    }
    state.llm_config_cache.clear();
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::ok(cfg, "LLM config updated successfully").into_response(),
        None => (StatusCode::NOT_FOUND, "llm config not found").into_response(),
    }
}

// ─── DELETE /llm-configs/{id} ────────────────────────────────────────────────

/// Soft-delete one of the caller's configs. Fails while any live agent still
/// has the config attached.
#[utoipa::path(
    delete,
    path = "/api/llm-configs/{id}",
    tag = "llm-router",
    params(
        ("id" = Uuid, Path, description = "LLM config id"),
    ),
    responses(
        (status = 200, description = "Config deleted", body = crate::openapi::EmptyEnvelope),
        (status = 401, description = "Missing or invalid session"),
        (status = 404, description = "Unknown id, or not owned by the caller"),
        (status = 409, description = "Config is attached to one or more agents"),
    ),
)]
pub(crate) async fn delete_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if fetch_config(&state.db, id, user_id).await.is_none() {
        return (StatusCode::NOT_FOUND, "llm config not found").into_response();
    }
    // A config in use by any live agent can't be deleted — detach it first.
    let in_use: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agents WHERE llm_config_id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    if in_use > 0 {
        return (
            StatusCode::CONFLICT,
            format!("config is attached to {in_use} agent(s); detach it before deleting"),
        )
            .into_response();
    }
    // Soft delete; clear is_default so the partial unique index frees the slot.
    let result = sqlx::query(
        "UPDATE llm_configs SET deleted_at = now(), is_default = false \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await;
    match result {
        Ok(_) => {
            state.llm_config_cache.clear();
            ApiResponse::ok(json!(null), "LLM config deleted successfully").into_response()
        }
        Err(e) => db_error("delete", e),
    }
}

// ─── POST /llm-configs/{id}/default ──────────────────────────────────────────

/// Mark one of the caller's configs as their default, clearing any prior default.
#[utoipa::path(
    post,
    path = "/api/llm-configs/{id}/default",
    tag = "llm-router",
    params(
        ("id" = Uuid, Path, description = "LLM config id"),
    ),
    responses(
        (status = 200, description = "`data` is the config object now marked as default (id, \
            name, provider, model, fallback_models, temperature, max_tokens, \
            api_key_secret_name, pinned, pinned_model, tier1_model, tier2_model, tier3_model, \
            embedding_provider, embedding_model, embedding_api_key_secret_name, \
            embedding_model_override, is_default, created_at, updated_at)", body = crate::mcp::openapi::McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
        (status = 404, description = "Unknown id, or not owned by the caller"),
    ),
)]
pub(crate) async fn set_default(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if fetch_config(&state.db, id, user_id).await.is_none() {
        return (StatusCode::NOT_FOUND, "llm config not found").into_response();
    }
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return db_error("begin", e),
    };
    if let Err(e) = sqlx::query(
        "UPDATE llm_configs SET is_default = false \
         WHERE created_by = $1 AND is_default AND deleted_at IS NULL AND id <> $2",
    )
    .bind(user_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    {
        return db_error("clear default", e);
    }
    if let Err(e) = sqlx::query(
        "UPDATE llm_configs SET is_default = true, updated_at = now() \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    {
        return db_error("set default", e);
    }
    if let Err(e) = tx.commit().await {
        return db_error("commit", e);
    }
    state.llm_config_cache.clear();
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::ok(cfg, "LLM config set as default successfully").into_response(),
        None => (StatusCode::NOT_FOUND, "llm config not found").into_response(),
    }
}

// ─── DELETE /llm-configs/{id}/default ────────────────────────────────────────

/// Clear the default flag on one of the caller's configs, leaving them with no
/// default. The counterpart to [`set_default`]: without it, once a config was
/// marked default the flag could only ever move to a *different* config, never
/// be removed — so "make this not the default" was unreachable, and with a
/// single config there was no way out at all.
///
/// Agents that were falling back to this default resolve to `source: "none"`
/// afterwards and use the platform key path (see `resolve_agent_config`).
#[utoipa::path(
    delete,
    path = "/api/llm-configs/{id}/default",
    tag = "llm-router",
    params(
        ("id" = Uuid, Path, description = "LLM config id"),
    ),
    responses(
        (status = 200, description = "`data` is the config object, now with `is_default: false`", body = crate::mcp::openapi::McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
        (status = 404, description = "Unknown id, or not owned by the caller"),
    ),
)]
pub(crate) async fn clear_default(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if fetch_config(&state.db, id, user_id).await.is_none() {
        return (StatusCode::NOT_FOUND, "llm config not found").into_response();
    }
    // Single statement — no transaction needed, unlike `set_default`, which has
    // to clear the old default and set the new one atomically.
    if let Err(e) = sqlx::query(
        "UPDATE llm_configs SET is_default = false, updated_at = now() \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await
    {
        return db_error("clear default", e);
    }
    state.llm_config_cache.clear();
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => {
            ApiResponse::ok(cfg, "LLM config default cleared successfully").into_response()
        }
        None => (StatusCode::NOT_FOUND, "llm config not found").into_response(),
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn config_name_taken(db: &sqlx::PgPool, user_id: Uuid, name: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM llm_configs \
         WHERE created_by = $1 AND name = $2 AND deleted_at IS NULL)",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

async fn config_name_taken_by_other(
    db: &sqlx::PgPool,
    user_id: Uuid,
    name: &str,
    exclude: Uuid,
) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM llm_configs \
         WHERE created_by = $1 AND name = $2 AND deleted_at IS NULL AND id <> $3)",
    )
    .bind(user_id)
    .bind(name)
    .bind(exclude)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

fn db_error(op: &str, e: sqlx::Error) -> axum::response::Response {
    tracing::error!(%e, op, "llm_configs: db error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("failed to {op} llm config"),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-5: OpenAI answers a retired model with 404 "has been deprecated".
    #[tokio::test]
    async fn a_retired_model_is_refused_with_the_providers_reason() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/chat/completions")
            .with_status(404)
            .with_body(r#"{"error":{"message":"The model `gpt-4.1-nano` has been deprecated"}}"#)
            .create_async()
            .await;
        let reason =
            openai_model_refusal(&reqwest::Client::new(), &server.url(), "k", "gpt-4.1-nano").await;
        assert_eq!(
            reason.as_deref(),
            Some(
                "model 'gpt-4.1-nano' can't be used: The model `gpt-4.1-nano` has been deprecated"
            )
        );
    }

    #[tokio::test]
    async fn only_a_404_blocks_a_save() {
        for status in [200, 400, 401, 429, 500, 503] {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock("POST", "/chat/completions")
                .with_status(status)
                .with_body("{}")
                .create_async()
                .await;
            let reason =
                openai_model_refusal(&reqwest::Client::new(), &server.url(), "k", "gpt-4o-mini")
                    .await;
            assert_eq!(reason, None, "status {status} must not block a save");
        }
    }

    #[tokio::test]
    async fn an_unreachable_provider_does_not_block_a_save() {
        // Port 9 (discard) on loopback: the connection is refused immediately.
        let reason = openai_model_refusal(
            &reqwest::Client::new(),
            "http://127.0.0.1:9",
            "k",
            "gpt-4o-mini",
        )
        .await;
        assert_eq!(reason, None);
    }

    #[test]
    fn accepts_supported_provider() {
        assert!(
            validate(
                "anthropic",
                true,
                Some("claude-3-5-sonnet-20241022"),
                &None,
                false
            )
            .is_ok()
        );
        assert!(validate("openai", true, Some("gpt-4o-mini"), &None, false).is_ok());
    }

    #[test]
    fn accepts_custom_provider() {
        // A registered custom provider (membership resolved by the caller via the DB)
        // validates just like a built-in.
        assert!(validate("deepseek", true, Some("deepseek-chat"), &None, false).is_ok());
    }

    #[test]
    fn accepts_no_model_when_tiers_set() {
        assert!(validate("anthropic", true, None, &None, true).is_ok());
    }

    #[test]
    fn rejects_no_model_and_no_tiers() {
        let err = validate("openai", true, None, &None, false).unwrap_err();
        assert!(err.contains("either model or at least one tier model"));
    }

    #[test]
    fn rejects_unsupported_provider() {
        // Not a built-in and not registered (caller resolved membership to false).
        let err = validate("cohere", false, Some("command-r"), &None, false).unwrap_err();
        assert!(err.contains("unsupported provider"));
    }

    #[test]
    fn rejects_empty_model() {
        let err = validate("openai", true, Some("   "), &None, false).unwrap_err();
        assert!(err.contains("model must not be empty"));
    }

    #[test]
    fn rejects_empty_pinned_model() {
        let err = validate(
            "openai",
            true,
            Some("gpt-4o"),
            &Some("  ".to_string()),
            false,
        )
        .unwrap_err();
        assert!(err.contains("pinned_model must not be empty"));
    }
}
