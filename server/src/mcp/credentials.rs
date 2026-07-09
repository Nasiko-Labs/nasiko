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
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use nasiko_mcp_gateway::credentials;

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

/// `POST /api/mcp/servers/{id}/credential` — store the caller's credential.
pub async fn register(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
    Json(body): Json<RegisterCredential>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = credentials::authorize_server(&state.mcp, user_id, server_id).await?;

    credentials::register_credential(&state.mcp, user_id, &server, &body.credential_type, &body.credential_value)
        .await?;

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
    let server = credentials::authorize_server(&state.mcp, user_id, server_id).await?;
    let credential_type = credentials::credential_status(&state.mcp, server_id, user_id).await?;
    Ok(Json(json!({
        "server_id": server.id,
        "server_name": server.name,
        "connected": credential_type.is_some(),
        "credential_type": credential_type,
    })))
}

/// `DELETE /api/mcp/servers/{id}/credential` — remove the caller's credential.
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(server_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let server = credentials::authorize_server(&state.mcp, user_id, server_id).await?;
    credentials::delete_credential(&state.mcp, &server, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
