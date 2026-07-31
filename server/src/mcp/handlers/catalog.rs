//! Catalog + platform Composio connector registration.

use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::super::openapi::McpEnvelope;
use super::super::{ApiError, ApiResponse, AppJson, AppPath, ensure_admin, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/catalog` — connectable services, credential-free.
#[utoipa::path(
    get,
    path = "/api/mcp/catalog",
    tag = "mcp",
    responses(
        (status = 200, description = "Connectable services catalog", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
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

/// `GET /api/mcp/composio/toolkits` — platform Composio toolkits only.
#[utoipa::path(
    get,
    path = "/api/mcp/composio/toolkits",
    tag = "mcp",
    responses(
        (status = 200, description = "Registered Composio toolkits", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub async fn list_toolkits(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<ApiResponse, ApiError> {
    let user = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::catalog::list_toolkits(&state, user).await?,
        "Toolkits retrieved successfully",
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAuthConfig {
    pub toolkit: String,
    #[serde(default = "default_true")]
    pub use_composio_managed: bool,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `POST /api/mcp/auth-configs` — register a platform Composio connector (admin).
#[utoipa::path(
    post,
    path = "/api/mcp/auth-configs",
    tag = "mcp",
    request_body = CreateAuthConfig,
    responses(
        (status = 201, description = "Auth config created — `data` is the connector view", body = McpEnvelope),
        (status = 403, description = "Admin privileges required", body = McpEnvelope),
    ),
)]
pub async fn create_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    AppJson(body): AppJson<CreateAuthConfig>,
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
            description: body.description,
            logo_url: body.logo_url,
        },
    )
    .await?;
    Ok(ApiResponse::created(
        view,
        "Auth config created successfully",
    ))
}

/// `GET /api/mcp/auth-configs` — list platform Composio connectors (admin).
#[utoipa::path(
    get,
    path = "/api/mcp/auth-configs",
    tag = "mcp",
    responses(
        (status = 200, description = "Registered platform Composio connectors", body = McpEnvelope),
        (status = 403, description = "Admin privileges required", body = McpEnvelope),
    ),
)]
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAuthConfig {
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub description: Option<String>,
}

/// `PATCH /api/mcp/auth-configs/{connector_id}` — edit composio catalog metadata (admin).
#[utoipa::path(
    patch,
    path = "/api/mcp/auth-configs/{connector_id}",
    tag = "mcp",
    params(("connector_id" = Uuid, Path, description = "Composio connector id")),
    request_body = UpdateAuthConfig,
    responses(
        (status = 200, description = "Auth config updated", body = McpEnvelope),
        (status = 403, description = "Admin privileges required", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn update_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
    AppJson(body): AppJson<UpdateAuthConfig>,
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
#[utoipa::path(
    delete,
    path = "/api/mcp/auth-configs/{connector_id}",
    tag = "mcp",
    params(("connector_id" = Uuid, Path, description = "Composio connector id")),
    responses(
        (status = 200, description = "Auth config deleted", body = McpEnvelope),
        (status = 403, description = "Admin privileges required", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn delete_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(connector_id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    ensure_admin(&claims)?;
    service::catalog::delete_composio(&state, connector_id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Auth config deleted successfully",
    ))
}
