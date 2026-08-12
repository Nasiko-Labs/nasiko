use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::OciState;
use crate::authz::{
    Caller, CallerIdentity, Writer, check_pull_access, check_repo_delete_access, check_write_access,
};
use crate::error::{OciCode, OciError, Result};
use crate::ops;

/// Per-chunk body-read cap for `patch_upload`/`complete_upload`. Also applied
/// as a router-level `DefaultBodyLimit` (see `routes::router`) so oversized
/// requests are rejected before a handler even runs, not just inside it.
pub(crate) const MAX_CHUNK_BYTES: usize = 512 * 1024 * 1024;

pub async fn head_blob(
    State(state): State<OciState>,
    caller: Caller,
    Path((owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_pull_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    if !ops::blob_linked(&state, &name, &digest).await? {
        return Err(OciError::blob_unknown(format!("blob {digest} not found")));
    }
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
    caller: Caller,
    Path((owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_pull_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let data = ops::get_blob_bytes(&state, &name, &digest).await?;
    let size_str = data.len().to_string();
    Ok((
        StatusCode::OK,
        [
            ("Content-Length", size_str.as_str()),
            ("Docker-Content-Digest", digest.as_str()),
            ("Content-Type", "application/octet-stream"),
        ],
        data,
    )
        .into_response())
}

pub async fn delete_blob(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_delete_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    ops::delete_blob(&state, &name, &digest).await?;
    Ok((StatusCode::ACCEPTED, "").into_response())
}

/// Query parameters the spec defines on `POST …/blobs/uploads/`.
#[derive(Deserialize)]
pub struct InitiateUploadParams {
    /// Monolithic upload (end-4b): the whole blob is in this POST's body.
    digest: Option<String>,
    /// Cross-repository mount (end-11): reuse a blob the registry already holds.
    mount: Option<String>,
    /// Which repository to mount from. Advisory — blobs are globally
    /// content-addressed, so it states intent, not a distinct byte source.
    from: Option<String>,
}

/// Begin a blob upload. Three spec behaviours share this route:
///
/// - `?mount=<digest>&from=<repo>` (end-11): claim bytes already held and answer
///   `201` with no transfer, falling back to a session when not held.
/// - `?digest=<digest>` (end-4b): the whole blob is in the body; store it and
///   answer `201` in one round trip.
/// - neither: open a chunked session and answer `202` with a `Location`.
pub async fn initiate_upload(
    State(state): State<OciState>,
    writer: Writer,
    Path((owner, repo)): Path<(String, String)>,
    Query(params): Query<InitiateUploadParams>,
    request: Request<Body>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");

    if let Some(mount_digest) = params.mount.as_deref() {
        if ops::mount_blob(&state, &name, mount_digest).await? {
            tracing::debug!(
                "mounted blob {mount_digest} into {name} from {}",
                params.from.as_deref().unwrap_or("<unspecified>"),
            );
            let location = format!("/v2/{name}/blobs/{mount_digest}");
            return Ok((
                StatusCode::CREATED,
                [
                    ("Location", location.as_str()),
                    ("Docker-Content-Digest", mount_digest),
                    ("Content-Length", "0"),
                ],
                "",
            )
                .into_response());
        }
        tracing::debug!("blob {mount_digest} not held; falling back to an upload session");
    }

    if let Some(expected) = params.digest.as_deref() {
        let data = axum::body::to_bytes(request.into_body(), MAX_CHUNK_BYTES)
            .await
            .map_err(|e| OciError::Oci(OciCode::BlobUploadInvalid, e.to_string()))?;
        let result = ops::upload_blob_monolithic(&state, &name, data, expected).await?;

        let location = format!("/v2/{name}/blobs/{}", result.digest);
        return Ok((
            StatusCode::CREATED,
            [
                ("Location", location.as_str()),
                ("Docker-Content-Digest", result.digest.as_str()),
                ("Content-Length", "0"),
            ],
            "",
        )
            .into_response());
    }

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

/// GET …/blobs/uploads/:uuid (end-13) — how much of the session is held, so a
/// client that lost its `Range` bookkeeping can resume rather than restart.
pub async fn upload_status(
    State(state): State<OciState>,
    writer: Writer,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id = parse_upload_id(&upload_uuid)?;

    let offset = ops::upload_offset(&state, &name, upload_id).await?;
    let location = format!("/v2/{name}/blobs/uploads/{upload_id}");
    let range = format!("0-{}", (offset - 1).max(0));

    Ok((
        StatusCode::NO_CONTENT,
        [
            ("Location", location.as_str()),
            ("Range", range.as_str()),
            ("Docker-Upload-UUID", upload_uuid.as_str()),
        ],
        "",
    )
        .into_response())
}

/// DELETE …/blobs/uploads/:uuid — abandon a session and free its buffer now.
pub async fn cancel_upload(
    State(state): State<OciState>,
    writer: Writer,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id = parse_upload_id(&upload_uuid)?;
    ops::cancel_upload(&state, &name, upload_id).await?;
    Ok((StatusCode::NO_CONTENT, "").into_response())
}

/// An unparseable UUID names no session that could exist, which is the same
/// condition as a session that is gone — so it gets the same code.
fn parse_upload_id(raw: &str) -> Result<Uuid> {
    raw.parse()
        .map_err(|_| OciError::upload_unknown("invalid upload UUID"))
}

pub async fn patch_upload(
    State(state): State<OciState>,
    writer: Writer,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id = parse_upload_id(&upload_uuid)?;

    // Parsed before the body is consumed: a mismatched range must be rejected
    // without reading (and buffering) a chunk we are going to discard.
    let declared_start = request
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split('-').next()?.trim().parse::<i64>().ok());

    let chunk = axum::body::to_bytes(request.into_body(), MAX_CHUNK_BYTES)
        .await
        .map_err(|e| OciError::Oci(OciCode::BlobUploadInvalid, e.to_string()))?;

    let result = match ops::append_chunk(&state, &name, upload_id, chunk, declared_start).await {
        Ok(r) => r,
        // A range rejection has to carry the registry's real offset, otherwise the
        // client has nothing to resync against and can only start over.
        Err(OciError::Oci(OciCode::RangeNotSatisfiable, msg)) => {
            let offset = ops::upload_offset(&state, &name, upload_id)
                .await
                .unwrap_or(0);
            let range = format!("0-{}", (offset - 1).max(0));
            let body = serde_json::json!({"errors": [{
                "code": OciCode::RangeNotSatisfiable.as_str(),
                "message": msg,
            }]});
            return Ok((
                OciCode::RangeNotSatisfiable.status(),
                [
                    ("Range", range.as_str()),
                    ("Content-Type", "application/json"),
                ],
                body.to_string(),
            )
                .into_response());
        }
        Err(e) => return Err(e),
    };

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
    writer: Writer,
    Path((owner, repo, upload_uuid)): Path<(String, String, String)>,
    Query(params): Query<CompleteUploadParams>,
    request: Request<Body>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");
    let upload_id = parse_upload_id(&upload_uuid)?;

    let put_body = axum::body::to_bytes(request.into_body(), MAX_CHUNK_BYTES)
        .await
        .map_err(|e| OciError::Oci(OciCode::BlobUploadInvalid, e.to_string()))?;

    let result =
        ops::complete_upload(&state, &name, upload_id, put_body, params.digest.as_deref()).await?;

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
