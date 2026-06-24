use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth::AdminAuth,
    error::{AppError, Result},
    AppState,
};

/// HEAD /v2/*name/blobs/:digest — check whether a blob exists (used by oras/docker before push)
pub async fn head_blob(
    State(state): State<AppState>,
    Path((_, _, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    if !state.storage.blob_exists(&digest).await {
        return Err(AppError::NotFound(format!("blob {digest} not found")));
    }
    Ok((
        StatusCode::OK,
        [
            ("Docker-Content-Digest", digest.as_str()),
            ("Content-Length", "0"),
        ],
        "",
    )
        .into_response())
}

/// GET /v2/*name/blobs/:digest — proxy blob data from S3
pub async fn get_blob(
    State(state): State<AppState>,
    Path((_, _, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    if !state.storage.blob_exists(&digest).await {
        return Err(AppError::NotFound(format!("blob {digest} not found")));
    }
    let data = state.storage.get_blob(&digest).await?;
    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("docker-content-digest", &digest)
        .body(Body::from(data))
        .unwrap())
}

/// DELETE /v2/*name/blobs/:digest — remove a blob from S3
pub async fn delete_blob(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((_, _, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    if !state.storage.blob_exists(&digest).await {
        return Err(AppError::NotFound(format!("blob {digest} not found")));
    }
    state.storage.delete_blob(&digest).await?;
    Ok((StatusCode::ACCEPTED, "").into_response())
}

/// POST /v2/*name/blobs/uploads/ — initiate an upload session
pub async fn initiate_upload(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, repo)): Path<(String, String)>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");
    let upload_id = Uuid::new_v4();

    sqlx::query("INSERT INTO oci_uploads (uuid, repository) VALUES ($1, $2)")
        .bind(upload_id)
        .bind(&name)
        .execute(&state.pool)
        .await?;

    let location = format!(
        "{}/v2/{}/blobs/uploads/{}",
        state.config.public_base_url, name, upload_id
    );
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

/// PATCH /v2/*name/blobs/uploads/:uuid — upload a chunk (docker push chunked protocol)
///
/// Each PATCH appends to an in-memory buffer keyed by upload UUID. The buffer
/// is finalized and uploaded to S3 atomically on PUT.
pub async fn patch_upload(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");
    let upload_id: Uuid = upload_uuid
        .parse()
        .map_err(|_| AppError::BadRequest("invalid upload UUID".into()))?;

    let row = sqlx::query(
        "SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
    .bind(upload_id)
    .bind(&name)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("upload session not found".into()))?;

    let current_offset: i64 = row.try_get("offset_bytes")?;

    let chunk = axum::body::to_bytes(request.into_body(), 512 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let chunk_len = chunk.len() as i64;

    state
        .upload_buffers
        .entry(upload_id)
        .or_default()
        .extend_from_slice(&chunk);

    let new_offset = current_offset + chunk_len;

    sqlx::query("UPDATE oci_uploads SET offset_bytes = $1 WHERE uuid = $2")
        .bind(new_offset)
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    let location = format!(
        "{}/v2/{}/blobs/uploads/{}",
        state.config.public_base_url, name, upload_id
    );
    let range = format!("0-{}", (new_offset - 1).max(0));
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

/// PUT /v2/*name/blobs/uploads/:uuid — finalize upload, store blob in S3
///
/// Handles both protocols:
/// - Chunked (docker push): data arrived via PATCH, PUT body is empty
/// - Monolithic (oras push): all data in PUT body, no prior PATCH calls
pub async fn complete_upload(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    Query(params): Query<CompleteUploadParams>,
    request: Request<Body>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");
    let upload_id: Uuid = upload_uuid
        .parse()
        .map_err(|_| AppError::BadRequest("invalid upload UUID".into()))?;

    let exists = sqlx::query(
        "SELECT uuid FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
    .bind(upload_id)
    .bind(&name)
    .fetch_optional(&state.pool)
    .await?
    .is_some();

    if !exists {
        return Err(AppError::NotFound("upload session not found".into()));
    }

    let put_body = axum::body::to_bytes(request.into_body(), 512 * 1024 * 1024)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Merge PATCH buffer (chunked) with any data in the PUT body (last chunk or monolithic)
    let data = if let Some((_, mut buf)) = state.upload_buffers.remove(&upload_id) {
        if !put_body.is_empty() {
            buf.extend_from_slice(&put_body);
        }
        buf.freeze()
    } else {
        put_body
    };

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let computed = format!("sha256:{}", hex::encode(hasher.finalize()));

    if let Some(expected) = &params.digest
        && *expected != computed {
            return Err(AppError::BadRequest(format!(
                "digest mismatch: expected {expected}, got {computed}"
            )));
        }

    state.storage.put_blob(&computed, data).await?;

    sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    let location = format!(
        "{}/v2/{}/blobs/{}",
        state.config.public_base_url, name, computed
    );

    Ok((
        StatusCode::CREATED,
        [
            ("Location", location.as_str()),
            ("Docker-Content-Digest", computed.as_str()),
            ("Content-Length", "0"),
        ],
        "",
    )
        .into_response())
}
