//! Upload-your-own-MCP-server handlers — zip or GitHub source, async build
//! (queued via the shared `build_jobs` worker, Step 9), polling status/logs.
//! No SSE streaming in v1 (see the plan's Deferred section).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::{
    Json,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::super::{ApiError, parse_user};
use crate::auth::Claims;
use crate::multipart_util::{StreamUploadError, stream_field_to_fresh_temp_file};
use crate::state::AppState;

/// `POST /api/mcp/connectors/upload` — multipart zip upload.
pub async fn upload_zip(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
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
                        return Err(nasiko_mcp_gateway::McpError::BadRequest(format!("failed to read upload stream: {e}")).into());
                    }
                    Err(StreamUploadError::Io(e)) => {
                        tracing::error!(%e, "mcp upload: stream to disk failed");
                        return Err(nasiko_mcp_gateway::McpError::Internal("internal error".into()).into());
                    }
                }
            }
            _ => {}
        }
    }

    let name = name.filter(|n| !n.is_empty()).ok_or_else(|| nasiko_mcp_gateway::McpError::BadRequest("name is required".into()))?;
    let version_tag = version_tag.unwrap_or_else(|| "v1".to_string());
    let zip_path = zip_path.ok_or_else(|| nasiko_mcp_gateway::McpError::BadRequest("source zip is required".into()))?;

    let (connector_id, build_id) =
        crate::mcp::build::queue_zip_upload(&state, owner, name, version_tag, zip_path, env).await?;

    Ok((StatusCode::ACCEPTED, Json(json!({ "connector_id": connector_id, "build_id": build_id }))))
}

#[derive(Debug, Deserialize)]
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
pub async fn upload_github(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<UploadFromGithub>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = parse_user(&claims)?;
    if body.name.is_empty() {
        return Err(nasiko_mcp_gateway::McpError::BadRequest("name is required".into()).into());
    }
    crate::build::routes::validate_github_url(&body.github_url, &state.config.git_clone_allowed_hosts)
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

    Ok((StatusCode::ACCEPTED, Json(json!({ "connector_id": connector_id, "build_id": build_id }))))
}

/// `GET /api/mcp/connectors/{id}/build-status` — plain polling JSON, no SSE.
pub async fn build_status(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(Json(crate::mcp::build::get_build_status(&state.db, caller, claims.is_superuser, id).await?))
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
pub async fn build_logs(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let caller = parse_user(&claims)?;
    Ok(Json(crate::mcp::build::get_build_logs(&state.db, &state.runtime, caller, claims.is_superuser, id, q.tail).await?))
}
