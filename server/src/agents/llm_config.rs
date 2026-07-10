use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

/// Outbound providers the LLM router can translate to, and the inbound SDK formats it
/// can parse — used to validate `llm-config` writes.
const SUPPORTED_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];
const SUPPORTED_INBOUND_FORMATS: [&str; 3] = ["openai", "anthropic", "gemini"];

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/{id}/llm-config",
        get(get_llm_config).patch(update_llm_config),
    )
}

/// Resolve the agent's owner, enforcing owner-only (superuser override) access for
/// llm-config read/write. `Err` is a ready-to-return response (404 unknown / 403 not owner).
async fn agent_owner_or_reject(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    user_id: Uuid,
    is_superuser: bool,
) -> Result<Uuid, axum::response::Response> {
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match owner {
        None => Err((StatusCode::NOT_FOUND, "agent not found").into_response()),
        Some(o) if o != user_id && !is_superuser => {
            Err((StatusCode::FORBIDDEN, "not the agent owner").into_response())
        }
        Some(o) => Ok(o),
    }
}

// ─── PATCH /{id}/llm-config ──────────────────────────────────────────────────

/// Self-service LLM routing config for an agent (P2.6). Sets the `agents.llm_config`
/// JSONB (provider/model/fallbacks/tuning/secret) and, optionally, `inbound_format`.
/// Owner-only (or superuser); the gateway routes off this on the next request (≤ cache TTL).
#[derive(Debug, Deserialize)]
pub struct UpdateLlmConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// Name of the caller's `user_secrets` row holding the provider API key. None ⇒ the
    /// platform-key path (see resolver §6.5).
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
    /// Optionally also change which SDK the agent's code speaks (drives deploy injection).
    #[serde(default)]
    pub inbound_format: Option<String>,
    /// Compliance lock: pin routing so the smart model router never re-selects. The pinned
    /// model is `pinned_model` if set, else `model`.
    #[serde(default)]
    pub pinned: bool,
    /// The model to pin to when `pinned`. `None` ⇒ pin to `model`.
    #[serde(default)]
    pub pinned_model: Option<String>,
}

/// Validate the provider/model/inbound_format fields (everything that doesn't need the DB).
fn validate_llm_config(req: &UpdateLlmConfigRequest) -> Result<(), String> {
    if !SUPPORTED_PROVIDERS.contains(&req.provider.as_str()) {
        return Err(format!(
            "unsupported provider '{}' (expected one of: {})",
            req.provider,
            SUPPORTED_PROVIDERS.join(", ")
        ));
    }
    if req.model.trim().is_empty() {
        return Err("model must not be empty".to_string());
    }
    if let Some(fmt) = &req.inbound_format
        && !SUPPORTED_INBOUND_FORMATS.contains(&fmt.as_str())
    {
        return Err(format!(
            "unsupported inbound_format '{fmt}' (expected one of: {})",
            SUPPORTED_INBOUND_FORMATS.join(", ")
        ));
    }
    // A pinned_model, when given, must be non-empty (otherwise it'd pin to nothing).
    if let Some(pm) = &req.pinned_model
        && pm.trim().is_empty()
    {
        return Err("pinned_model must not be empty".to_string());
    }
    Ok(())
}

/// `GET /{id}/llm-config` — current routing config + inbound format (owner/superuser).
async fn get_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };
    if let Err(resp) = agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        return resp;
    }

    let row: Option<(Option<serde_json::Value>, String)> = sqlx::query_as(
        "SELECT llm_config, inbound_format FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match row {
        Some((llm_config, inbound_format)) => (
            StatusCode::OK,
            Json(json!({
                "agent_id": agent_id,
                "llm_config": llm_config,           // null ⇒ backward-compat defaults apply
                "inbound_format": inbound_format,
            })),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "agent not found").into_response(),
    }
}

async fn update_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(req): Json<UpdateLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    // Owner-only mutation (superuser may override). Read access (public/grant) is NOT
    // enough to edit routing config.
    let owner = match agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    if let Err(msg) = validate_llm_config(&req) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    // A referenced secret must exist for this owner (resolver would otherwise 400 at call
    // time). Validate against the agent owner's secrets, not the (possibly superuser) caller.
    if let Some(name) = req.api_key_secret_name.as_deref().filter(|s| !s.is_empty()) {
        let exists: bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM user_secrets WHERE user_id = $1 AND name = $2)",
        )
        .bind(owner)
        .bind(name)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
        if !exists {
            return (
                StatusCode::BAD_REQUEST,
                format!("secret '{name}' not found for the agent owner"),
            )
                .into_response();
        }
    }

    // Build the llm_config JSONB exactly as the resolver's LLMConfig deserializes it.
    let llm_config = json!({
        "provider": req.provider,
        "model": req.model,
        "fallback_models": req.fallback_models,
        "temperature": req.temperature,
        "max_tokens": req.max_tokens,
        "api_key_secret_name": req.api_key_secret_name,
        "pinned": req.pinned,
        "pinned_model": req.pinned_model,
    });

    let result = if let Some(fmt) = &req.inbound_format {
        sqlx::query(
            "UPDATE agents SET llm_config = $2, inbound_format = $3, updated_at = now() WHERE id = $1",
        )
        .bind(agent_id)
        .bind(&llm_config)
        .bind(fmt)
        .execute(&state.db)
        .await
    } else {
        sqlx::query("UPDATE agents SET llm_config = $2, updated_at = now() WHERE id = $1")
            .bind(agent_id)
            .bind(&llm_config)
            .execute(&state.db)
            .await
    };

    match result {
        Ok(_) => {
            let mut body = json!({ "agent_id": agent_id, "llm_config": llm_config });
            if let Some(fmt) = &req.inbound_format {
                body["inbound_format"] = json!(fmt);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to update llm_config: {e}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(provider: &str, model: &str, inbound: Option<&str>) -> UpdateLlmConfigRequest {
        UpdateLlmConfigRequest {
            provider: provider.into(),
            model: model.into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            inbound_format: inbound.map(str::to_string),
            pinned: false,
            pinned_model: None,
        }
    }

    #[test]
    fn accepts_supported_provider_and_format() {
        assert!(validate_llm_config(&req("anthropic", "claude-3-5-sonnet-20241022", Some("gemini"))).is_ok());
        assert!(validate_llm_config(&req("openai", "gpt-4o-mini", None)).is_ok());
    }

    #[test]
    fn rejects_unsupported_provider() {
        let err = validate_llm_config(&req("cohere", "command-r", None)).unwrap_err();
        assert!(err.contains("unsupported provider"));
    }

    #[test]
    fn rejects_empty_model() {
        let err = validate_llm_config(&req("openai", "   ", None)).unwrap_err();
        assert!(err.contains("model must not be empty"));
    }

    #[test]
    fn rejects_unsupported_inbound_format() {
        let err = validate_llm_config(&req("openai", "gpt-4o", Some("crewai"))).unwrap_err();
        assert!(err.contains("unsupported inbound_format"));
    }

    #[test]
    fn accepts_pinning() {
        let mut r = req("anthropic", "claude-3-5-sonnet-20241022", None);
        r.pinned = true;
        r.pinned_model = Some("claude-3-5-sonnet-20241022".into());
        assert!(validate_llm_config(&r).is_ok());
    }

    #[test]
    fn rejects_empty_pinned_model() {
        let mut r = req("openai", "gpt-4o", None);
        r.pinned_model = Some("  ".into());
        let err = validate_llm_config(&r).unwrap_err();
        assert!(err.contains("pinned_model must not be empty"));
    }
}
