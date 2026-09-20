use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::OciState;
use crate::authz::{
    Caller, CallerIdentity, Writer, check_pull_access, check_repo_delete_access, check_write_access,
};
use crate::error::{OciError, Result};
use crate::ops;

/// Header values here are digests and paths, already constrained to ASCII, so a
/// rejection means something upstream let a control character through.
fn header_value(raw: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(raw).map_err(|_| {
        OciError::manifest_invalid(format!("value '{raw}' cannot be sent as a header"))
    })
}

pub async fn get_manifest(
    State(state): State<OciState>,
    caller: Caller,
    Path((owner, repo, reference)): Path<(String, String, String)>,
) -> Result<Response> {
    check_pull_access(&state, &caller, &repo).await?;
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
    writer: Writer,
    Path((owner, repo, reference)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    check_write_access(&state, &writer, &repo).await?;
    let name = format!("{owner}/{repo}");
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();

    let body = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| OciError::manifest_invalid(e.to_string()))?;

    let result = ops::put_manifest(&state, &name, &reference, &content_type, &body).await?;

    let location = format!("/v2/{name}/manifests/{}", result.digest);

    let mut headers = HeaderMap::new();
    headers.insert("Location", header_value(&location)?);
    headers.insert("Docker-Content-Digest", header_value(&result.digest)?);
    headers.insert("Content-Length", HeaderValue::from_static("0"));
    // Required of any registry implementing the referrers API: it is how a client
    // learns its attachment was indexed and it need not fall back to the tag
    // scheme.
    if let Some(subject) = result.subject {
        headers.insert("OCI-Subject", header_value(&subject)?);
    }

    Ok((StatusCode::CREATED, headers, "").into_response())
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

/// `?artifactType=` narrowing for the referrers index (end-12b).
#[derive(Deserialize)]
pub struct ReferrerParams {
    #[serde(rename = "artifactType")]
    artifact_type: Option<String>,
}

pub async fn get_referrers(
    State(state): State<OciState>,
    caller: Caller,
    Path((owner, repo, subject_digest)): Path<(String, String, String)>,
    Query(params): Query<ReferrerParams>,
) -> Result<Response> {
    check_pull_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let entries = ops::get_referrers(
        &state,
        &name,
        &subject_digest,
        params.artifact_type.as_deref(),
    )
    .await?;

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
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/vnd.oci.image.index.v1+json"),
    );
    // Announced only when a filter was really applied. Sending it empty on every
    // response claims filtering happened, telling a client its results are
    // already narrowed when they are not.
    if params.artifact_type.is_some() {
        headers.insert(
            "OCI-Filters-Applied",
            HeaderValue::from_static("artifactType"),
        );
    }

    Ok((StatusCode::OK, headers, body).into_response())
}
