use bytes::Bytes;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::OciState;
use crate::error::{OciCode, OciError, Result};
use crate::storage::S3Storage;

pub async fn blob_exists(state: &OciState, digest: &str) -> bool {
    state.storage.blob_exists(digest).await
}

/// The digest of `data`, in the `sha256:<hex>` form the spec uses. Shared so
/// every caller derives a digest the same way.
pub fn sha256_of(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Was `digest` ever claimed by `repository`? The confidentiality gate for
/// GET/HEAD: repo-level ownership alone must NOT be sufficient to read an
/// arbitrary digest, since blobs are globally content-addressed and shared.
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
/// from the host server, rather than a presigned redirect straight to the
/// storage backend.
///
/// A redirect to the storage backend's own endpoint only works when that
/// endpoint is reachable from wherever the puller runs. For the default
/// self-hosted backend (RustFS behind a K8s ClusterIP) that's only true from
/// inside the cluster's pod network — but image pulls happen via
/// kubelet/containerd running in the **node's** network namespace, which can't
/// resolve any in-cluster service name at all, since CoreDNS is only wired into
/// pods' `/etc/resolv.conf`, not the node's own resolver. Found live: every real
/// K8s node's image pull 404'd on the presigned RustFS URL with a DNS lookup
/// failure. Streaming through the host reuses the exact path (ingress + TLS) that
/// manifest pulls already prove reachable, and keeps every byte behind this app's
/// own auth check instead of a bearer-token URL valid for anyone who has it.
pub async fn get_blob_bytes(state: &OciState, repository: &str, digest: &str) -> Result<Bytes> {
    if !blob_linked(state, repository, digest).await? {
        return Err(OciError::blob_unknown(format!("blob {digest} not found")));
    }
    if !state.storage.blob_exists(digest).await {
        return Err(OciError::blob_unknown(format!("blob {digest} not found")));
    }
    state.storage.get_blob(digest).await
}

/// Advisory-lock class for blob-digest locks, so they can never collide with any
/// other advisory lock a host might take. Postgres advisory locks are a bare
/// integer key space with no namespacing of its own.
const BLOB_LOCK_CLASS: i32 = 0x0C1_B10B;

/// Serialize everything that touches one blob digest — claims and reclaims — for
/// the rest of the transaction. Held per digest, so unrelated blobs never
/// contend. `hashtext` collisions merely make two digests share a lock, which
/// costs a little serialization and breaks nothing.
async fn lock_blob_digest(tx: &mut sqlx::PgConnection, digest: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(BLOB_LOCK_CLASS)
        .bind(digest)
        .execute(tx)
        .await?;
    Ok(())
}

/// Claim `digest` for `repository`: cancel any queued reclaim, confirm the bytes
/// are really there, then record the reference.
///
/// All three steps happen under the digest lock, and the order matters. A sweep
/// in flight holds that lock, so by the time this proceeds the bytes are either
/// definitively present or definitively gone — never mid-delete. The presence
/// check is also what the spec requires of a manifest push: a manifest whose
/// blobs aren't in the registry must be rejected rather than stored as a pullable
/// manifest that fails on its own layers.
pub(crate) async fn claim_blob(
    tx: &mut sqlx::PgConnection,
    storage: &S3Storage,
    repository: &str,
    digest: &str,
    absent_code: OciCode,
) -> Result<()> {
    lock_blob_digest(tx, digest).await?;

    sqlx::query("DELETE FROM oci_blob_gc WHERE digest = $1")
        .bind(digest)
        .execute(&mut *tx)
        .await?;

    if !storage.blob_exists(digest).await {
        return Err(OciError::Oci(
            absent_code,
            format!("blob {digest} is not present in the registry"),
        ));
    }

    sqlx::query(
        "INSERT INTO oci_blob_refs (digest, repository) VALUES ($1, $2)
         ON CONFLICT (digest, repository) DO NOTHING",
    )
    .bind(digest)
    .bind(repository)
    .execute(tx)
    .await?;

    Ok(())
}

/// Reclaim one queued blob. Re-checks the reference count under the digest lock
/// and either drops the tombstone (something claimed the digest again) or removes
/// the bytes and then the tombstone.
///
/// Every failure is one-sided. If the storage delete fails the transaction rolls
/// back and the tombstone stays queued for the next sweep. If the commit fails
/// after the storage delete, the tombstone also stays queued and the reference
/// count is still zero, so the next sweep repeats a delete that is already a
/// no-op and then clears the row. What cannot happen is a committed reference
/// pointing at bytes that are already gone: bytes are only ever removed while
/// holding this lock with zero references, and every claim path takes the same
/// lock and re-verifies presence.
pub async fn sweep_blob(state: &OciState, digest: &str) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    lock_blob_digest(&mut tx, digest).await?;

    let queued: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_blob_gc WHERE digest = $1)")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;
    if !queued {
        return Ok(());
    }

    let still_referenced: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_blob_refs WHERE digest = $1)")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;

    if !still_referenced {
        state.storage.delete_blob(digest).await?;
    }

    sqlx::query("DELETE FROM oci_blob_gc WHERE digest = $1")
        .bind(digest)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Drain the reclaim queue at startup. Tombstones committed by a request that
/// then crashed — or whose storage delete failed — would otherwise sit forever,
/// so the bytes they represent would never be reclaimed.
pub async fn sweep_pending_blob_gc(state: &OciState) {
    let queued: Vec<String> = match sqlx::query_scalar("SELECT digest FROM oci_blob_gc")
        .fetch_all(&state.pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("could not read the blob reclaim queue: {e}");
            return;
        }
    };
    if queued.is_empty() {
        return;
    }

    tracing::info!("reclaiming {} queued blob(s)", queued.len());
    for digest in queued {
        if let Err(e) = sweep_blob(state, &digest).await {
            tracing::warn!("could not reclaim blob {digest}, leaving it queued: {e}");
        }
    }
}

/// Reference-counted blob removal.
///
/// Blobs are content-addressed and shared across repositories, so the bytes only
/// go once no repository still claims them. This repository's reference is
/// dropped, and if that was the last one the digest is queued for reclaim — the
/// commit is the only durable decision made here. The physical delete then runs
/// as a best effort; if it fails the digest stays queued, because a Postgres
/// transaction cannot roll back a storage delete and deleting inline could commit
/// a reference back into place over bytes that were already gone.
pub async fn delete_blob(state: &OciState, repository: &str, digest: &str) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    lock_blob_digest(&mut tx, digest).await?;

    // This repo must have an actual recorded claim — fail closed rather than let
    // a repo affect a digest it never referenced.
    let removed = sqlx::query("DELETE FROM oci_blob_refs WHERE digest = $1 AND repository = $2")
        .bind(digest)
        .bind(repository)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(OciError::blob_unknown(format!(
            "blob {digest} not referenced by repository '{repository}'"
        )));
    }

    let still_referenced: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_blob_refs WHERE digest = $1)")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;

    if !still_referenced {
        sqlx::query("INSERT INTO oci_blob_gc (digest) VALUES ($1) ON CONFLICT (digest) DO NOTHING")
            .bind(digest)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    if !still_referenced && let Err(e) = sweep_blob(state, digest).await {
        tracing::warn!("blob {digest} queued for reclaim but not yet removed: {e}");
    }

    Ok(())
}

/// Cross-repository mount: record `repository`'s claim on bytes the registry
/// already holds. Blobs live in one global content-addressed key space, so there
/// is nothing to copy — mounting *is* the claim.
///
/// `from` is the source repository, and it is **required and verified**, not
/// advisory. Blob reads are gated on `blob_linked`, so a mount that accepted any
/// digest present anywhere in storage would be a way around that gate: a caller
/// who learned a digest belonging to a repository they cannot read could claim it
/// into one they own and then read it. The digest must actually be linked to
/// `from`; the caller's right to read `from` is checked by the route layer, which
/// is where authorization lives.
///
/// Returns `false` when the mount cannot be satisfied — no `from`, the source
/// does not claim the digest, or the bytes are absent. The spec requires that
/// case fall back to a normal upload session rather than failing the push, so the
/// client simply uploads the blob.
pub async fn mount_blob(
    state: &OciState,
    repository: &str,
    digest: &str,
    from: Option<&str>,
) -> Result<bool> {
    let Some(from) = from else {
        return Ok(false);
    };
    if !blob_linked(state, from, digest).await? {
        return Ok(false);
    }
    if !state.storage.blob_exists(digest).await {
        return Ok(false);
    }
    let mut tx = state.pool.begin().await?;
    claim_blob(
        &mut tx,
        &state.storage,
        repository,
        digest,
        OciCode::BlobUnknown,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
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
/// (`OciState::upload_buffers`) across ALL chunks of one upload session. Each
/// individual chunk is already bounded at the HTTP body-read layer, but nothing
/// otherwise stops an unbounded NUMBER of chunks — the buffer grows until
/// `complete_upload` flushes it, so a chunked upload could OOM the process well
/// before hitting any per-request cap. This is a stopgap: the
/// buffer-then-put-at-completion pattern still holds the whole blob in RAM even
/// under this cap. Streaming straight to the storage backend via S3 multipart
/// would remove the RAM ceiling entirely and is tracked as a follow-up.
pub const MAX_UPLOAD_TOTAL_BYTES: i64 = 5 * 1024 * 1024 * 1024; // 5 GiB

/// How much of this session the registry currently holds, for a client that lost
/// its own `Range` bookkeeping and wants to resume rather than restart.
pub async fn upload_offset(state: &OciState, repository: &str, upload_id: Uuid) -> Result<i64> {
    sqlx::query_scalar("SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2")
        .bind(upload_id)
        .bind(repository)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| OciError::upload_unknown("upload session not found"))
}

/// Abandon a session, freeing its buffer immediately instead of leaving it
/// pinned in memory until the process restarts.
pub async fn cancel_upload(state: &OciState, repository: &str, upload_id: Uuid) -> Result<()> {
    let removed = sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1 AND repository = $2")
        .bind(upload_id)
        .bind(repository)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(OciError::upload_unknown("upload session not found"));
    }
    state.upload_buffers.remove(&upload_id);
    Ok(())
}

/// Tear down a session whose accumulated size would exceed the cap, so a
/// misbehaving client can't keep an ever-growing allocation (or a dangling DB
/// row) alive by chunking one upload indefinitely.
async fn abort_oversized(state: &OciState, repository: &str, upload_id: Uuid) -> OciError {
    state.upload_buffers.remove(&upload_id);
    let _ = sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1 AND repository = $2")
        .bind(upload_id)
        .bind(repository)
        .execute(&state.pool)
        .await;
    OciError::Oci(
        OciCode::BlobUploadInvalid,
        format!("upload exceeds maximum total size of {MAX_UPLOAD_TOTAL_BYTES} bytes"),
    )
}

/// `declared_start` is the offset the client claimed via `Content-Range`, when it
/// sent one; `None` means it made no claim and the chunk simply appends.
pub async fn append_chunk(
    state: &OciState,
    repository: &str,
    upload_id: Uuid,
    chunk: Bytes,
    declared_start: Option<i64>,
) -> Result<ChunkResult> {
    // One critical section per upload, taken with `FOR UPDATE` on the session row.
    // Reading the offset outside a lock lets two concurrent PATCHes observe the
    // same `current_offset`, both append to the buffer, and both write the same
    // `new_offset` — leaving the buffer longer than the offset the registry
    // believes it holds, so the finished blob has duplicated bytes and fails its
    // digest check only at the very end of the upload.
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "SELECT offset_bytes FROM oci_uploads WHERE uuid = $1 AND repository = $2 FOR UPDATE",
    )
    .bind(upload_id)
    .bind(repository)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| OciError::upload_unknown("upload session not found"))?;

    let current_offset: i64 = row.try_get("offset_bytes")?;

    // A declared range that doesn't start exactly where the session left off
    // means client and registry disagree about progress. Appending anyway would
    // silently corrupt the blob, surfacing only as a digest mismatch at the end
    // of a long upload — or not at all, if the client omits the digest.
    if let Some(start) = declared_start
        && start != current_offset
    {
        return Err(OciError::Oci(
            OciCode::RangeNotSatisfiable,
            format!("chunk starts at {start} but the session is at {current_offset}"),
        ));
    }

    let chunk_len = chunk.len() as i64;
    let new_offset = current_offset + chunk_len;
    if new_offset > MAX_UPLOAD_TOTAL_BYTES {
        drop(tx);
        return Err(abort_oversized(state, repository, upload_id).await);
    }

    // Commit the offset BEFORE growing the buffer. The buffer is in memory and
    // cannot join the transaction, so one of the two has to go first, and this
    // order is the recoverable one: a failure here leaves the offset ahead of the
    // buffer, and the client's next chunk is refused with a `416` carrying the
    // real offset. The reverse order would leave bytes in the buffer that the
    // offset does not account for, and a retried chunk would append them twice.
    sqlx::query("UPDATE oci_uploads SET offset_bytes = $1 WHERE uuid = $2")
        .bind(new_offset)
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    state
        .upload_buffers
        .entry(upload_id)
        .or_default()
        .extend_from_slice(&chunk);

    Ok(ChunkResult {
        upload_id,
        new_offset,
    })
}

pub struct CompleteResult {
    pub digest: String,
}

/// Store `data` as a content-addressed blob and claim it for `repository`.
///
/// The claim happens here rather than waiting for a manifest to reference the
/// blob: without it, an upload that is never referenced — an abandoned or failed
/// push — holds no reference at all, so DELETE can only answer 404 and the bytes
/// are unreclaimable dead storage for good.
///
/// A reclaim sweep for the same digest can, very rarely, remove the bytes between
/// the put and the claim (only possible if the digest was already queued by
/// another repository). `claim_blob` verifies presence under the digest lock and
/// errors, so the client retries — a spurious retry, never a reference recorded
/// over missing bytes. The put deliberately stays outside the transaction so a
/// multi-gigabyte upload doesn't hold an advisory lock and a DB connection.
async fn store_and_claim(
    state: &OciState,
    repository: &str,
    data: Bytes,
    expected_digest: Option<&str>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let computed = format!("sha256:{}", hex::encode(hasher.finalize()));

    if let Some(expected) = expected_digest
        && expected != computed
    {
        return Err(OciError::digest_invalid(format!(
            "digest mismatch: expected {expected}, got {computed}"
        )));
    }

    state.storage.put_blob(&computed, data).await?;

    let mut tx = state.pool.begin().await?;
    claim_blob(
        &mut tx,
        &state.storage,
        repository,
        &computed,
        OciCode::BlobUnknown,
    )
    .await?;
    tx.commit().await?;

    Ok(computed)
}

/// Monolithic upload: the whole blob arrives in one request, no session needed.
pub async fn upload_blob_monolithic(
    state: &OciState,
    repository: &str,
    data: Bytes,
    expected_digest: &str,
) -> Result<CompleteResult> {
    let digest = store_and_claim(state, repository, data, Some(expected_digest)).await?;
    Ok(CompleteResult { digest })
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
        return Err(OciError::upload_unknown("upload session not found"));
    };

    // append_chunk enforces the cap on every PATCH via this same column, but the
    // final chunk here was never checked — a client could PATCH to just under the
    // cap, then finalize with one more max-size chunk, pushing the in-memory
    // buffer well past it before anything caught it.
    if offset_bytes + final_chunk.len() as i64 > MAX_UPLOAD_TOTAL_BYTES {
        return Err(abort_oversized(state, repository, upload_id).await);
    }

    let data = if let Some((_, mut buf)) = state.upload_buffers.remove(&upload_id) {
        if !final_chunk.is_empty() {
            buf.extend_from_slice(&final_chunk);
        }
        buf.freeze()
    } else {
        final_chunk
    };

    let digest = store_and_claim(state, repository, data, expected_digest).await?;

    sqlx::query("DELETE FROM oci_uploads WHERE uuid = $1")
        .bind(upload_id)
        .execute(&state.pool)
        .await?;

    Ok(CompleteResult { digest })
}
