//! Catalog + platform Composio connector registration.

use axum::{Json, extract::{Path, State}};
use serde::Deserialize;
use uuid::Uuid;

use super::super::{ApiError, ApiResponse, ensure_admin, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/catalog` — connectable services, credential-free.
pub async fn get_catalog(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<ApiResponse, ApiError> {
    let user = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::catalog::get_catalog(&state, user).await?,
        "Catalog retrieved successfully",
    ))
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
) -> Result<ApiResponse, ApiError> {
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
    Ok(ApiResponse::created(view, "Auth config created successfully"))
}

/// `GET /api/mcp/auth-configs` — list platform Composio connectors (admin).
pub async fn list_auth_configs(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<ApiResponse, ApiError> {
    ensure_admin(&claims)?;
    Ok(ApiResponse::ok(
        service::catalog::list_composio(&state).await?,
        "Auth configs retrieved successfully",
    ))
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
) -> Result<ApiResponse, ApiError> {
    ensure_admin(&claims)?;
    let meta = service::catalog::ComposioMetadata {
        display_name: body.display_name,
        logo_url: body.logo_url,
        description: body.description,
    };
    Ok(ApiResponse::ok(
        service::catalog::update_composio(&state, connector_id, meta).await?,
        "Auth config updated successfully",
    ))
}

/// `DELETE /api/mcp/auth-configs/{connector_id}` — remove a composio connector (admin).
pub async fn delete_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(connector_id): Path<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_admin(&claims)?;
    service::catalog::delete_composio(&state, connector_id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Auth config deleted successfully"))
}
