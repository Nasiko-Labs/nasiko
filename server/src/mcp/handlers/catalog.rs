//! Catalog + platform Composio connector registration.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::super::{ApiError, ensure_admin, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/catalog` — connectable services, credential-free.
pub async fn get_catalog(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let user = parse_user(&claims)?;
    Ok(Json(service::catalog::get_catalog(&state, user).await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateAuthConfig {
    pub toolkit: String,
    #[serde(default = "default_true")]
    pub use_composio_managed: bool,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `POST /api/mcp/auth-configs` — register a platform Composio connector (admin).
pub async fn create_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateAuthConfig>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_admin(&claims)?;
    let view = service::catalog::create_composio(
        &state,
        &service::catalog::ComposioReg {
            toolkit: body.toolkit,
            use_composio_managed: body.use_composio_managed,
            client_id: body.client_id,
            client_secret: body.client_secret,
            scopes: body.scopes,
            display_name: body.display_name,
            logo_url: body.logo_url,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `GET /api/mcp/auth-configs` — list platform Composio connectors (admin).
/// Gated like its create/update/delete siblings on the same resource.
pub async fn list_auth_configs(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<Json<Value>, ApiError> {
    ensure_admin(&claims)?;
    Ok(Json(service::catalog::list_composio(&state).await?))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAuthConfig {
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub description: Option<String>,
}

/// `PATCH /api/mcp/auth-configs/{connector_id}` — edit composio catalog metadata (admin).
pub async fn update_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
    Json(body): Json<UpdateAuthConfig>,
) -> Result<Json<Value>, ApiError> {
    ensure_admin(&claims)?;
    let meta = service::catalog::ComposioMetadata {
        display_name: body.display_name,
        logo_url: body.logo_url,
        description: body.description,
    };
    Ok(Json(
        service::catalog::update_composio(&state, connector_id, meta).await?,
    ))
}

/// `DELETE /api/mcp/auth-configs/{connector_id}` — remove a composio connector (admin).
pub async fn delete_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_admin(&claims)?;
    service::catalog::delete_composio(&state, connector_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
