use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::Row;

use serde_json::json;

use crate::{auth::AdminAuth, error::Result, AppState};

/// GET /v2/:name/manifests/:reference
pub async fn get_manifest(
    State(state): State<AppState>,
    Path((owner, repo, reference)): Path<(String, String, String)>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");
    let row = sqlx::query(
        r#"
        SELECT digest, media_type, content::text, size_bytes
        FROM oci_manifests
        WHERE repository = $1 AND (digest = $2 OR reference = $2)
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(&name)
    .bind(&reference)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| crate::error::AppError::NotFound(format!("manifest {name}:{reference} not found")))?;

    let digest: String = row.try_get("digest")?;
    let media_type: String = row.try_get("media_type")?;
    let body: String = row.try_get("content")?;

    Ok((
        StatusCode::OK,
        [
            ("content-type", media_type.as_str()),
            ("docker-content-digest", digest.as_str()),
        ],
        body,
    )
        .into_response())
}

/// PUT /v2/:name/manifests/:reference
pub async fn put_manifest(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, repo, reference)): Path<(String, String, String)>,
    request: Request<Body>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();

    let body = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| crate::error::AppError::BadRequest(e.to_string()))?;

    let size_bytes = body.len() as i64;

    let mut hasher = Sha256::new();
    hasher.update(&body);
    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));

    // Parse for validation and referrer extraction, but store original bytes as
    // TEXT so the sha256 digest stays consistent with what we serve back.
    let content = serde_json::from_slice::<serde_json::Value>(&body)
        .map_err(|e| crate::error::AppError::BadRequest(format!("invalid JSON manifest: {e}")))?;
    let raw_body = String::from_utf8(body.to_vec())
        .map_err(|e| crate::error::AppError::BadRequest(format!("manifest is not valid UTF-8: {e}")))?;

    sqlx::query(
        r#"
        INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (digest) DO UPDATE SET reference = EXCLUDED.reference
        "#,
    )
    .bind(&digest)
    .bind(&name)
    .bind(&reference)
    .bind(&content_type)
    .bind(&raw_body)
    .bind(size_bytes)
    .execute(&state.pool)
    .await?;

    // If this manifest has a "subject" field, record it in oci_referrers so
    // GET /v2/{name}/referrers/{subject_digest} can return this manifest.
    if let Some(subject) = content.get("subject") {
        if let Some(subject_digest) = subject.get("digest").and_then(|v| v.as_str()) {
            let artifact_type = content
                .get("artifactType")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let annotations = content.get("annotations").cloned();
            sqlx::query(
                r#"
                INSERT INTO oci_referrers (subject_digest, repository, referrer_digest, artifact_type, annotations, size_bytes)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (subject_digest, referrer_digest) DO NOTHING
                "#,
            )
            .bind(subject_digest)
            .bind(&name)
            .bind(&digest)
            .bind(&artifact_type)
            .bind(&annotations)
            .bind(size_bytes)
            .execute(&state.pool)
            .await?;
        }
    }

    // Index into artifacts table for V1 metadata API discoverability
    index_artifact(&state, &name, &reference, &digest, size_bytes, &content).await;

    let location = format!(
        "{}/v2/{}/manifests/{}",
        state.config.public_base_url, name, digest
    );

    Ok((
        StatusCode::CREATED,
        [
            ("Location", location.as_str()),
            ("Docker-Content-Digest", digest.as_str()),
            ("Content-Length", "0"),
        ],
        "",
    )
        .into_response())
}

/// GET /v2/:name/referrers/:digest — OCI Referrers API (Spec v1.1)
///
/// Returns an OCI Image Index listing all manifests that declared this digest
/// as their `subject` (e.g. SBOMs, signatures, attestations).
pub async fn get_referrers(
    State(state): State<AppState>,
    Path((owner, repo, subject_digest)): Path<(String, String, String)>,
) -> Result<Response> {
    let name = format!("{owner}/{repo}");

    let rows = sqlx::query(
        r#"
        SELECT r.referrer_digest, r.artifact_type, r.annotations, r.size_bytes,
               m.media_type
        FROM oci_referrers r
        JOIN oci_manifests m ON m.digest = r.referrer_digest
        WHERE r.repository = $1 AND r.subject_digest = $2
        ORDER BY r.created_at DESC
        "#,
    )
    .bind(&name)
    .bind(&subject_digest)
    .fetch_all(&state.pool)
    .await?;

    let manifests: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let referrer_digest: String = row.try_get("referrer_digest").unwrap_or_default();
            let media_type: String = row.try_get("media_type").unwrap_or_default();
            let artifact_type: Option<String> = row.try_get("artifact_type").unwrap_or(None);
            let annotations: Option<serde_json::Value> = row.try_get("annotations").unwrap_or(None);
            let size_bytes: i64 = row.try_get("size_bytes").unwrap_or(0);
            let mut entry = json!({
                "mediaType": media_type,
                "digest": referrer_digest,
                "size": size_bytes,
            });
            if let Some(at) = artifact_type {
                entry["artifactType"] = json!(at);
            }
            if let Some(ann) = annotations {
                entry["annotations"] = ann;
            }
            entry
        })
        .collect();

    let index = json!({
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

/// Index a pushed manifest into the artifacts table so the V1 metadata API can find it.
/// Extracts owner/name from the repo path, version from the tag, and metadata from annotations.
async fn index_artifact(
    state: &AppState,
    repo: &str,
    reference: &str,
    digest: &str,
    size_bytes: i64,
    manifest: &serde_json::Value,
) {
    // Only index tagged pushes (not digest-only references)
    if reference.starts_with("sha256:") {
        return;
    }

    // repo is "owner/name"
    let parts: Vec<&str> = repo.splitn(2, '/').collect();
    let (owner, name) = match parts.as_slice() {
        [o, n] => (*o, *n),
        _ => return,
    };

    let annotations = manifest.get("annotations").and_then(|a| a.as_object());

    let artifact_type = annotations
        .and_then(|a| a.get("org.nasiko.type"))
        .and_then(|v| v.as_str())
        .unwrap_or("agent");

    let description = annotations
        .and_then(|a| a.get("org.opencontainers.image.description"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let framework = annotations
        .and_then(|a| a.get("org.nasiko.framework"))
        .and_then(|v| v.as_str());

    let tags: Vec<&str> = annotations
        .and_then(|a| a.get("org.nasiko.tags"))
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').collect())
        .unwrap_or_default();

    // Use the total layer size (actual content), not manifest size
    let layer_size = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .map(|layers| {
            layers.iter()
                .filter_map(|l| l.get("size").and_then(|s| s.as_i64()))
                .sum::<i64>()
        })
        .unwrap_or(size_bytes);

    let result = sqlx::query(
        r#"
        INSERT INTO artifacts (owner, name, version, artifact_type, description, oci_digest, size_bytes, tags, framework)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (owner, name, version) DO UPDATE SET
            oci_digest = EXCLUDED.oci_digest,
            size_bytes = EXCLUDED.size_bytes,
            description = EXCLUDED.description,
            tags = EXCLUDED.tags,
            framework = EXCLUDED.framework,
            updated_at = NOW()
        "#,
    )
    .bind(owner)
    .bind(name)
    .bind(reference)
    .bind(artifact_type)
    .bind(description)
    .bind(digest)
    .bind(layer_size)
    .bind(&tags)
    .bind(framework)
    .execute(&state.pool)
    .await;

    if let Err(e) = result {
        tracing::warn!("failed to index artifact {repo}:{reference}: {e}");
    } else {
        tracing::info!("indexed artifact {owner}/{name}:{reference} ({artifact_type})");
    }
}

/// DELETE /v2/:name/manifests/:reference
pub async fn delete_manifest(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, repo, reference)): Path<(String, String, String)>,
) -> Result<StatusCode> {
    let name = format!("{owner}/{repo}");
    let rows = sqlx::query(
        "DELETE FROM oci_manifests WHERE repository = $1 AND (digest = $2 OR reference = $2)",
    )
    .bind(&name)
    .bind(&reference)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(crate::error::AppError::NotFound(format!(
            "manifest {name}:{reference} not found"
        )));
    }

    Ok(StatusCode::ACCEPTED)
}
