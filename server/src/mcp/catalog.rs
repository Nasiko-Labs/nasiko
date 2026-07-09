//! Service catalog + platform Composio auth-config (toolkit) registration.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;

use nasiko_mcp_gateway::catalog::{self, CreateAuthConfigInput};

use super::{ApiError, ensure_admin};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/catalog` — connectable services, credential-free.
pub async fn get_catalog(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(catalog::get_catalog_view(&state.mcp).await?))
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

/// `POST /api/mcp/auth-configs` — register a platform Composio toolkit (admin).
/// Registers the OAuth app with Composio, then records it locally.
pub async fn create_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateAuthConfig>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_admin(&claims)?;

    let view = catalog::create_platform_auth_config(
        &state.mcp,
        CreateAuthConfigInput {
            toolkit: &body.toolkit,
            use_composio_managed: body.use_composio_managed,
            client_id: body.client_id.as_deref(),
            client_secret: body.client_secret.as_deref(),
            scopes: body.scopes.as_deref(),
            display_name: body.display_name.as_deref(),
            logo_url: body.logo_url.as_deref(),
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(view)))
}

/// `GET /api/mcp/auth-configs` — list platform toolkits.
pub async fn list_auth_configs(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(catalog::list_auth_configs_view(&state.mcp).await?))
}

/// `DELETE /api/mcp/auth-configs/{auth_config_id}` — remove a platform toolkit (admin).
pub async fn delete_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(auth_config_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_admin(&claims)?;
    catalog::delete_auth_config(&state.mcp, &auth_config_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
