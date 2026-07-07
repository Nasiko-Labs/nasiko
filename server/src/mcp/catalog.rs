//! Service catalog + platform Composio auth-config (toolkit) registration.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};

use nasiko_mcp_gateway::{McpError, repo};

use super::{ApiError, capitalize, ensure_admin};
use crate::auth::Claims;
use crate::state::AppState;

/// `GET /api/mcp/catalog` — connectable services, credential-free.
pub async fn get_catalog(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let configs = repo::list_platform_auth_configs(&state.mcp.db).await?;
    let servers = repo::list_platform_mcp_servers(&state.mcp.db).await?;

    let mut services: Vec<Value> = Vec::new();
    for ac in configs {
        services.push(json!({
            "name": ac.toolkit,
            "type": "composio",
            "display_name": ac.display_name.unwrap_or_else(|| capitalize(&ac.toolkit)),
            "description": Value::Null,
            "logo_url": ac.logo_url,
            "auth_flow": "oauth",
        }));
    }
    for s in servers {
        let auth_flow = match s.auth_type.as_str() {
            "oauth2" => "oauth",
            "bearer" | "basic" | "url_param" => "api_key",
            _ => "none",
        };
        services.push(json!({
            "name": s.name,
            "type": "mcp",
            "display_name": s.display_name.unwrap_or_else(|| capitalize(&s.name)),
            "description": s.description,
            "logo_url": s.logo_url,
            "auth_flow": auth_flow,
        }));
    }

    Ok(Json(json!({ "services": services })))
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

    let toolkit = body.toolkit.to_lowercase();
    if repo::get_platform_auth_config_by_toolkit(&state.mcp.db, &toolkit).await?.is_some() {
        return Err(ApiError(McpError::Conflict(format!(
            "platform auth config for '{toolkit}' already exists"
        ))));
    }
    // Guard against a toolkit name colliding with a platform MCP server name —
    // per-agent permission rows key on server_name alone, so a collision would
    // make one toggle control both. (Same guard as the PoC.)
    if repo::get_platform_mcp_server_by_name(&state.mcp.db, &toolkit).await?.is_some() {
        return Err(ApiError(McpError::Conflict(format!(
            "'{toolkit}' is already a platform MCP server — choose a different name"
        ))));
    }

    let provider = state.mcp.providers.require_composio()?;
    let created = provider
        .create_auth_config(
            &toolkit,
            body.use_composio_managed,
            body.client_id.as_deref(),
            body.client_secret.as_deref(),
            body.scopes.as_deref(),
        )
        .await?;

    let row = repo::create_auth_config(
        &state.mcp.db,
        &created.auth_config_id,
        None, // platform
        &toolkit,
        body.use_composio_managed,
        true, // is_platform
        body.display_name.as_deref(),
        body.logo_url.as_deref(),
    )
    .await?;

    tracing::info!(toolkit = %toolkit, auth_config_id = %row.auth_config_id, "registered platform composio toolkit");
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "auth_config_id": row.auth_config_id,
            "toolkit": row.toolkit,
            "is_platform": row.is_platform,
        })),
    ))
}

/// `GET /api/mcp/auth-configs` — list platform toolkits.
pub async fn list_auth_configs(
    State(state): State<AppState>,
    _claims: Claims,
) -> Result<Json<Value>, ApiError> {
    let configs = repo::list_platform_auth_configs(&state.mcp.db).await?;
    let out: Vec<Value> = configs
        .into_iter()
        .map(|ac| {
            json!({
                "auth_config_id": ac.auth_config_id,
                "toolkit": ac.toolkit,
                "is_platform": ac.is_platform,
                "display_name": ac.display_name,
                "logo_url": ac.logo_url,
            })
        })
        .collect();
    let total = out.len();
    Ok(Json(json!({ "data": out, "total": total })))
}

/// `DELETE /api/mcp/auth-configs/{auth_config_id}` — remove a platform toolkit (admin).
pub async fn delete_auth_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(auth_config_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_admin(&claims)?;
    if !repo::delete_auth_config(&state.mcp.db, &auth_config_id).await? {
        return Err(ApiError(McpError::NotFound(format!(
            "auth config '{auth_config_id}' not found"
        ))));
    }
    Ok(StatusCode::NO_CONTENT)
}
