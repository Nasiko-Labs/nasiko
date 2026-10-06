use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{Duration, Utc};
use nasiko_llm_router::{
    GatewayConfig,
    auth::{mint_agent_token, parse_algorithm},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::Claims;
use crate::mcp::ApiResponse;
use crate::state::AppState;

/// The inbound SDK formats the LLM router can parse — used to validate `inbound_format`.
const SUPPORTED_INBOUND_FORMATS: [&str; 3] = ["openai", "anthropic", "gemini"];
const LOCAL_ROUTING_TOKEN_TTL_SECONDS: u64 = 10 * 60;

/// The `llm_configs` columns the resolver reads, assembled by Postgres into one JSON object.
const CONFIG_JSON: &str = "json_build_object(\
     'id', id, 'name', name, 'provider', provider, 'model', model, \
     'fallback_models', fallback_models, 'temperature', temperature, \
     'max_tokens', max_tokens, 'api_key_secret_name', api_key_secret_name, \
     'pinned', pinned, 'pinned_model', pinned_model, \
     'tier1_model', tier1_model, 'tier2_model', tier2_model, 'tier3_model', tier3_model, \
     'is_default', is_default)";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/{id}/llm-config",
            get(get_llm_config)
                .patch(update_llm_config)
                .delete(delete_llm_config),
        )
        .route("/{id}/llm-token", post(issue_llm_token))
        .route(
            "/{id}/mcp-token",
            post(issue_mcp_token).delete(revoke_mcp_token),
        )
}

/// Resolve the agent's owner, enforcing owner-only (superuser override) access for
/// llm-config read/write and the mcp-token DELETE. `Err` is a ready-to-return response
/// (404 unknown / 403 not owner).
///
/// A lookup failure is a 500, not a 404: a DB blip must not tell the caller
/// "no such agent" when the real answer is "couldn't check".
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
            .map_err(|error| {
                tracing::error!(%error, %agent_id, "agent owner lookup failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve agent owner",
                )
                    .into_response()
            })?;
    match owner {
        None => Err((StatusCode::NOT_FOUND, "agent not found").into_response()),
        Some(o) if o != user_id && !is_superuser => {
            Err((StatusCode::FORBIDDEN, "not the agent owner").into_response())
        }
        Some(o) => Ok(o),
    }
}

/// Owner-only, no superuser override: rejects every non-owner outright,
/// including a superuser, who [`agent_owner_or_reject`] alone would let
/// through. Credential-issuing endpoints (LLM routing token, on-demand MCP
/// gateway token) need this: a superuser can administer the agent but must
/// not be able to mint a credential that impersonates it.
async fn require_owner(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    user_id: Uuid,
) -> Result<(), axum::response::Response> {
    agent_owner_or_reject(db, agent_id, user_id, false).await?;
    Ok(())
}

#[derive(Serialize, ToSchema)]
pub(crate) struct LlmTokenResponse {
    token: String,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct LlmTokenEnvelope {
    data: LlmTokenResponse,
    status_code: u16,
    message: String,
}

/// Issue a short-lived agent identity token for a local SDK process. The caller
/// must own the agent; the signing secret never leaves the control plane.
#[utoipa::path(
    post,
    path = "/api/agents/{id}/llm-token",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 200, description = "Short-lived LLM routing token", body = LlmTokenEnvelope),
        (status = 403, description = "Caller is not the agent owner"),
        (status = 404, description = "No such agent"),
        (status = 503, description = "LLM router authentication is not configured"),
    ),
)]
pub(crate) async fn issue_llm_token(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(resp) = require_owner(&state.db, agent_id, user_id).await {
        return resp;
    }

    let cfg = GatewayConfig::from_env();
    if cfg.agent_jwt_secret.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_JWT_SECRET is not configured",
        )
            .into_response();
    }
    let token = match mint_agent_token(
        &agent_id.to_string(),
        &user_id.to_string(),
        &cfg.agent_jwt_secret,
        LOCAL_ROUTING_TOKEN_TTL_SECONDS,
        parse_algorithm(&cfg.agent_jwt_algorithm),
    ) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, %agent_id, "failed to mint local LLM routing token");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to issue LLM routing token",
            )
                .into_response();
        }
    };
    let expires_at = Utc::now() + Duration::seconds(LOCAL_ROUTING_TOKEN_TTL_SECONDS as i64);
    (
        [(header::CACHE_CONTROL, "no-store")],
        ApiResponse::ok(
            json!(LlmTokenResponse { token, expires_at }),
            "LLM routing token issued successfully",
        ),
    )
        .into_response()
}

#[derive(Serialize, ToSchema)]
pub(crate) struct McpTokenResponse {
    /// The plaintext gateway credential, returned exactly once — store it,
    /// the control plane keeps only its hash.
    token: String,
    /// The platform's configured public gateway URL, or `null` when unset.
    gateway_url: Option<String>,
    /// `{gateway_url}/s/{token}` — the credential-bearing URL form for MCP
    /// clients that accept only a `url`, or `null` when `gateway_url` is
    /// unset. This URL is itself a secret: handle it exactly like `token`.
    connect_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct McpTokenEnvelope {
    data: McpTokenResponse,
    status_code: u16,
    message: String,
}

/// Mint (rotate) a local process's MCP gateway credential.
///
/// The same credential a deployed container receives as `MCP_GATEWAY_TOKEN`
/// at deploy time (`mcp::wiring::inject_agent_gateway_token`), minted here on
/// demand for a local coding agent (Claude Code / Codex / OpenCode, bound via
/// `nasiko connect`), which is never deployed and so never receives one
/// automatically. Owner-only, and restricted to CLI-bound rows
/// (`coding_agent_integration_id` set): minting for a deployed agent would
/// leave its running container(s) holding a token the row stops recognizing
/// once the rotation grace window lapses, with no way to hand them the new
/// one short of a redeploy. Rotation keeps that same grace window
/// (`nasiko_mcp_gateway::agent_tokens::ROTATION_GRACE_SECS`): the previous
/// token keeps authenticating for a while after a re-mint, so POST alone does
/// not immediately cut off a leaked credential — use `DELETE` for that.
#[utoipa::path(
    post,
    path = "/api/agents/{id}/mcp-token",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 200, description = "MCP gateway credential (plaintext, shown once)", body = McpTokenEnvelope),
        (status = 403, description = "Caller is not the agent owner"),
        (status = 404, description = "No such agent"),
        (status = 409, description = "Not a CLI-bound coding-agent row — a deployed agent's credential rotates on redeploy instead"),
    ),
)]
pub(crate) async fn issue_mcp_token(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(resp) = require_owner(&state.db, agent_id, user_id).await {
        return resp;
    }

    match is_coding_agent(&state.db, agent_id).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::CONFLICT,
                "not a local coding agent: a deployed agent's credential rotates on redeploy",
            )
                .into_response();
        }
        Err(resp) => return resp,
    }

    let token = match nasiko_mcp_gateway::agent_tokens::mint(&state.db, agent_id).await {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, %agent_id, "failed to mint MCP gateway token");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to issue MCP gateway token",
            )
                .into_response();
        }
    };
    let gateway_url = state
        .config
        .mcp_gateway_public_url
        .as_deref()
        .map(|url| url.trim_end_matches('/').to_string());
    let connect_url = gateway_url
        .as_deref()
        .map(|url| nasiko_mcp_gateway::injector::connect_url(url, &token));
    (
        [(header::CACHE_CONTROL, "no-store")],
        ApiResponse::ok(
            json!(McpTokenResponse {
                token,
                gateway_url,
                connect_url,
            }),
            "MCP gateway token issued successfully",
        ),
    )
        .into_response()
}

/// `Ok(true)`/`Ok(false)` for whether the (already ownership-checked) agent
/// is a CLI-bound coding-agent row; `Err` is a ready-to-return 500 on a
/// lookup failure. Deliberately keyed on `coding_agent_integration_id IS NOT
/// NULL` — the same column `oss/llm-router/src/resolver/mod.rs` and the MCP
/// gateway's own rule 3b (`oss/server/src/mcp/handlers/gateway.rs`) read —
/// never agent metadata or name, so all three agree on which rows this
/// policy covers.
async fn is_coding_agent(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> Result<bool, axum::response::Response> {
    sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_integration_id IS NOT NULL FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await
    .map_err(|error| {
        tracing::error!(%error, %agent_id, "coding-agent lookup failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to check agent type",
        )
            .into_response()
    })
}

/// Revoke the agent's MCP gateway credential.
///
/// Kills every gateway credential of the agent — the live one and any
/// grace-window predecessor (`nasiko_mcp_gateway::agent_tokens::revoke`) —
/// immediately, with none of POST's rotation grace. A superuser may call this
/// on any agent: revoking grants nothing, so unlike minting it cannot
/// impersonate the agent. Idempotent: revoking an agent with no live
/// credential still answers 204. For a deployed agent this leaves it 401ing
/// at `/api/mcp` until its next redeploy re-mints a credential — the
/// documented recovery.
#[utoipa::path(
    delete,
    path = "/api/agents/{id}/mcp-token",
    tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    responses(
        (status = 204, description = "Credential revoked"),
        (status = 403, description = "Caller is neither the agent owner nor a superuser"),
        (status = 404, description = "No such agent"),
    ),
)]
pub(crate) async fn revoke_mcp_token(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(resp) =
        agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        return resp;
    }

    match nasiko_mcp_gateway::agent_tokens::revoke(&state.db, agent_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(%error, %agent_id, "failed to revoke MCP gateway token");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke MCP gateway token",
            )
                .into_response()
        }
    }
}

/// Resolve the config an agent routes through: attached (`agents.llm_config_id`) → the owner's
/// default (`llm_configs.is_default`) → none. Mirrors `nasiko_llm_router`'s resolver so the API
/// shows exactly what the router will use. Returns the config JSON and its source label.
async fn resolve_agent_config(
    db: &sqlx::PgPool,
    attached: Option<Uuid>,
    owner: Uuid,
) -> (Option<Value>, &'static str) {
    if let Some(cid) = attached {
        let cfg: Option<Value> = sqlx::query_scalar::<_, Value>(&format!(
            "SELECT {CONFIG_JSON} FROM llm_configs WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(cid)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
        if let Some(cfg) = cfg {
            return (Some(cfg), "attached");
        }
    }
    let default: Option<Value> = sqlx::query_scalar::<_, Value>(&format!(
        "SELECT {CONFIG_JSON} FROM llm_configs \
         WHERE created_by = $1 AND is_default AND deleted_at IS NULL"
    ))
    .bind(owner)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match default {
        Some(cfg) => (Some(cfg), "owner-default"),
        None => (None, "none"),
    }
}

// ─── GET /{id}/llm-config ─────────────────────────────────────────────────────

/// Response envelope for `GET /{id}/llm-config` — documents the shape of the
/// ad hoc `serde_json::json!` object the handler returns.
#[derive(Serialize, ToSchema)]
pub(crate) struct LlmConfigResponse {
    agent_id: Uuid,
    /// Which config is attached; `null` ⇒ owner default / none.
    llm_config_id: Option<Uuid>,
    /// The resolved config the router will actually use, or `null`.
    llm_config: Option<Value>,
    /// `"attached"` | `"owner-default"` | `"none"`.
    source: String,
    inbound_format: String,
    /// Agent-level model pin, overriding the config's own `pinned_model`, or `null`.
    pinned_model: Option<String>,
}

/// `crate::mcp::ApiResponse` envelope around [`LlmConfigResponse`].
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct LlmConfigEnvelope {
    data: LlmConfigResponse,
    status_code: u16,
    message: String,
}

/// The agent's **resolved** routing config (attached → owner default →
/// none), which config is attached, its source, and the inbound format. Owner-or-superuser only.
#[utoipa::path(
    get,
    path = "/api/agents/{id}/llm-config",
    tag = "agents",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    responses(
        (status = 200, description = "Resolved LLM routing config", body = LlmConfigEnvelope),
        (status = 403, description = "Caller is not the agent owner"),
        (status = 404, description = "No such agent"),
    ),
)]
pub(crate) async fn get_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let owner = match agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    let row: Option<(Option<Uuid>, String, Option<String>)> = sqlx::query_as(
        "SELECT llm_config_id, inbound_format, pinned_model FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((attached, inbound_format, agent_pin)) = row else {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    };
    let (config, source) = resolve_agent_config(&state.db, attached, owner).await;
    ApiResponse::ok(
        json!({
            "agent_id": agent_id,
            "llm_config_id": attached,
            "llm_config": config,
            "source": source,
            "inbound_format": inbound_format,
            "pinned_model": agent_pin,
        }),
        "Agent LLM config retrieved successfully",
    )
    .into_response()
}

// ─── PATCH /{id}/llm-config ───────────────────────────────────────────────────

/// Deserialize any *present* value (including `null`) into `Some`, so an absent field stays
/// `None`. This is what makes the double-option below distinguish "field omitted" from
/// "field set to null" — plain `#[serde(default)]` collapses both to `None`.
fn deserialize_present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Attach/detach a reusable LLM config to an agent, and optionally change the inbound format.
/// A config can only be attached if it belongs to the agent owner (per-user ownership keeps the
/// referenced secret in the owner's store). Owner-only (or superuser).
#[derive(Debug, Deserialize, ToSchema)]
pub struct AttachLlmConfigRequest {
    /// Double-option distinguishes the three cases: absent ⇒ leave unchanged; `null` ⇒ detach
    /// (fall back to the owner's default); a UUID ⇒ attach that config.
    #[serde(default, deserialize_with = "deserialize_present")]
    #[schema(value_type = Option<Uuid>)]
    pub llm_config_id: Option<Option<Uuid>>,
    /// Optionally change which SDK the agent's code speaks (drives deploy injection).
    #[serde(default)]
    pub inbound_format: Option<String>,
    /// Agent-level model pin. Overrides the config's `pinned_model`. Double-option:
    /// absent ⇒ leave unchanged; `null` ⇒ clear pin; string ⇒ set pin.
    #[serde(default, deserialize_with = "deserialize_present")]
    pub pinned_model: Option<Option<String>>,
}

/// Response envelope for `PATCH /{id}/llm-config` — documents the shape of
/// the ad hoc `serde_json::json!` object the handler returns.
#[derive(Serialize, ToSchema)]
pub(crate) struct LlmConfigUpdateResponse {
    agent_id: Uuid,
    llm_config_id: Option<Uuid>,
    llm_config: Option<Value>,
    source: String,
    /// Agent-level model pin after applying this update, or `null`.
    pinned_model: Option<String>,
}

/// `crate::mcp::ApiResponse` envelope around [`LlmConfigUpdateResponse`].
#[derive(Serialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct LlmConfigUpdateEnvelope {
    data: LlmConfigUpdateResponse,
    status_code: u16,
    message: String,
}

/// Attach/detach a reusable LLM config to an agent, and optionally change the
/// inbound SDK format. A config can only be attached if it belongs to the
/// agent owner. Owner-or-superuser only.
#[utoipa::path(
    patch,
    path = "/api/agents/{id}/llm-config",
    tag = "agents",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    request_body = AttachLlmConfigRequest,
    responses(
        (status = 200, description = "Updated, with the freshly resolved config", body = LlmConfigUpdateEnvelope),
        (status = 400, description = "Config not found/not owned, or unsupported inbound_format"),
        (status = 403, description = "Caller is not the agent owner"),
        (status = 404, description = "No such agent"),
    ),
)]
pub(crate) async fn update_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(req): Json<AttachLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    // Owner-only mutation (superuser may override). Read access is NOT enough to change routing.
    let owner = match agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    // Attach / detach. A config must be owned by the AGENT owner (not the caller) so its secret
    // resolves from the same store the router reads.
    match req.llm_config_id {
        Some(Some(config_id)) => {
            let ok: bool = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM llm_configs \
                 WHERE id = $1 AND created_by = $2 AND deleted_at IS NULL)",
            )
            .bind(config_id)
            .bind(owner)
            .fetch_one(&state.db)
            .await
            .unwrap_or(false);
            if !ok {
                return (
                    StatusCode::BAD_REQUEST,
                    "llm config not found or not owned by the agent owner",
                )
                    .into_response();
            }
            // Config change clears the agent-level pin (it was set in the old config's context).
            if let Err(e) = sqlx::query(
                "UPDATE agents SET llm_config_id = $2, pinned_model = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .bind(config_id)
            .execute(&state.db)
            .await
            {
                return db_error("attach", e);
            }
        }
        Some(None) => {
            // Detach clears the agent-level pin too.
            if let Err(e) = sqlx::query(
                "UPDATE agents SET llm_config_id = NULL, pinned_model = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .execute(&state.db)
            .await
            {
                return db_error("detach", e);
            }
        }
        None => {}
    }

    // Agent-level pin: absent ⇒ leave unchanged; null ⇒ clear; string ⇒ set.
    // Only applied when the config was NOT just changed (config change already clears it).
    if req.llm_config_id.is_none() {
        match &req.pinned_model {
            Some(Some(model)) => {
                if model.trim().is_empty() {
                    return (StatusCode::BAD_REQUEST, "pinned_model must not be empty")
                        .into_response();
                }
                if let Err(e) = sqlx::query(
                    "UPDATE agents SET pinned_model = $2, updated_at = now() WHERE id = $1",
                )
                .bind(agent_id)
                .bind(model)
                .execute(&state.db)
                .await
                {
                    return db_error("set pinned_model", e);
                }
            }
            Some(None) => {
                if let Err(e) = sqlx::query(
                    "UPDATE agents SET pinned_model = NULL, updated_at = now() WHERE id = $1",
                )
                .bind(agent_id)
                .execute(&state.db)
                .await
                {
                    return db_error("clear pinned_model", e);
                }
            }
            None => {}
        }
    }

    if let Some(fmt) = req.inbound_format.as_deref() {
        if !SUPPORTED_INBOUND_FORMATS.contains(&fmt) {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "unsupported inbound_format '{fmt}' (expected one of: {})",
                    SUPPORTED_INBOUND_FORMATS.join(", ")
                ),
            )
                .into_response();
        }
        if let Err(e) =
            sqlx::query("UPDATE agents SET inbound_format = $2, updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .bind(fmt)
                .execute(&state.db)
                .await
        {
            return db_error("set inbound_format", e);
        }
    }

    // Return the freshly resolved config so the caller sees the effect immediately.
    let row: Option<(Option<Uuid>, Option<String>)> =
        sqlx::query_as("SELECT llm_config_id, pinned_model FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let (attached, agent_pin) = row.unwrap_or((None, None));
    let (config, source) = resolve_agent_config(&state.db, attached, owner).await;
    ApiResponse::ok(
        json!({
            "agent_id": agent_id,
            "llm_config_id": attached,
            "llm_config": config,
            "source": source,
            "pinned_model": agent_pin,
        }),
        "Agent LLM config updated successfully",
    )
    .into_response()
}

// ─── DELETE /{id}/llm-config ──────────────────────────────────────────────────

/// Detach the config and clear the agent-level pin in one call. Owner-or-superuser only.
#[utoipa::path(
    delete,
    path = "/api/agents/{id}/llm-config",
    tag = "agents",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    responses(
        (status = 200, description = "Config detached and pin cleared", body = crate::openapi::EmptyEnvelope),
        (status = 403, description = "Caller is not the agent owner"),
        (status = 404, description = "No such agent"),
    ),
)]
pub(crate) async fn delete_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if let Err(resp) =
        agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        return resp;
    }
    if let Err(e) = sqlx::query(
        "UPDATE agents SET llm_config_id = NULL, pinned_model = NULL, updated_at = now() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .execute(&state.db)
    .await
    {
        return db_error("delete", e);
    }
    ApiResponse::ok(json!(null), "Agent LLM config removed successfully").into_response()
}

fn db_error(op: &str, e: sqlx::Error) -> axum::response::Response {
    tracing::error!(%e, op, "agent llm-config: db error");
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
    fn attach_request_distinguishes_absent_null_and_value() {
        // absent field ⇒ leave unchanged
        let absent: AttachLlmConfigRequest = serde_json::from_str("{}").unwrap();
        assert!(absent.llm_config_id.is_none());
        // explicit null ⇒ detach
        let null: AttachLlmConfigRequest =
            serde_json::from_str(r#"{"llm_config_id": null}"#).unwrap();
        assert!(matches!(null.llm_config_id, Some(None)));
        // a UUID ⇒ attach
        let val: AttachLlmConfigRequest =
            serde_json::from_str(r#"{"llm_config_id": "11111111-1111-1111-1111-111111111111"}"#)
                .unwrap();
        assert!(matches!(val.llm_config_id, Some(Some(_))));
    }

    #[test]
    fn inbound_format_allowlist() {
        assert!(SUPPORTED_INBOUND_FORMATS.contains(&"openai"));
        assert!(!SUPPORTED_INBOUND_FORMATS.contains(&"crewai"));
    }
}
