use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::OciState;
use crate::error::{OciError, Result};

pub struct ManifestData {
    pub digest: String,
    pub media_type: String,
    pub content: String,
    pub size_bytes: i64,
}

pub async fn get_manifest(
    state: &OciState,
    repository: &str,
    reference: &str,
) -> Result<ManifestData> {
    let row = sqlx::query(
        r#"
        SELECT digest, media_type, content::text, size_bytes
        FROM oci_manifests
        WHERE repository = $1 AND (digest = $2 OR reference = $2)
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(repository)
    .bind(reference)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| OciError::NotFound(format!("manifest {repository}:{reference} not found")))?;

    let manifest = ManifestData {
        digest: row.try_get("digest")?,
        media_type: row.try_get("media_type")?,
        content: row.try_get("content")?,
        size_bytes: row.try_get("size_bytes")?,
    };

    // Pull-by-digest integrity check: when the caller addressed this manifest
    // by its content digest (not a mutable tag — OCI tags can never contain
    // ':', so any `reference` containing one is unambiguously a digest), the
    // returned bytes MUST hash to exactly the digest requested. This defends
    // against the DB's own `digest` column somehow not matching its `content`
    // (e.g. row-level corruption or a future code path that updates one
    // without the other) — a mismatch here is an integrity violation on a
    // security-sensitive path (image pull), not merely a "not found".
    // Tag lookups are intentionally NOT verified this way: tags are mutable
    // pointers by design, so "the content changed" is expected, not a defect.
    if is_digest_reference(reference) {
        let mut hasher = Sha256::new();
        hasher.update(manifest.content.as_bytes());
        let computed = format!("sha256:{}", hex::encode(hasher.finalize()));
        if computed != reference {
            return Err(OciError::Storage(format!(
                "manifest {repository}@{reference}: stored content digest ({computed}) does not match requested digest — integrity check failed"
            )));
        }
    }

    Ok(manifest)
}

/// OCI tag names may never contain `:` (spec: `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`),
/// so any `reference` containing one is unambiguously a content digest
/// (`<algorithm>:<hex>`), not a tag.
fn is_digest_reference(reference: &str) -> bool {
    reference.contains(':')
}

pub struct PutManifestResult {
    pub digest: String,
}

/// Extract every blob digest an OCI image manifest references (config +
/// layers). Manifest lists/indexes (`"manifests": [...]`) reference OTHER
/// manifests, not blobs — out of scope here, explicitly skipped rather than
/// mis-parsed. Each platform-specific child manifest in a multi-arch push is
/// PUT separately and DOES have real layers/config, so this isn't a coverage
/// gap for the standard buildx/multi-arch push flow.
fn extract_referenced_blob_digests(content: &serde_json::Value) -> Vec<String> {
    if content
        .get("manifests")
        .and_then(|v| v.as_array())
        .is_some()
    {
        return Vec::new();
    }
    let mut digests = Vec::new();
    if let Some(d) = content
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|v| v.as_str())
    {
        digests.push(d.to_string());
    }
    if let Some(layers) = content.get("layers").and_then(|v| v.as_array()) {
        for l in layers {
            if let Some(d) = l.get("digest").and_then(|v| v.as_str()) {
                digests.push(d.to_string());
            }
        }
    }
    digests
}

pub async fn put_manifest(
    state: &OciState,
    repository: &str,
    reference: &str,
    content_type: &str,
    body: &[u8],
) -> Result<PutManifestResult> {
    let size_bytes = body.len() as i64;

    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));

    // Push-by-digest verification (OCI spec): when the caller addressed this
    // PUT by a content digest rather than a mutable tag, the pushed body must
    // actually hash to that digest — otherwise a poisoned reference gets
    // stored and later 500s on pull (`get_manifest`'s own digest check) rather
    // than being rejected up front with the 400 the spec requires.
    if is_digest_reference(reference) && reference != digest {
        return Err(OciError::BadRequest(format!(
            "digest mismatch: reference {reference} does not match computed digest {digest}"
        )));
    }

    let content: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| OciError::BadRequest(format!("invalid JSON manifest: {e}")))?;
    let raw_body = String::from_utf8(body.to_vec())
        .map_err(|e| OciError::BadRequest(format!("manifest is not valid UTF-8: {e}")))?;

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO oci_manifests (digest, repository, reference, media_type, content, size_bytes)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (repository, digest) DO UPDATE SET reference = EXCLUDED.reference
        "#,
    )
    .bind(&digest)
    .bind(repository)
    .bind(reference)
    .bind(content_type)
    .bind(&raw_body)
    .bind(size_bytes)
    .execute(&mut *tx)
    .await?;

    // Record which blobs THIS repo's manifest references — the linkage
    // delete_blob/get_blob/head_blob rely on to avoid destroying/exposing a
    // blob that belongs to a different repo sharing the same digest. Uses a
    // real `?` (not a swallowed error) — an unrecorded link would silently
    // reintroduce the cross-repo data-loss bug this table exists to close.
    for blob_digest in extract_referenced_blob_digests(&content) {
        sqlx::query(
            "INSERT INTO oci_blob_refs (digest, repository) VALUES ($1, $2) ON CONFLICT (digest, repository) DO NOTHING",
        )
        .bind(&blob_digest)
        .bind(repository)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Referrers API: if manifest has a "subject" field, index it
    if let Some(subject) = content.get("subject")
        && let Some(subject_digest) = subject.get("digest").and_then(|v| v.as_str())
    {
        let artifact_type = content
            .get("artifactType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let annotations = content.get("annotations").cloned();
        let _ = sqlx::query(
                r#"
                INSERT INTO oci_referrers (subject_digest, repository, referrer_digest, artifact_type, annotations, size_bytes)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (subject_digest, referrer_digest) DO NOTHING
                "#,
            )
            .bind(subject_digest)
            .bind(repository)
            .bind(&digest)
            .bind(&artifact_type)
            .bind(&annotations)
            .bind(size_bytes)
            .execute(&state.pool)
            .await;
    }

    Ok(PutManifestResult { digest })
}

pub async fn delete_manifest(state: &OciState, repository: &str, reference: &str) -> Result<()> {
    let rows = sqlx::query(
        "DELETE FROM oci_manifests WHERE repository = $1 AND (digest = $2 OR reference = $2)",
    )
    .bind(repository)
    .bind(reference)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(OciError::NotFound(format!(
            "manifest {repository}:{reference} not found"
        )));
    }

    Ok(())
}

pub struct ReferrerEntry {
    pub digest: String,
    pub media_type: String,
    pub artifact_type: Option<String>,
    pub annotations: Option<serde_json::Value>,
    pub size_bytes: i64,
}

pub async fn get_referrers(
    state: &OciState,
    repository: &str,
    subject_digest: &str,
) -> Result<Vec<ReferrerEntry>> {
    let rows = sqlx::query(
        r#"
        SELECT r.referrer_digest, r.artifact_type, r.annotations, r.size_bytes,
               m.media_type
        FROM oci_referrers r
        JOIN oci_manifests m ON m.digest = r.referrer_digest AND m.repository = r.repository
        WHERE r.repository = $1 AND r.subject_digest = $2
        ORDER BY r.created_at DESC
        "#,
    )
    .bind(repository)
    .bind(subject_digest)
    .fetch_all(&state.pool)
    .await?;

    let entries = rows
        .into_iter()
        .map(|row| ReferrerEntry {
            digest: row.try_get("referrer_digest").unwrap_or_default(),
            media_type: row.try_get("media_type").unwrap_or_default(),
            artifact_type: row.try_get("artifact_type").unwrap_or(None),
            annotations: row.try_get("annotations").unwrap_or(None),
            size_bytes: row.try_get("size_bytes").unwrap_or(0),
        })
        .collect();

    Ok(entries)
}
