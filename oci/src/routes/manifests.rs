use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::OciState;
use crate::authz::{check_repo_access, check_repo_delete_access, CallerIdentity};
use crate::error::Result;
use crate::ops;

pub async fn get_manifest(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, reference)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let m = ops::get_manifest(&state, &name, &reference).await?;

    Ok((
        StatusCode::OK,
        [
            ("content-type", m.media_type.as_str()),
            ("docker-content-digest", m.digest.as_str()),
        ],
        m.content,
    )
        .into_response())
}

pub async fn put_manifest(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, reference)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();

    let body = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| crate::error::OciError::BadRequest(e.to_string()))?;

    let result = ops::put_manifest(&state, &name, &reference, &content_type, &body).await?;

    let location = format!("/v2/{name}/manifests/{}", result.digest);

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

pub async fn delete_manifest(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, reference)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_delete_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    ops::delete_manifest(&state, &name, &reference).await?;
    Ok((StatusCode::ACCEPTED, "").into_response())
}

pub async fn get_referrers(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo, subject_digest)): Path<(String, String, String)>,
) -> Result<Response> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let entries = ops::get_referrers(&state, &name, &subject_digest).await?;

    let manifests: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|e| {
            let mut entry = serde_json::json!({
                "mediaType": e.media_type,
                "digest": e.digest,
                "size": e.size_bytes,
            });
            if let Some(at) = e.artifact_type {
                entry["artifactType"] = serde_json::json!(at);
            }
            if let Some(ann) = e.annotations {
                entry["annotations"] = ann;
            }
            entry
        })
        .collect();

    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": manifests,
    });

    let body = index.to_string();
    Ok((
        StatusCode::OK,
        [
            ("content-type", "application/vnd.oci.image.index.v1+json"),
            ("OCI-Filters-Applied", ""),
        ],
        body,
    )
        .into_response())
}
