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

    if let Some(expected) = expected_digest {
        if expected != computed {
            return Err(OciError::BadRequest(format!(
                "digest mismatch: expected {expected}, got {computed}"
            )));
        }
    }

    state.storage.put_blob(&computed, data).await?;

    sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    Ok(CompleteResult { digest: computed })
}
