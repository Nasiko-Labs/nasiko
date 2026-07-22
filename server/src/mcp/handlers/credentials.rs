//! Per-user bearer/basic/url_param credentials for MCP connectors (write-only).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::super::{ApiError, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct RegisterCredential {
    pub value: String,
}

/// `POST /api/mcp/connectors/{id}/credential` — store the caller's credential.
pub async fn register(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
    Json(body): Json<RegisterCredential>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    let view = service::credentials::register(&state, user_id, connector_id, &body.value).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `GET /api/mcp/connectors/{id}/credential/status` — whether a credential exists.
pub async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(Json(
        service::credentials::status(&state, user_id, connector_id).await?,
    ))
}

/// `DELETE /api/mcp/connectors/{id}/credential` — remove the caller's credential.
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::credentials::delete(&state, user_id, connector_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
