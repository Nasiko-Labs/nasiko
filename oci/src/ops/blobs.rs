use bytes::Bytes;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::OciState;
use crate::error::{OciError, Result};

pub async fn blob_exists(state: &OciState, digest: &str) -> bool {
    state.storage.blob_exists(digest).await
}

pub async fn get_blob_redirect_url(state: &OciState, digest: &str) -> Result<String> {
    if !state.storage.blob_exists(digest).await {
        return Err(OciError::NotFound(format!("blob {digest} not found")));
    }
    state.storage.presigned_get_url(digest, 3600).await
}

pub async fn delete_blob(state: &OciState, digest: &str) -> Result<()> {
    if !state.storage.blob_exists(digest).await {
        return Err(OciError::NotFound(format!("blob {digest} not found")));
    }
    state.storage.delete_blob(digest).await
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
    let row = sqlx::query(
        "SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
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

    Ok(ChunkResult { upload_id, new_offset })
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
    let exists = sqlx::query(
        "SELECT uuid FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
    .bind(upload_id)
    .bind(repository)
    .fetch_optional(&state.pool)
    .await?
    .is_some();

    if !exists {
        return Err(OciError::NotFound("upload session not found".into()));
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
        && expected != computed {
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
