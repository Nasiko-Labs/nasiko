//! Custom MCP connector registration + probe.

use axum::extract::State;
use serde::Deserialize;
use uuid::Uuid;

use super::super::{ApiError, ApiResponse, AppJson, AppPath, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateConnector {
    pub name: String,
    pub url: String,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub basic_username: Option<String>,
    pub basic_password: Option<String>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
}

fn default_transport() -> String {
    "streamable_http".to_string()
}
fn default_auth_type() -> String {
    "none".to_string()
}

/// `POST /api/mcp/connectors` — register a custom MCP connector (owned by caller).
pub async fn create(
    State(state): State<AppState>,
    claims: Claims,
    AppJson(body): AppJson<CreateConnector>,
) -> Result<ApiResponse, ApiError> {
    let owner = parse_user(&claims)?;
    let view = service::connectors::create(
        &state,
        owner,
        service::connectors::NewConnectorInput {
            name: body.name,
            url: body.url,
            transport: body.transport,
            auth_type: body.auth_type,
            url_param_name: body.url_param_name,
            credential_header_name: body.credential_header_name,
            headers: body.headers,
            basic_username: body.basic_username,
            basic_password: body.basic_password,
            description: body.description,
            display_name: body.display_name,
            logo_url: body.logo_url,
            oauth_client_id: body.oauth_client_id,
            oauth_client_secret: body.oauth_client_secret,
        },
    )
    .await?;
    Ok(ApiResponse::created(view, "Connector created successfully"))
}

#[derive(Debug, Deserialize)]
pub struct UpdateConnector {
    pub name: Option<String>,
    pub url: Option<String>,
    pub transport: Option<String>,
    pub auth_type: Option<String>,
    pub url_param_name: Option<String>,
    pub credential_header_name: Option<String>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub logo_url: Option<String>,
    pub is_active: Option<bool>,
}

/// `PATCH /api/mcp/connectors/{id}` — update an owned connector.
pub async fn update(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
    AppJson(body): AppJson<UpdateConnector>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let input = service::connectors::UpdateConnectorInput {
        name: body.name,
        url: body.url,
        transport: body.transport,
        auth_type: body.auth_type,
        url_param_name: body.url_param_name,
        credential_header_name: body.credential_header_name,
        headers: body.headers,
        description: body.description,
        display_name: body.display_name,
        logo_url: body.logo_url,
        is_active: body.is_active,
    };
    Ok(ApiResponse::ok(
        service::connectors::update(&state, caller, claims.is_superuser, id, input).await?,
        "Connector updated successfully",
    ))
}

/// `GET /api/mcp/connectors` — custom connectors visible to the caller.
pub async fn list(State(state): State<AppState>, claims: Claims) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list(&state, user_id).await?,
        "Connectors retrieved successfully",
    ))
}

/// `GET /api/mcp/connectors/{id}` — a single connector, 404 if not reachable.
pub async fn get(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::get(&state, user_id, id).await?,
        "Connector retrieved successfully",
    ))
}

/// `DELETE /api/mcp/connectors/{id}` — delete an owned connector (or any, if admin).
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::delete(&state, caller, claims.is_superuser, id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Connector deleted successfully"))
}

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    pub url: String,
}

/// `POST /api/mcp/connectors/probe` — detect a server's auth type.
pub async fn probe(
    State(state): State<AppState>,
    _claims: Claims,
    AppJson(body): AppJson<ProbeRequest>,
) -> Result<ApiResponse, ApiError> {
    Ok(ApiResponse::ok(
        service::connectors::probe(&state, &body.url).await?,
        "Connector probed successfully",
    ))
}

/// `POST /api/mcp/connectors/{id}/pin` — pin for quick access.
pub async fn pin(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::connectors::pin(&state, user_id, id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Connector pinned successfully"))
}

/// `DELETE /api/mcp/connectors/{id}/pin` — unpin.
pub async fn unpin(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::connectors::unpin(&state, user_id, id).await?;
    Ok(ApiResponse::ok(serde_json::Value::Null, "Connector unpinned successfully"))
}

/// `GET /api/mcp/connectors/pinned` — the caller's pinned connectors.
pub async fn pinned(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list_pinned(&state, user_id).await?,
        "Pinned connectors retrieved successfully",
    ))
}

/// `GET /api/mcp/connectors/recent` — the caller's recently-used connectors.
pub async fn recent(
    State(state): State<AppState>,
    claims: Claims,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list_recent(&state, user_id).await?,
        "Recent connectors retrieved successfully",
    ))
}
