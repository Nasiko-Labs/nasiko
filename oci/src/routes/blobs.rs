use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::OciState;
use crate::authz::{check_repo_access, check_repo_delete_access, CallerIdentity};
use crate::error::{OciError, Result};
use crate::ops;

pub async fn head_blob(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((_owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let size = state.storage.blob_size(&digest).await?;
    let size_str = size.to_string();
    Ok((
        StatusCode::OK,
        [
            ("Content-Length", size_str.as_str()),
            ("Docker-Content-Digest", digest.as_str()),
            ("Content-Type", "application/octet-stream"),
        ],
        "",
    )
        .into_response())
}

pub async fn get_blob(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((_owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let url = ops::get_blob_redirect_url(&state, &digest).await?;
    Ok(Redirect::temporary(&url).into_response())
}

pub async fn delete_blob(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((_owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_delete_access(&state, &caller, &repo).await?;
    ops::delete_blob(&state, &digest).await?;
    Ok((StatusCode::ACCEPTED, "").into_response())
}

pub async fn initiate_upload(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo)): Path<(String, String)>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id = ops::initiate_upload(&state, &name).await?;

    let location = format!("/v2/{name}/blobs/uploads/{upload_id}");
    let uuid_str = upload_id.to_string();

    Ok((
        StatusCode::ACCEPTED,
        [
            ("Location", location.as_str()),
            ("Range", "0-0"),
            ("Docker-Upload-UUID", uuid_str.as_str()),
        ],
        "",
    )
        .into_response())
}

pub async fn patch_upload(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id: Uuid = upload_uuid
        .parse()
        .map_err(|_| OciError::BadRequest("invalid upload UUID".into()))?;

    let chunk = axum::body::to_bytes(request.into_body(), 512 * 1024 * 1024)
        .await
        .map_err(|e| OciError::BadRequest(e.to_string()))?;

    let result = ops::append_chunk(&state, &name, upload_id, chunk).await?;

    let location = format!("/v2/{name}/blobs/uploads/{upload_id}");
    let range = format!("0-{}", (result.new_offset - 1).max(0));
    let uuid_str = upload_id.to_string();

    Ok((
        StatusCode::ACCEPTED,
        [
            ("Location", location.as_str()),
            ("Range", range.as_str()),
            ("Docker-Upload-UUID", uuid_str.as_str()),
        ],
        "",
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct CompleteUploadParams {
    digest: Option<String>,
}

pub async fn complete_upload(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    Query(params): Query<CompleteUploadParams>,
    request: Request<Body>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id: Uuid = upload_uuid
        .parse()
        .map_err(|_| OciError::BadRequest("invalid upload UUID".into()))?;

    let put_body = axum::body::to_bytes(request.into_body(), 512 * 1024 * 1024)
        .await
        .map_err(|e| OciError::BadRequest(e.to_string()))?;

    let result = ops::complete_upload(
        &state,
        &name,
        upload_id,
        put_body,
        params.digest.as_deref(),
    )
    .await?;

    let location = format!("/v2/{name}/blobs/{}", result.digest);

    Ok((
        StatusCode::CREATED,
        [
            ("Location", location.as_str()),
            ("Docker-Content-Digest", result.digest.as_str()),
            ("Content-Length", "0"),
        ],
        "",
    )
        .into_response())
}
