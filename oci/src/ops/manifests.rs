use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::OciState;
use crate::error::{OciCode, OciError, Result};

pub struct ManifestData {
    pub digest: String,
    pub media_type: String,
    pub content: String,
    pub size_bytes: i64,
}

/// OCI tag names may never contain `:` (spec: `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`),
/// so any `reference` containing one is unambiguously a content digest
/// (`<algorithm>:<hex>`), not a tag.
pub fn is_digest_reference(reference: &str) -> bool {
    reference.contains(':')
}

/// Resolve a reference to a stored manifest and return its bytes.
///
/// A reference is either a tag or a digest, and the two can never be confused,
/// so this looks the reference up as a tag and otherwise treats it as a digest —
/// at most one of the two matches.
pub async fn get_manifest(
    state: &OciState,
    repository: &str,
    reference: &str,
) -> Result<ManifestData> {
    let row = sqlx::query(
        r#"
        SELECT m.digest, m.media_type, m.content, m.size_bytes
        FROM oci_manifests m
        WHERE m.repository = $1
          AND m.digest = COALESCE(
              (SELECT t.digest FROM oci_tags t WHERE t.repository = $1 AND t.tag = $2),
              $2
          )
        "#,
    )
    .bind(repository)
    .bind(reference)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| {
        OciError::manifest_unknown(format!("manifest {repository}:{reference} not found"))
    })?;

    let manifest = ManifestData {
        digest: row.try_get("digest")?,
        media_type: row.try_get("media_type")?,
        content: row.try_get("content")?,
        size_bytes: row.try_get("size_bytes")?,
    };

    // Integrity on read: the stored bytes MUST hash to the digest on record.
    // Unconditional, and sound for tag lookups too, because a tag now points at
    // an immutable row rather than one whose content is rewritten in place — so a
    // mismatch is always corruption, never a tag having moved. Manifests are
    // small, so this is cheap.
    let mut hasher = Sha256::new();
    hasher.update(manifest.content.as_bytes());
    let computed = format!("sha256:{}", hex::encode(hasher.finalize()));
    if computed != manifest.digest {
        return Err(OciError::Storage(format!(
            "manifest {repository}:{reference}: stored content hashes to {computed}, not {} — integrity check failed",
            manifest.digest
        )));
    }

    Ok(manifest)
}

pub struct PutManifestResult {
    pub digest: String,
    /// The `subject` digest, when the manifest declared one. The spec requires a
    /// registry implementing the referrers API to echo this back as
    /// `OCI-Subject`, which is how a client learns its attachment was indexed.
    pub subject: Option<String>,
}

/// Every blob digest an OCI image manifest references (config + each layer).
///
/// A manifest index (`"manifests": [...]`) references other manifests, not
/// blobs, so it contributes nothing here — each platform child manifest of a
/// multi-arch push is PUT separately and records its own blobs, so this is not a
/// coverage gap for the standard buildx flow.
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

/// Verify, store and index a manifest under `repository:reference`.
///
/// Content and pointers are separate: `oci_manifests` holds immutable bytes keyed
/// by their own hash, `oci_tags` holds the mutable pointer. So pushing the same
/// content under a second tag adds a tag without displacing the first, and
/// repointing a tag leaves the superseded manifest stored and pullable by digest.
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

    // Push-by-digest verification: when the caller addressed this PUT by a
    // content digest rather than a mutable tag, the pushed body must actually
    // hash to that digest — otherwise a poisoned reference gets stored and later
    // fails on pull instead of being rejected up front.
    if is_digest_reference(reference) && reference != digest {
        return Err(OciError::digest_invalid(format!(
            "digest mismatch: reference {reference} does not match computed digest {digest}"
        )));
    }

    let content: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| OciError::manifest_invalid(format!("invalid JSON manifest: {e}")))?;
    let raw_body = String::from_utf8(body.to_vec())
        .map_err(|e| OciError::manifest_invalid(format!("manifest is not valid UTF-8: {e}")))?;

    // One transaction for the whole push. Committing the manifest before its blob
    // references would leave, on any later failure, a pullable manifest whose
    // blobs are unrecorded — which a subsequent last-reference blob delete would
    // then reclaim.
    let mut tx = state.pool.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO oci_manifests (digest, repository, media_type, content, size_bytes)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (repository, digest) DO NOTHING
        "#,
    )
    .bind(&digest)
    .bind(repository)
    .bind(content_type)
    .bind(&raw_body)
    .bind(size_bytes)
    .execute(&mut *tx)
    .await?;

    if !is_digest_reference(reference) {
        sqlx::query(
            r#"
            INSERT INTO oci_tags (repository, tag, digest) VALUES ($1, $2, $3)
            ON CONFLICT (repository, tag) DO UPDATE SET
                digest = EXCLUDED.digest,
                updated_at = NOW()
            "#,
        )
        .bind(repository)
        .bind(reference)
        .bind(&digest)
        .execute(&mut *tx)
        .await?;
    }

    // Claim every blob this manifest references, so a delete elsewhere cannot
    // reclaim bytes this repository still needs. Sorted and deduplicated to give
    // every pusher the same digest-lock order — two concurrent pushes sharing
    // layers could otherwise deadlock.
    let mut blob_digests = extract_referenced_blob_digests(&content);
    blob_digests.sort();
    blob_digests.dedup();
    for blob_digest in &blob_digests {
        crate::ops::blobs::claim_blob(
            &mut tx,
            &state.storage,
            repository,
            blob_digest,
            OciCode::ManifestBlobUnknown,
        )
        .await?;
    }

    // Referrers API: a manifest declaring a `subject` is an attachment (SBOM,
    // signature, attestation) to the manifest that digest names.
    let subject = content
        .get("subject")
        .and_then(|s| s.get("digest"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    if let Some(ref subject_digest) = subject {
        let artifact_type = content
            .get("artifactType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let annotations = content.get("annotations").cloned();
        sqlx::query(
            r#"
            INSERT INTO oci_referrers (subject_digest, repository, referrer_digest, artifact_type, annotations, size_bytes)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (repository, subject_digest, referrer_digest) DO NOTHING
            "#,
        )
        .bind(subject_digest)
        .bind(repository)
        .bind(&digest)
        .bind(&artifact_type)
        .bind(&annotations)
        .bind(size_bytes)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(PutManifestResult { digest, subject })
}

/// Delete the manifest a reference resolves to. The foreign keys cascade every
/// tag that pointed at it and every referrer attached to it, so no pointer
/// outlives the content it named.
pub async fn delete_manifest(state: &OciState, repository: &str, reference: &str) -> Result<()> {
    let rows = sqlx::query(
        r#"
        DELETE FROM oci_manifests
        WHERE repository = $1
          AND digest = COALESCE(
              (SELECT t.digest FROM oci_tags t WHERE t.repository = $1 AND t.tag = $2),
              $2
          )
        "#,
    )
    .bind(repository)
    .bind(reference)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(OciError::manifest_unknown(format!(
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

/// Every manifest that declared `subject_digest` as its subject, optionally
/// narrowed to one `artifactType`.
///
/// There is exactly one `oci_manifests` row per `(repository, digest)`, so this
/// join yields one descriptor per referrer however many tags point at it.
pub async fn get_referrers(
    state: &OciState,
    repository: &str,
    subject_digest: &str,
    artifact_type: Option<&str>,
) -> Result<Vec<ReferrerEntry>> {
    // The subject is always a digest, never a tag, so a malformed one is a client
    // error rather than an empty result.
    if !is_digest_reference(subject_digest) {
        return Err(OciError::digest_invalid(format!(
            "subject '{subject_digest}' is not a digest"
        )));
    }

    let rows = sqlx::query(
        r#"
        SELECT r.referrer_digest, r.artifact_type, r.annotations, r.size_bytes,
               m.media_type
        FROM oci_referrers r
        JOIN oci_manifests m
          ON m.repository = r.repository AND m.digest = r.referrer_digest
        WHERE r.repository = $1 AND r.subject_digest = $2
          AND ($3::text IS NULL OR r.artifact_type = $3)
        ORDER BY r.created_at DESC
        "#,
    )
    .bind(repository)
    .bind(subject_digest)
    .bind(artifact_type)
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

/// Does this repository hold anything at all? A repository is not a record — it
/// exists exactly when some manifest carries its name — so this is the only
/// meaningful existence test, and it stays true for a repository whose manifests
/// are all digest-addressed and therefore untagged.
pub async fn repository_exists(state: &OciState, repository: &str) -> Result<bool> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_manifests WHERE repository = $1)")
            .bind(repository)
            .fetch_one(&state.pool)
            .await?;
    Ok(exists)
}

#[cfg(test)]
mod tests {
    use super::{extract_referenced_blob_digests, is_digest_reference};

    #[test]
    fn tag_references_are_not_digests() {
        assert!(!is_digest_reference("latest"));
        assert!(!is_digest_reference("v1.2.3"));
        assert!(!is_digest_reference("1.0.0-rc.1"));
    }

    #[test]
    fn sha256_references_are_digests() {
        assert!(is_digest_reference(&format!("sha256:{}", "a".repeat(64))));
        assert!(is_digest_reference(&format!("sha512:{}", "b".repeat(128))));
    }

    #[test]
    fn image_manifest_yields_config_and_every_layer() {
        let m = serde_json::json!({
            "config": {"digest": "sha256:cfg"},
            "layers": [{"digest": "sha256:l1"}, {"digest": "sha256:l2"}],
        });
        assert_eq!(
            extract_referenced_blob_digests(&m),
            vec!["sha256:cfg", "sha256:l1", "sha256:l2"]
        );
    }

    /// An index references other *manifests*, not blobs. Treating its entries as
    /// blob digests would claim references to things that are not blobs, and a
    /// later delete would then look for bytes that never existed.
    #[test]
    fn an_index_contributes_no_blob_digests() {
        let index = serde_json::json!({
            "manifests": [
                {"digest": "sha256:amd64", "platform": {"architecture": "amd64"}},
                {"digest": "sha256:arm64", "platform": {"architecture": "arm64"}},
            ],
        });
        assert!(extract_referenced_blob_digests(&index).is_empty());
    }
}
