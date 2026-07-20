use bytes::Bytes;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::OciState;
use crate::error::{OciError, Result};

pub async fn blob_exists(state: &OciState, digest: &str) -> bool {
    state.storage.blob_exists(digest).await
}

/// Was `digest` ever recorded as referenced by `repository` (via a pushed
/// manifest)? The confidentiality gate for GET/HEAD: repo-level ownership of
/// `repository` alone must NOT be sufficient to read an arbitrary digest,
/// since blobs are globally content-addressed and shared across repos.
pub async fn blob_linked(state: &OciState, repository: &str, digest: &str) -> Result<bool> {
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM oci_blob_refs WHERE digest = $1 AND repository = $2)",
    )
    .bind(digest)
    .bind(repository)
    .fetch_one(&state.pool)
    .await?;
    Ok(linked)
}

/// Fetches the full blob body from storage for the caller to receive directly
/// from `nasiko-server`, rather than a presigned redirect straight to the
/// storage backend.
///
/// A redirect to the storage backend's own endpoint only works when that
/// endpoint is reachable from wherever the puller runs. For the default
/// self-hosted backend (RustFS behind a K8s ClusterIP) that's only true from
/// inside the cluster's pod network — but image pulls happen via
/// kubelet/containerd running in the **node's** network namespace, which
/// can't resolve any in-cluster service name (bare or FQDN) at all, since
/// CoreDNS is only wired into pods' `/etc/resolv.conf`, not the node's own
/// resolver. Found live: every real K8s node's image pull 404'd on the
/// presigned RustFS URL with a DNS lookup failure. Streaming through
/// `nasiko-server` instead reuses the exact path (ingress + TLS) that
/// manifest pulls already prove reachable from real nodes, and keeps every
/// byte behind this app's own auth check instead of a bearer-token URL that's
/// valid for anyone who has it until it expires. Mirrors `ee/artifact-registry`'s
/// `get_blob`, which has always streamed directly for the same reason.
pub async fn get_blob_bytes(state: &OciState, repository: &str, digest: &str) -> Result<Bytes> {
    if !blob_linked(state, repository, digest).await? {
        return Err(OciError::NotFound(format!("blob {digest} not found")));
    }
    if !state.storage.blob_exists(digest).await {
        return Err(OciError::NotFound(format!("blob {digest} not found")));
    }
    state.storage.get_blob(digest).await
}

/// Ref-counted delete: only removes the physical object once no repository
/// still references the digest. Wrapped in one transaction so the
/// ref-count check and the physical delete are atomic against a concurrent
/// manifest push/delete from another repo sharing the same digest.
pub async fn delete_blob(state: &OciState, repository: &str, digest: &str) -> Result<()> {
    if !state.storage.blob_exists(digest).await {
        return Err(OciError::NotFound(format!("blob {digest} not found")));
    }

    let mut tx = state.pool.begin().await?;

    // This repo must have an actual recorded claim to the digest — fail
    // closed rather than let a repo affect a digest it never referenced
    // (including legacy pre-migration blobs with no oci_blob_refs rows yet).
    let removed = sqlx::query("DELETE FROM oci_blob_refs WHERE digest = $1 AND repository = $2")
        .bind(digest)
        .bind(repository)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(OciError::NotFound(format!(
            "blob {digest} not found in repository '{repository}'"
        )));
    }

    let still_referenced: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_blob_refs WHERE digest = $1)")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;

    if still_referenced {
        // Another repo still needs it — this repo's link is gone, object stays.
        tx.commit().await?;
        return Ok(());
    }

    let result = state.storage.delete_blob(digest).await;
    tx.commit().await?;
    result
}

pub async fn initiate_upload(state: &OciState, repository: &str) -> Result<Uuid> {
    let upload_id = Uuid::new_v4();

    sqlx::query("INSERT INTO oci_uploads (uuid, repository) VALUES ($1, $2)")
        .bind(upload_id)
        .bind(repository)
        .execute(&state.pool)
        .await?;

    Ok(upload_id)
}

pub struct ChunkResult {
    pub upload_id: Uuid,
    pub new_offset: i64,
}

/// Hard cap on the total bytes accumulated in the in-memory upload buffer
/// (`OciState::upload_buffers: DashMap<Uuid, BytesMut>`) across ALL chunks of
/// one upload session. Each individual chunk is already bounded to 512 MiB at
/// the HTTP body-read layer (see `oss/oci/src/routes/blobs.rs`), but nothing
/// previously stopped a client from sending an unbounded NUMBER of chunks —
/// the buffer grows without limit until `complete_upload` flushes it, so a
/// chunked upload could OOM the process well before hitting any per-request
/// cap. This is a stopgap: the buffer-then-put-at-completion pattern still
/// holds the whole blob in RAM even under this cap. A true streaming/
/// multipart upload straight to the storage backend (S3 supports
/// create_multipart_upload/upload_part/complete_multipart_upload) would
/// remove the RAM ceiling entirely and is tracked as a follow-up — this cap
/// only prevents unbounded growth, not the underlying buffering itself.
const MAX_UPLOAD_TOTAL_BYTES: i64 = 5 * 1024 * 1024 * 1024; // 5 GiB

pub async fn append_chunk(
    state: &OciState,
    repository: &str,
    upload_id: Uuid,
    chunk: Bytes,
) -> Result<ChunkResult> {
    let row =
        sqlx::query("SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2")
            .bind(upload_id)
            .bind(repository)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| OciError::NotFound("upload session not found".into()))?;

    let current_offset: i64 = row.try_get("offset_bytes")?;
    let chunk_len = chunk.len() as i64;

    // Reject BEFORE growing the buffer, and tear the upload session down
    // entirely on overflow so a single misbehaving/malicious client can't
    // keep an ever-growing allocation (or a dangling DB row) alive by
    // chunking one upload indefinitely.
    let new_offset = current_offset + chunk_len;
    if new_offset > MAX_UPLOAD_TOTAL_BYTES {
        state.upload_buffers.remove(&upload_id);
        let _ = sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1 AND repository = $2")
            .bind(upload_id)
            .bind(repository)
            .execute(&state.pool)
            .await;
        return Err(OciError::BadRequest(format!(
            "upload exceeds maximum total size of {MAX_UPLOAD_TOTAL_BYTES} bytes"
        )));
    }

    state
        .upload_buffers
        .entry(upload_id)
        .or_default()
        .extend_from_slice(&chunk);

    sqlx::query("UPDATE oci_uploads SET offset_bytes = $1 WHERE uuid = $2")
        .bind(new_offset)
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    Ok(ChunkResult {
        upload_id,
        new_offset,
    })
}

pub struct CompleteResult {
    pub digest: String,
}

pub async fn complete_upload(
    state: &OciState,
    repository: &str,
    upload_id: Uuid,
    final_chunk: Bytes,
    expected_digest: Option<&str>,
) -> Result<CompleteResult> {
    let offset_bytes: Option<i64> = sqlx::query_scalar(
        "SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
    .bind(upload_id)
    .bind(repository)
    .fetch_optional(&state.pool)
    .await?;

    let Some(offset_bytes) = offset_bytes else {
        return Err(OciError::NotFound("upload session not found".into()));
    };

    // append_chunk enforces MAX_UPLOAD_TOTAL_BYTES on every PATCH chunk (via
    // this same `offset_bytes` column), but the final chunk here was never
    // checked — a client could PATCH up to just under the cap, then finalize
    // with one more MAX_CHUNK_BYTES (512 MiB) chunk, pushing the actual
    // in-memory buffer ~512 MiB past the cap before anything caught it.
    if offset_bytes + final_chunk.len() as i64 > MAX_UPLOAD_TOTAL_BYTES {
        state.upload_buffers.remove(&upload_id);
        let _ = sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1 AND repository = $2")
            .bind(upload_id)
            .bind(repository)
            .execute(&state.pool)
            .await;
        return Err(OciError::BadRequest(format!(
            "upload exceeds maximum total size of {MAX_UPLOAD_TOTAL_BYTES} bytes"
        )));
    }

    let data = if let Some((_, mut buf)) = state.upload_buffers.remove(&upload_id) {
        if !final_chunk.is_empty() {
            buf.extend_from_slice(&final_chunk);
        }
        buf.freeze()
    } else {
        final_chunk
    };

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let computed = format!("sha256:{}", hex::encode(hasher.finalize()));

    if let Some(expected) = expected_digest
        && expected != computed
    {
        return Err(OciError::BadRequest(format!(
            "digest mismatch: expected {expected}, got {computed}"
        )));
    }

    state.storage.put_blob(&computed, data).await?;

    sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    Ok(CompleteResult { digest: computed })
}
