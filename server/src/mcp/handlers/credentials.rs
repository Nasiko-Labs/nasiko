//! Per-user bearer/basic/url_param credentials for MCP connectors (write-only).

use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::super::openapi::McpEnvelope;
use super::super::{ApiError, ApiResponse, AppJson, AppPath, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterCredential {
    pub value: String,
}

/// `POST /api/mcp/connectors/{id}/credential` — store the caller's credential.
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/{id}/credential",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    request_body = RegisterCredential,
    responses(
        (status = 201, description = "Credential stored — `data.connected` says whether verification succeeded", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn register(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
    AppJson(body): AppJson<RegisterCredential>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let view = service::credentials::register(&state, user_id, connector_id, &body.value).await?;
    // The credential is always stored regardless of outcome (see
    // register_credential's doc comment) — the message just reflects whether
    // it was actually proven to work, not merely accepted.
    let message = if view["connected"].as_bool().unwrap_or(false) {
        "Credential registered and verified successfully"
    } else {
        "Credential stored, but verification failed — see the error field"
    };
    Ok(ApiResponse::created(view, message))
}

/// `GET /api/mcp/connectors/{id}/credential/status` — whether a credential exists.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/{id}/credential/status",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Credential status — `data` is `{connector_id, name, connected, auth_type}`", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::credentials::status(&state, user_id, connector_id).await?,
        "Credential status retrieved successfully",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/credential` — remove the caller's credential.
#[utoipa::path(
    delete,
    path = "/api/mcp/connectors/{id}/credential",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Credential deleted", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::credentials::delete(&state, user_id, connector_id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Credential deleted successfully",
    ))
}
