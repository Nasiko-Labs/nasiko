//! Per-user bearer/basic/url_param credentials for MCP servers.
//!
//! `credential_value` is encrypted with `SecretsCrypto::for_user` and is
//! write-only — never returned by any endpoint.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::{McpError, repo};
use nasiko_secrets::SecretsCrypto;

use super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct RegisterCredential {
    pub credential_value: String,
    #[serde(default = "default_bearer")]
    pub credential_type: String,
}

fn default_bearer() -> String {
    "bearer".to_string()
}

/// Load a server and confirm the caller may register a credential for it
/// (platform servers: any authed user; user-scoped: the owner only).
async fn authorize_server(
    state: &AppState,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<repo::McpServer, ApiError> {
    let server = repo::get_mcp_server_by_id(&state.mcp.db, server_id)
        .await?
        .ok_or_else(|| ApiError(McpError::NotFound(format!("MCP server '{server_id}' not found"))))?;
    if !server.is_platform && server.user_id != Some(user_id) {
        return Err(ApiError(McpError::Forbidden("this server does not belong to you".into())));
    }
    Ok(server)
}

/// `POST /api/mcp/servers/{id}/credential` — store the caller's credential.
pub async fn register(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
    Json(body): Json<RegisterCredential>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = authorize_server(&state, user_id, server_id).await?;

    if !matches!(server.auth_type.as_str(), "bearer" | "basic" | "url_param") {
        return Err(ApiError(McpError::BadRequest(format!(
            "credential registration is only for bearer/basic/url_param servers, not '{}'",
            server.auth_type
        ))));
    }

    // Normalize the credential the same way the PoC's connect flow did, so the
    // session resolver can inject it verbatim.
    let value = normalize_for(&server, &body.credential_value);

    // Encrypt with the user-scoped key, then upsert.
    let encrypted = SecretsCrypto::for_user(user_id)
        .encrypt(&value)
        .map_err(|e| McpError::Internal(format!("encrypt credential: {e}")))?;
    repo::upsert_user_credential(
        &state.mcp.db,
        server.id,
        user_id,
        &body.credential_type,
        &encrypted,
    )
    .await?;

    tracing::info!(server = %server.name, %user_id, "registered user credential");
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "server_id": server.id,
            "server_name": server.name,
            "connected": true,
            "credential_type": body.credential_type,
        })),
    ))
}

/// `GET /api/mcp/servers/{id}/credential/status` — whether a credential exists
/// (value never returned).
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = authorize_server(&state, user_id, server_id).await?;
    let cred = repo::get_user_credential(&state.mcp.db, server_id, user_id).await?;
    Ok(Json(json!({
        "server_id": server.id,
        "server_name": server.name,
        "connected": cred.is_some(),
        "credential_type": cred.map(|c| c.credential_type),
    })))
}

/// `DELETE /api/mcp/servers/{id}/credential` — remove the caller's credential.
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = authorize_server(&state, user_id, server_id).await?;
    if !repo::delete_user_credential(&state.mcp.db, server_id, user_id).await? {
        return Err(ApiError(McpError::NotFound("no credential to delete".into())));
    }
    // Drop the cached session so the removed credential stops being injected.
    nasiko_mcp_gateway::session::invalidate_session_cache(&state.mcp, user_id).await;
    tracing::info!(server = %server.name, %user_id, "deleted user credential");
    Ok(StatusCode::NO_CONTENT)
}

/// Apply the PoC's credential normalization: auto-prefix `Bearer `/`Basic ` for
/// the standard Authorization header, base64-encode basic `user:pass`, and leave
/// url_param / custom-header raw. Shared with the unified connect flow.
pub(crate) fn normalize_for(server: &repo::McpServer, raw: &str) -> String {
    let header = server.credential_header_name.as_deref().unwrap_or("Authorization");
    let lower = raw.to_ascii_lowercase();
    match server.auth_type.as_str() {
        "bearer" if header.eq_ignore_ascii_case("Authorization") && !lower.starts_with("bearer ") => {
            format!("Bearer {raw}")
        }
        "basic" if !lower.starts_with("basic ") => {
            // Accept either a raw "user:pass" or an already-encoded blob.
            if raw.contains(':') {
                format!("Basic {}", B64.encode(raw))
            } else {
                format!("Basic {raw}")
            }
        }
        // url_param and custom-header credentials are stored raw.
        _ => raw.to_string(),
    }
}
