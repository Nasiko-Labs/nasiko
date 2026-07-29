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
use uuid::Uuid;

use crate::auth::Claims;
use crate::mcp::ApiResponse;
use crate::state::AppState;

/// Outbound providers the LLM router can translate to — used to validate config writes.
const SUPPORTED_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];

/// The `llm_configs` columns returned to clients, assembled by Postgres into one JSON object.
const CONFIG_JSON: &str = "json_build_object(\
     'id', id, 'name', name, 'provider', provider, 'model', model, \
     'fallback_models', fallback_models, 'temperature', temperature, \
     'max_tokens', max_tokens, 'api_key_secret_name', api_key_secret_name, \
     'pinned', pinned, 'pinned_model', pinned_model, \
     'tier1_model', tier1_model, 'tier2_model', tier2_model, 'tier3_model', tier3_model, \
     'is_default', is_default, \
     'created_at', created_at, 'updated_at', updated_at)";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/llm-configs", get(list).post(create))
        .route(
            "/llm-configs/{id}",
            get(get_one).patch(update).delete(delete_config),
        )
        .route("/llm-configs/{id}/default", post(set_default))
}

#[derive(Debug, Deserialize)]
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
    /// Mark this as the caller's default (clears any prior default).
    #[serde(default)]
    pub is_default: bool,
}

/// Partial update — every field is optional. Absent fields keep their current value.
/// `is_default` is not touched here — use `POST /llm-configs/{id}/default`.
#[derive(Debug, Deserialize)]
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
}

/// Validate the fields that don't need the DB. Shared by create/update.
///
/// `model` may be `None` when the user relies entirely on tier-based routing; it is required
/// only when no tier model is set (otherwise the router has no fallback).
fn validate(
    provider: &str,
    model: Option<&str>,
    pinned_model: &Option<String>,
    has_any_tier: bool,
) -> Result<(), String> {
    if !SUPPORTED_PROVIDERS.contains(&provider) {
        return Err(format!(
            "unsupported provider '{provider}' (expected one of: {})",
            SUPPORTED_PROVIDERS.join(", ")
        ));
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

async fn list(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
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

async fn create(
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
    let has_any_tier =
        req.tier1_model.is_some() || req.tier2_model.is_some() || req.tier3_model.is_some();
    if let Err(msg) = validate(
        &req.provider,
        req.model.as_deref(),
        &req.pinned_model,
        has_any_tier,
    ) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }
    if config_name_taken(&state.db, user_id, &req.name).await {
        return (
            StatusCode::CONFLICT,
            format!("an LLM config named '{}' already exists", req.name),
        )
            .into_response();
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
          is_default) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING id",
    )
    .bind(user_id)
    .bind(&req.name)
    .bind(&req.provider)
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
    .fetch_one(&mut *tx)
    .await;

    let id = match inserted {
        Ok((id,)) => id,
        Err(e) => return db_error("insert", e),
    };
    if let Err(e) = tx.commit().await {
        return db_error("commit", e);
    }

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

async fn get_one(
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

async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if fetch_config(&state.db, id, user_id).await.is_none() {
        return (StatusCode::NOT_FOUND, "llm config not found").into_response();
    }
    // Validate only the fields that are present.
    if let Some(provider) = &req.provider
        && !SUPPORTED_PROVIDERS.contains(&provider.as_str())
    {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unsupported provider '{provider}' (expected one of: {})",
                SUPPORTED_PROVIDERS.join(", ")
            ),
        )
            .into_response();
    }
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
         updated_at = now() \
         WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .bind(&req.name)
    .bind(&req.provider)
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
    .execute(&state.db)
    .await;

    if let Err(e) = result {
        return db_error("update", e);
    }
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::ok(cfg, "LLM config updated successfully").into_response(),
        None => (StatusCode::NOT_FOUND, "llm config not found").into_response(),
    }
}

// ─── DELETE /llm-configs/{id} ────────────────────────────────────────────────

async fn delete_config(
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
        Ok(_) => ApiResponse::ok(json!(null), "LLM config deleted successfully").into_response(),
        Err(e) => db_error("delete", e),
    }
}

// ─── POST /llm-configs/{id}/default ──────────────────────────────────────────

async fn set_default(
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
    match fetch_config(&state.db, id, user_id).await {
        Some(cfg) => ApiResponse::ok(cfg, "LLM config set as default successfully").into_response(),
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

    #[test]
    fn accepts_supported_provider() {
        assert!(
            validate(
                "anthropic",
                Some("claude-3-5-sonnet-20241022"),
                &None,
                false
            )
            .is_ok()
        );
        assert!(validate("openai", Some("gpt-4o-mini"), &None, false).is_ok());
    }

    #[test]
    fn accepts_no_model_when_tiers_set() {
        assert!(validate("anthropic", None, &None, true).is_ok());
    }

    #[test]
    fn rejects_no_model_and_no_tiers() {
        let err = validate("openai", None, &None, false).unwrap_err();
        assert!(err.contains("either model or at least one tier model"));
    }

    #[test]
    fn rejects_unsupported_provider() {
        let err = validate("cohere", Some("command-r"), &None, false).unwrap_err();
        assert!(err.contains("unsupported provider"));
    }

    #[test]
    fn rejects_empty_model() {
        let err = validate("openai", Some("   "), &None, false).unwrap_err();
        assert!(err.contains("model must not be empty"));
    }

    #[test]
    fn rejects_empty_pinned_model() {
        let err = validate("openai", Some("gpt-4o"), &Some("  ".to_string()), false).unwrap_err();
        assert!(err.contains("pinned_model must not be empty"));
    }
}
