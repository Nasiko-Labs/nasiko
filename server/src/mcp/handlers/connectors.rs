//! Custom MCP connector registration + probe.

use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::super::openapi::McpEnvelope;
use super::super::{ApiError, ApiResponse, AppJson, AppPath, parse_user, service};
use crate::auth::Claims;
use crate::state::AppState;

#[derive(Debug, Deserialize, ToSchema)]
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
#[utoipa::path(
    post,
    path = "/api/mcp/connectors",
    tag = "mcp",
    request_body = CreateConnector,
    responses(
        (status = 201, description = "Connector created — `data` is the connector view", body = McpEnvelope),
        (status = 400, description = "Invalid request body", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
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

#[derive(Debug, Deserialize, ToSchema)]
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
#[utoipa::path(
    patch,
    path = "/api/mcp/connectors/{id}",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    request_body = UpdateConnector,
    responses(
        (status = 200, description = "Connector updated — `data` is the connector view", body = McpEnvelope),
        (status = 403, description = "Not the connector's owner", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
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
#[utoipa::path(
    get,
    path = "/api/mcp/connectors",
    tag = "mcp",
    responses(
        (status = 200, description = "Connectors visible to the caller — `data` is a list of connector views", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub async fn list(State(state): State<AppState>, claims: Claims) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        service::connectors::list(&state, user_id).await?,
        "Connectors retrieved successfully",
    ))
}

/// `GET /api/mcp/connectors/{id}` — a single connector, 404 if not reachable.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/{id}",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Connector view", body = McpEnvelope),
        (status = 404, description = "No such connector (or not visible to the caller)", body = McpEnvelope),
    ),
)]
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
#[utoipa::path(
    delete,
    path = "/api/mcp/connectors/{id}",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Connector deleted", body = McpEnvelope),
        (status = 403, description = "Not the connector's owner", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    service::connectors::delete(&state, caller, claims.is_superuser, id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Connector deleted successfully",
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProbeRequest {
    pub url: String,
}

/// `POST /api/mcp/connectors/probe` — detect a server's auth type.
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/probe",
    tag = "mcp",
    request_body = ProbeRequest,
    responses(
        (status = 200, description = "Probe result — `data` describes the detected transport/auth type", body = McpEnvelope),
        (status = 400, description = "Unreachable or invalid MCP server URL", body = McpEnvelope),
    ),
)]
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
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/{id}/pin",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Connector pinned", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn pin(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::connectors::pin(&state, user_id, id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Connector pinned successfully",
    ))
}

/// `DELETE /api/mcp/connectors/{id}/pin` — unpin.
#[utoipa::path(
    delete,
    path = "/api/mcp/connectors/{id}/pin",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Connector unpinned", body = McpEnvelope),
        (status = 404, description = "No such connector", body = McpEnvelope),
    ),
)]
pub async fn unpin(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let user_id = parse_user(&claims)?;
    service::connectors::unpin(&state, user_id, id).await?;
    Ok(ApiResponse::ok(
        serde_json::Value::Null,
        "Connector unpinned successfully",
    ))
}

/// `GET /api/mcp/connectors/pinned` — the caller's pinned connectors.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/pinned",
    tag = "mcp",
    responses(
        (status = 200, description = "Pinned connectors — `data` is a list of connector views", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
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
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/recent",
    tag = "mcp",
    responses(
        (status = 200, description = "Recently-used connectors — `data` is a list of connector views", body = McpEnvelope),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
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
