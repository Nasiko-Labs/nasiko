//! Upload-your-own-MCP-server handlers — zip or GitHub source, async build
//! (queued via the shared `build_jobs` worker, Step 9), polling status/logs.
//! No SSE streaming in v1 (see the plan's Deferred section).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::extract::{Multipart, State};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::super::openapi::McpEnvelope;
use super::super::{ApiError, ApiResponse, AppJson, AppPath, AppQuery, parse_user};
use crate::auth::Claims;
use crate::multipart_util::{StreamUploadError, stream_field_to_fresh_temp_file};
use crate::state::AppState;

/// Doc-only schema for the multipart form consumed by `upload_zip`.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct UploadZipForm {
    /// Connector name (required).
    name: String,
    /// Version tag; defaults to `v1`.
    version_tag: Option<String>,
    /// JSON object of env vars for the built server, as a string field.
    env: Option<String>,
    /// Zip archive with the MCP server source (also accepted as `file`).
    #[schema(value_type = String, format = Binary)]
    source: Vec<u8>,
}

/// `POST /api/mcp/connectors/upload` — multipart zip upload.
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/upload",
    tag = "mcp",
    request_body(content = UploadZipForm, content_type = "multipart/form-data"),
    responses(
        (status = 202, description = "Build queued — `data` is `{connector_id, build_id}`", body = McpEnvelope),
        (status = 400, description = "Missing name/zip or upload too large", body = McpEnvelope),
        (status = 403, description = "Caller lacks deploy permission"),
    ),
)]
pub async fn upload_zip(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> Result<ApiResponse, ApiError> {
    let owner = parse_user(&claims)?;

    let mut name: Option<String> = None;
    let mut version_tag: Option<String> = None;
    let mut zip_path: Option<PathBuf> = None;
    let mut env: HashMap<String, String> = HashMap::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "env" => {
                if let Ok(text) = field.text().await
                    && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text)
                {
                    env = map;
                }
            }
            "source" | "file" => {
                match stream_field_to_fresh_temp_file(
                    "nasiko-mcp-upload",
                    "upload.zip",
                    field,
                    state.config.mcp_upload_max_bytes,
                )
                .await
                {
                    Ok(path) => zip_path = Some(path),
                    Err(StreamUploadError::TooLarge) => {
                        return Err(nasiko_mcp_gateway::McpError::BadRequest(format!(
                            "upload exceeds {} bytes",
                            state.config.mcp_upload_max_bytes
                        ))
                        .into());
                    }
                    Err(StreamUploadError::ReadFailed(e)) => {
                        return Err(nasiko_mcp_gateway::McpError::BadRequest(format!(
                            "failed to read upload stream: {e}"
                        ))
                        .into());
                    }
                    Err(StreamUploadError::Io(e)) => {
                        tracing::error!(%e, "mcp upload: stream to disk failed");
                        return Err(nasiko_mcp_gateway::McpError::Internal(
                            "internal error".into(),
                        )
                        .into());
                    }
                }
            }
            _ => {}
        }
    }

    let name = name
        .filter(|n| !n.is_empty())
        .ok_or_else(|| nasiko_mcp_gateway::McpError::BadRequest("name is required".into()))?;
    let version_tag = version_tag.unwrap_or_else(|| "v1".to_string());
    let zip_path = zip_path
        .ok_or_else(|| nasiko_mcp_gateway::McpError::BadRequest("source zip is required".into()))?;

    let (connector_id, build_id) =
        crate::mcp::build::queue_zip_upload(&state, owner, name, version_tag, zip_path, env)
            .await?;

    Ok(ApiResponse::accepted(
        json!({ "connector_id": connector_id, "build_id": build_id }),
        "MCP server build queued",
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UploadFromGithub {
    pub name: String,
    #[serde(default = "default_version_tag")]
    pub version_tag: String,
    pub github_url: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

fn default_version_tag() -> String {
    "v1".to_string()
}

/// `POST /api/mcp/connectors/upload-github` — clone a repo instead of a zip.
/// Re-validates `github_url` (HTTPS-only + host allowlist) the same way
/// `execute_build`/`execute_mcp_server_build` already do at clone time — this
/// is a defence-in-depth check at the handler layer, not the only one.
#[utoipa::path(
    post,
    path = "/api/mcp/connectors/upload-github",
    tag = "mcp",
    request_body = UploadFromGithub,
    responses(
        (status = 202, description = "Build queued — `data` is `{connector_id, build_id}`", body = McpEnvelope),
        (status = 400, description = "Missing name or disallowed GitHub URL", body = McpEnvelope),
        (status = 403, description = "Caller lacks deploy permission"),
    ),
)]
pub async fn upload_github(
    State(state): State<AppState>,
    claims: Claims,
    AppJson(body): AppJson<UploadFromGithub>,
) -> Result<ApiResponse, ApiError> {
    let owner = parse_user(&claims)?;
    if body.name.is_empty() {
        return Err(nasiko_mcp_gateway::McpError::BadRequest("name is required".into()).into());
    }
    crate::build::routes::validate_github_url(
        &body.github_url,
        &state.config.git_clone_allowed_hosts,
    )
    .map_err(nasiko_mcp_gateway::McpError::BadRequest)?;

    let (connector_id, build_id) = crate::mcp::build::queue_github_upload(
        &state,
        owner,
        body.name,
        body.version_tag,
        body.github_url,
        body.env,
    )
    .await?;

    Ok(ApiResponse::accepted(
        json!({ "connector_id": connector_id, "build_id": build_id }),
        "MCP server build queued",
    ))
}

/// `GET /api/mcp/connectors/{id}/build-status` — plain polling JSON, no SSE.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/{id}/build-status",
    tag = "mcp",
    params(("id" = Uuid, Path, description = "Connector id")),
    responses(
        (status = 200, description = "Latest build status for the connector", body = McpEnvelope),
        (status = 404, description = "No such connector or build", body = McpEnvelope),
    ),
)]
pub async fn build_status(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(ApiResponse::ok(
        crate::mcp::build::get_build_status(&state.db, caller, claims.is_superuser, id).await?,
        "build status retrieved successfully",
    ))
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: u32,
}
fn default_tail() -> u32 {
    200
}

/// `GET /api/mcp/connectors/{id}/build-logs` — same ownership check as
/// `build_status`, real container stdout/stderr via `ContainerRuntime::logs`.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/{id}/build-logs",
    tag = "mcp",
    params(
        ("id" = Uuid, Path, description = "Connector id"),
        ("tail" = Option<u32>, Query, description = "Number of trailing log lines (default 200)"),
    ),
    responses(
        (status = 200, description = "Build container logs — `data` is the log text", body = McpEnvelope),
        (status = 404, description = "No such connector or build", body = McpEnvelope),
    ),
)]
pub async fn build_logs(
    State(state): State<AppState>,
    claims: Claims,
    AppPath(id): AppPath<Uuid>,
    AppQuery(q): AppQuery<LogsQuery>,
) -> Result<ApiResponse, ApiError> {
    let caller = parse_user(&claims)?;
    let logs = crate::mcp::build::get_build_logs(
        &state.db,
        &state.runtime,
        caller,
        claims.is_superuser,
        id,
        q.tail,
    )
    .await?;
    Ok(ApiResponse::ok(
        json!(logs),
        "build logs retrieved successfully",
    ))
}

// ── My uploaded MCP connectors (mirrors /api/agents/my-uploads) ─────────

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct UploadedConnectorRow {
    connector_id: Uuid,
    connector_name: String,
    build_status: Option<String>,
    error_msg: Option<String>,
    url: Option<String>,
    version_tag: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize, ToSchema)]
pub(crate) struct UploadedConnectorResponse {
    connector_id: String,
    connector_name: String,
    icon_url: Option<String>,
    upload_info: UploadInfoMcp,
    url: Option<String>,
    description: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub(crate) struct UploadInfoMcp {
    upload_type: &'static str,
    upload_status: String,
    status_message: Option<String>,
    error_detail: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
pub(crate) struct UploadedConnectorsListResponse {
    data: Vec<UploadedConnectorResponse>,
    status_code: u16,
    message: String,
}

fn mcp_display_status(build_status: Option<&str>) -> &'static str {
    match build_status {
        Some("running") | Some("success") => "Active",
        Some("pending") | Some("building") => "Deploying",
        Some("failed") => "Failed",
        _ => "Unknown",
    }
}

fn mcp_status_message(display_status: &str, version: Option<&str>) -> Option<String> {
    match display_status {
        "Active" => Some(format!(
            "MCP server v{} deployed successfully",
            version.unwrap_or("1"),
        )),
        "Deploying" => Some("MCP server is being built...".to_string()),
        "Failed" => Some("Build failed".to_string()),
        _ => None,
    }
}

/// `GET /api/mcp/connectors/my-uploads` — list uploaded MCP connectors
/// owned by the caller, mirroring `/api/agents/my-uploads`.
#[utoipa::path(
    get,
    path = "/api/mcp/connectors/my-uploads",
    tag = "mcp",
    responses(
        (status = 200, description = "The caller's uploaded MCP connectors", body = UploadedConnectorsListResponse),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub async fn list_my_uploads(
    State(state): State<AppState>,
    claims: Claims,
) -> impl axum::response::IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let rows: Result<Vec<UploadedConnectorRow>, _> = if claims.is_superuser {
        sqlx::query_as(
            r#"SELECT DISTINCT ON (c.id)
                   c.id           AS connector_id,
                   c.name         AS connector_name,
                   c.build_status,
                   b.error_msg,
                   c.url,
                   b.version_tag,
                   c.created_at
               FROM mcp_connectors c
               LEFT JOIN mcp_connector_builds b ON b.connector_id = c.id
               WHERE c.source_kind = 'uploaded_build'
               ORDER BY c.id, b.created_at DESC
               LIMIT 50"#,
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            r#"SELECT DISTINCT ON (c.id)
                   c.id           AS connector_id,
                   c.name         AS connector_name,
                   c.build_status,
                   b.error_msg,
                   c.url,
                   b.version_tag,
                   c.created_at
               FROM mcp_connectors c
               LEFT JOIN mcp_connector_builds b ON b.connector_id = c.id
               WHERE c.source_kind = 'uploaded_build' AND c.owner_id = $1
               ORDER BY c.id, b.created_at DESC
               LIMIT 50"#,
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => {
            let count = rows.len();
            let data = rows
                .into_iter()
                .map(|r| {
                    let display_status = mcp_display_status(r.build_status.as_deref());
                    let status_message =
                        mcp_status_message(display_status, r.version_tag.as_deref());
                    UploadedConnectorResponse {
                        connector_id: r.connector_id.to_string(),
                        connector_name: r.connector_name,
                        icon_url: None,
                        upload_info: UploadInfoMcp {
                            upload_type: "mcp_server",
                            upload_status: display_status.to_string(),
                            status_message,
                            error_detail: r.error_msg,
                        },
                        url: r.url,
                        description: None,
                    }
                })
                .collect();
            axum::Json(UploadedConnectorsListResponse {
                data,
                status_code: 200,
                message: format!("Retrieved {count} uploaded MCP connectors"),
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_my_uploads db error");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
