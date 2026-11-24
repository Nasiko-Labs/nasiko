use bytes::Bytes;
use futures::TryStreamExt;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::OciState;
use crate::error::{OciCode, OciError, Result};
use nasiko_runtime::{BlobStore, BlobStoreError, BlobStream, BlobWriter};

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
    Ok(state.storage.get_blob(digest).await?)
}

/// Advisory-lock class for blob-digest locks, so they can never collide with any
/// other advisory lock a host might take. Postgres advisory locks are a bare
/// integer key space with no namespacing of its own.
const BLOB_LOCK_CLASS: i32 = 0x0C1_B10B;

/// Hold one blob digest exclusively for the rest of the transaction: what
/// reclaims take, since they decide whether the bytes go. Held per digest, so
/// unrelated blobs never contend. `hashtext` collisions merely make two digests
/// share a lock, which costs a little serialization and breaks nothing.
async fn lock_blob_digest(tx: &mut sqlx::PgConnection, digest: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(BLOB_LOCK_CLASS)
        .bind(digest)
        .execute(tx)
        .await?;
    Ok(())
}

/// The shared side of the same lock, for claims. Claims only add references, and
/// recording one twice is a no-op, so claims never need to exclude each other —
/// only a reclaim. Every agent image claims the same base layers, so an exclusive
/// lock here queued every push behind every other one, each holding a database
/// connection while it waited.
async fn lock_blob_digest_shared(tx: &mut sqlx::PgConnection, digest: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1, hashtext($2))")
        .bind(BLOB_LOCK_CLASS)
        .bind(digest)
        .execute(tx)
        .await?;
    Ok(())
}

/// Claim `digest` for `repository`: cancel any queued reclaim, confirm the bytes
/// are really there, then record the reference.
///
/// All three steps happen under the digest lock (shared: claims run side by side,
/// a reclaim waits for them all), and the order matters. A sweep in flight holds
/// that lock exclusively, so by the time this proceeds the bytes are either
/// definitively present or definitively gone — never mid-delete. The presence
/// check is also what the spec requires of a manifest push: a manifest whose
/// blobs aren't in the registry must be rejected rather than stored as a pullable
/// manifest that fails on its own layers.
///
/// A digest some repository already references needs no storage round trip:
/// bytes are only removed under the exclusive lock once nothing references them,
/// so while this holds the shared lock a reference proves the bytes are there.
pub(crate) async fn claim_blob(
    tx: &mut sqlx::PgConnection,
    storage: &dyn BlobStore,
    repository: &str,
    digest: &str,
    absent_code: OciCode,
) -> Result<()> {
    // Staged upload chunks share the store's key space but are not blobs.
    if digest.starts_with(UPLOAD_PART_PREFIX) {
        return Err(OciError::Oci(
            absent_code,
            format!("blob {digest} is not present in the registry"),
        ));
    }

    lock_blob_digest_shared(tx, digest).await?;

    sqlx::query("DELETE FROM oci_blob_gc WHERE digest = $1")
        .bind(digest)
        .execute(&mut *tx)
        .await?;

    let referenced: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oci_blob_refs WHERE digest = $1)")
            .bind(digest)
            .fetch_one(&mut *tx)
            .await?;
    if !referenced && !storage.blob_exists(digest).await {
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
        state.storage.as_ref(),
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

/// Hard cap on the total bytes one upload session may accumulate across all of
/// its chunks. Each chunk is already bounded at the HTTP body-read layer, but
/// nothing else stops an unbounded NUMBER of chunks. `complete_upload` streams
/// the staged chunks into the blob, so the cap bounds storage, not memory.
pub const MAX_UPLOAD_TOTAL_BYTES: i64 = 5 * 1024 * 1024 * 1024; // 5 GiB

/// How long an upload session may stay open before the abandoned-upload sweep treats it
/// as abandoned and removes it with its staged chunks. A layer push takes
/// minutes; a day leaves room for any real one.
const ABANDONED_UPLOAD_HOURS: i32 = 24;

/// Marks a store key as a staged upload chunk rather than a blob digest.
const UPLOAD_PART_PREFIX: &str = "uploads:";

/// Where one staged chunk of an upload session lives in the blob store.
///
/// Every chunk is its own object, named by a part id the server generates, so a
/// chunk outlives the request that sent it and any replica can read it back to
/// finish the upload. The key goes through the store's digest-keyed methods, and
/// both store layouts map it to `blobs/uploads/<session>/<part>`, apart from every
/// `blobs/<algorithm>/<hex>` digest. The part id never reaches a client, and
/// `claim_blob` refuses this prefix, so no manifest or mount can name a chunk.
fn upload_part_key(upload_id: Uuid, part_id: Uuid) -> String {
    format!("{UPLOAD_PART_PREFIX}{upload_id}:{part_id}")
}

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

/// Remove staged chunks. Best effort: a chunk that cannot be removed is stray
/// storage, never part of a blob, so the failure is logged rather than returned.
async fn delete_upload_parts(state: &OciState, upload_id: Uuid, part_ids: &[Uuid]) {
    for part_id in part_ids {
        let key = upload_part_key(upload_id, *part_id);
        if let Err(e) = state.storage.delete_blob(&key).await {
            tracing::warn!("could not remove staged chunk {part_id} of upload {upload_id}: {e}");
        }
    }
}

/// Delete a session and then its chunks, committed and staged; `false` when
/// there was no such session. The row goes first and hands back its chunk list
/// in the same statement, so a chunk a racing PATCH commits is either in that
/// list or refused by its own conditional update (which then removes it), never
/// stranded. A staged chunk whose replica died before committing it is in the
/// list too.
async fn remove_session(state: &OciState, repository: &str, upload_id: Uuid) -> Result<bool> {
    let part_ids: Option<Vec<Uuid>> = sqlx::query_scalar(
        "DELETE FROM oci_uploads WHERE uuid = $1 AND repository = $2
         RETURNING part_ids || staged_part_ids",
    )
    .bind(upload_id)
    .bind(repository)
    .fetch_optional(&state.pool)
    .await?;
    let Some(part_ids) = part_ids else {
        return Ok(false);
    };
    delete_upload_parts(state, upload_id, &part_ids).await;
    Ok(true)
}

/// Abandon a session, removing its staged chunks now instead of leaving them
/// for the abandoned-upload sweep.
pub async fn cancel_upload(state: &OciState, repository: &str, upload_id: Uuid) -> Result<()> {
    if !remove_session(state, repository, upload_id).await? {
        return Err(OciError::upload_unknown("upload session not found"));
    }
    Ok(())
}

/// Remove sessions open longer than [`ABANDONED_UPLOAD_HOURS`], with their
/// chunks, committed and staged. The server runs it at startup and hourly (the
/// artifact registry at startup): a client that never finishes or cancels an
/// upload would otherwise leave its chunks in storage for good.
pub async fn sweep_abandoned_uploads(state: &OciState) {
    let abandoned: Vec<(Uuid, Vec<Uuid>)> = match sqlx::query_as(
        "DELETE FROM oci_uploads WHERE created_at < now() - make_interval(hours => $1)
         RETURNING uuid, part_ids || staged_part_ids",
    )
    .bind(ABANDONED_UPLOAD_HOURS)
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("could not remove abandoned upload sessions: {e}");
            return;
        }
    };
    if abandoned.is_empty() {
        return;
    }

    tracing::info!("removing {} abandoned upload session(s)", abandoned.len());
    for (upload_id, part_ids) in abandoned {
        delete_upload_parts(state, upload_id, &part_ids).await;
    }
}

/// Tear down a session whose accumulated size would exceed the cap, so a
/// misbehaving client can't keep growing staged storage (or a dangling DB row)
/// by chunking one upload indefinitely.
async fn abort_oversized(state: &OciState, repository: &str, upload_id: Uuid) -> OciError {
    if let Err(e) = remove_session(state, repository, upload_id).await {
        tracing::warn!("could not remove oversized upload {upload_id}: {e}");
    }
    OciError::Oci(
        OciCode::BlobUploadInvalid,
        format!("upload exceeds maximum total size of {MAX_UPLOAD_TOTAL_BYTES} bytes"),
    )
}

/// A declared range that doesn't start exactly where the session left off means
/// client and registry disagree about progress. Appending anyway would silently
/// corrupt the blob, surfacing only as a digest mismatch at the end of a long
/// upload — or not at all, if the client omits the digest.
fn check_chunk_start(declared_start: Option<i64>, current_offset: i64) -> Result<()> {
    match declared_start {
        Some(start) if start != current_offset => Err(OciError::Oci(
            OciCode::RangeNotSatisfiable,
            format!("chunk starts at {start} but the session is at {current_offset}"),
        )),
        _ => Ok(()),
    }
}

/// `declared_start` is the offset the client claimed via `Content-Range`, when it
/// sent one; `None` means it made no claim and the chunk simply appends.
///
/// The chunk is staged in the blob store first, under a fresh part id, and then
/// committed to the session with one conditional update on the offset it was
/// checked against. Any replica may take any PATCH, so the offset in
/// `oci_uploads` is the only arbiter: of two racing chunks one commits and the
/// other finds the offset moved. A chunk that declared its start is then refused
/// with a `416` (it no longer continues the session); one that did not is simply
/// appended after the winner, since its staged bytes do not depend on where they
/// land. A refused chunk's object is removed. Non-empty chunks always move the
/// offset forward, which is what makes the offset a sound compare-and-set key.
pub async fn append_chunk(
    state: &OciState,
    repository: &str,
    upload_id: Uuid,
    chunk: Bytes,
    declared_start: Option<i64>,
) -> Result<ChunkResult> {
    let mut current_offset = upload_offset(state, repository, upload_id).await?;
    check_chunk_start(declared_start, current_offset)?;

    // Nothing to stage, and committing it could not be told apart from a race.
    if chunk.is_empty() {
        return Ok(ChunkResult {
            upload_id,
            new_offset: current_offset,
        });
    }

    let chunk_len = chunk.len() as i64;
    if current_offset + chunk_len > MAX_UPLOAD_TOTAL_BYTES {
        return Err(abort_oversized(state, repository, upload_id).await);
    }

    // Recorded as staged before the object is written: the store cannot be listed, so an object
    // no row names could never be found again if this replica died before committing it. Removing
    // the session deletes staged chunks along with committed ones.
    let part_id = Uuid::new_v4();
    stage_part(state, repository, upload_id, part_id).await?;
    if let Err(e) = state
        .storage
        .put_blob(&upload_part_key(upload_id, part_id), chunk)
        .await
    {
        unstage_part(state, upload_id, part_id).await;
        return Err(e.into());
    }

    loop {
        // A database error here leaves the staged object in place: the update
        // may have committed with only the reply lost, and deleting a committed
        // chunk would corrupt the session. The id stays staged, so the object
        // goes when the session does.
        let committed: Option<i64> = sqlx::query_scalar(
            "UPDATE oci_uploads
                SET offset_bytes = offset_bytes + $1,
                    part_ids = array_append(part_ids, $2),
                    staged_part_ids = array_remove(staged_part_ids, $2)
              WHERE uuid = $3 AND repository = $4 AND offset_bytes = $5
             RETURNING offset_bytes",
        )
        .bind(chunk_len)
        .bind(part_id)
        .bind(upload_id)
        .bind(repository)
        .bind(current_offset)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(new_offset) = committed {
            return Ok(ChunkResult {
                upload_id,
                new_offset,
            });
        }

        // The session is gone, or another chunk moved the offset first.
        let refusal = match upload_offset(state, repository, upload_id).await {
            Err(e) => e,
            Ok(offset) => {
                current_offset = offset;
                match check_chunk_start(declared_start, offset) {
                    Err(e) => e,
                    Ok(()) if offset + chunk_len > MAX_UPLOAD_TOTAL_BYTES => {
                        abort_oversized(state, repository, upload_id).await
                    }
                    Ok(()) => continue,
                }
            }
        };
        delete_upload_parts(state, upload_id, &[part_id]).await;
        unstage_part(state, upload_id, part_id).await;
        return Err(refusal);
    }
}

/// Record `part_id` as staged on the session before its object is written. Fails
/// when the session is gone, so nothing is written for it.
async fn stage_part(
    state: &OciState,
    repository: &str,
    upload_id: Uuid,
    part_id: Uuid,
) -> Result<()> {
    let staged = sqlx::query(
        "UPDATE oci_uploads SET staged_part_ids = array_append(staged_part_ids, $1)
          WHERE uuid = $2 AND repository = $3",
    )
    .bind(part_id)
    .bind(upload_id)
    .bind(repository)
    .execute(&state.pool)
    .await?;
    if staged.rows_affected() == 0 {
        return Err(OciError::upload_unknown("upload session not found"));
    }
    Ok(())
}

/// Forget a staged chunk whose object was never written or has been removed. Best
/// effort: a leftover id only costs a delete of a missing object later.
async fn unstage_part(state: &OciState, upload_id: Uuid, part_id: Uuid) {
    if let Err(e) = sqlx::query(
        "UPDATE oci_uploads SET staged_part_ids = array_remove(staged_part_ids, $1) WHERE uuid = $2",
    )
    .bind(part_id)
    .bind(upload_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!("could not unstage chunk {part_id} of upload {upload_id}: {e}");
    }
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
    claim_stored(state, repository, &computed).await?;
    Ok(computed)
}

/// Claim a blob just stored for `repository` (see [`store_and_claim`]).
async fn claim_stored(state: &OciState, repository: &str, digest: &str) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    claim_blob(
        &mut tx,
        state.storage.as_ref(),
        repository,
        digest,
        OciCode::BlobUnknown,
    )
    .await?;
    tx.commit().await?;
    Ok(())
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
    let session: Option<(i64, Vec<Uuid>)> = sqlx::query_as(
        "SELECT offset_bytes, part_ids FROM oci_uploads WHERE uuid = $1 AND repository = $2",
    )
    .bind(upload_id)
    .bind(repository)
    .fetch_optional(&state.pool)
    .await?;

    let Some((offset_bytes, part_ids)) = session else {
        return Err(OciError::upload_unknown("upload session not found"));
    };

    // append_chunk enforces the cap on every PATCH via this same column, but the
    // final chunk here was never checked — a client could PATCH to just under the
    // cap, then finalize with one more max-size chunk, pushing the assembled blob
    // well past it before anything caught it.
    if offset_bytes + final_chunk.len() as i64 > MAX_UPLOAD_TOTAL_BYTES {
        return Err(abort_oversized(state, repository, upload_id).await);
    }

    let digest = match expected_digest {
        Some(expected) => check_sha256_digest(expected)?.to_owned(),
        // Without a digest to write under, one pass only to learn it.
        None => {
            stream_upload(
                state,
                upload_id,
                &part_ids,
                offset_bytes,
                &final_chunk,
                None,
            )
            .await?
        }
    };
    write_upload(
        state,
        upload_id,
        &part_ids,
        offset_bytes,
        &final_chunk,
        &digest,
    )
    .await?;
    claim_stored(state, repository, &digest).await?;

    remove_session(state, repository, upload_id).await?;

    Ok(CompleteResult { digest })
}

/// Writes the session's bytes under `digest`, committing them only if they hash
/// to it. A blob store's piecewise write shows nothing under the key until it
/// finishes, so a mismatch leaves whatever was there, and a concurrent push of
/// the same digest can only ever write the same bytes.
async fn write_upload(
    state: &OciState,
    upload_id: Uuid,
    part_ids: &[Uuid],
    staged_len: i64,
    final_chunk: &Bytes,
    digest: &str,
) -> Result<()> {
    let mut writer = state.storage.blob_writer(digest).await?;
    let streamed = stream_upload(
        state,
        upload_id,
        part_ids,
        staged_len,
        final_chunk,
        Some(writer.as_mut()),
    )
    .await;
    match streamed {
        Ok(computed) if computed == digest => {
            writer.finish().await?;
            Ok(())
        }
        Ok(computed) => {
            writer.abort().await;
            Err(OciError::digest_invalid(format!(
                "digest mismatch: expected {digest}, got {computed}"
            )))
        }
        Err(e) => {
            writer.abort().await;
            Err(e)
        }
    }
}

/// The whole blob's digest: the session's staged chunks read back in commit
/// order, from whichever replicas took the PATCHes, then the PUT's own final
/// chunk, each piece also handed to `sink` when there is one.
///
/// Only one piece of a staged chunk is held at a time (plus the final chunk,
/// which arrived as the PUT's body), so the blob's size is bounded by
/// [`MAX_UPLOAD_TOTAL_BYTES`], not by memory. The bytes read back must be
/// exactly the bytes the session committed: a chunk missing from storage would
/// otherwise surface only as a digest mismatch, or not at all when the client
/// sent no digest.
async fn stream_upload(
    state: &OciState,
    upload_id: Uuid,
    part_ids: &[Uuid],
    staged_len: i64,
    final_chunk: &Bytes,
    mut sink: Option<&mut (dyn BlobWriter + '_)>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut staged = 0i64;
    for part_id in part_ids {
        let mut pieces = open_upload_part(state, upload_id, *part_id).await?;
        while let Some(piece) = pieces.try_next().await? {
            staged += piece.len() as i64;
            if staged > staged_len {
                return Err(staged_len_mismatch(upload_id, staged, staged_len));
            }
            hasher.update(&piece);
            if let Some(writer) = sink.as_deref_mut() {
                writer.write(piece).await?;
            }
        }
    }
    if staged != staged_len {
        return Err(staged_len_mismatch(upload_id, staged, staged_len));
    }
    hasher.update(final_chunk);
    if let Some(writer) = sink
        && !final_chunk.is_empty()
    {
        writer.write(final_chunk.clone()).await?;
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// One staged chunk, as a stream. Absence is the session's fault, not a missing
/// blob, so it answers `BLOB_UPLOAD_INVALID` rather than `BLOB_UNKNOWN`.
async fn open_upload_part(state: &OciState, upload_id: Uuid, part_id: Uuid) -> Result<BlobStream> {
    state
        .storage
        .get_blob_stream(&upload_part_key(upload_id, part_id))
        .await
        .map_err(|e| match e {
            BlobStoreError::NotFound(_) => OciError::Oci(
                OciCode::BlobUploadInvalid,
                format!("staged chunk {part_id} of upload {upload_id} is missing"),
            ),
            other => other.into(),
        })
}

/// The bytes read back must be exactly the bytes the session committed.
fn staged_len_mismatch(upload_id: Uuid, held: i64, staged_len: i64) -> OciError {
    OciError::Oci(
        OciCode::BlobUploadInvalid,
        format!("upload {upload_id} holds {held} of the {staged_len} bytes it recorded"),
    )
}

/// A completed upload is always stored as SHA-256, so any other digest the
/// client names could never match; refusing it up front also keeps it from
/// naming the key a write goes to.
fn check_sha256_digest(digest: &str) -> Result<&str> {
    let well_formed = digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    });
    if well_formed {
        Ok(digest)
    } else {
        Err(OciError::digest_invalid(format!(
            "unsupported digest {digest}: expected sha256:<64 lowercase hex>"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_chunk_key_never_lands_on_a_digest_path() {
        let key = upload_part_key(Uuid::new_v4(), Uuid::new_v4());
        assert!(key.starts_with(UPLOAD_PART_PREFIX));
        let path = crate::storage::S3Storage::blob_key(&key);
        assert!(path.starts_with("blobs/uploads/"), "{path}");
        assert!(!path.starts_with(&crate::storage::S3Storage::blob_key("sha256:")));
    }

    #[test]
    fn only_a_declared_start_off_the_offset_is_refused() {
        assert!(check_chunk_start(None, 10).is_ok());
        assert!(check_chunk_start(Some(10), 10).is_ok());
        assert!(matches!(
            check_chunk_start(Some(0), 10),
            Err(OciError::Oci(OciCode::RangeNotSatisfiable, _))
        ));
    }

    #[test]
    fn only_a_sha256_digest_can_name_a_completed_upload() {
        let good = sha256_of(b"layer");
        assert!(check_sha256_digest(&good).is_ok());
        for bad in [
            "sha512:abcd",
            "sha256:ABCD",
            "uploads:x:y",
            &good[..good.len() - 1],
            &good.to_uppercase(),
        ] {
            assert!(
                matches!(
                    check_sha256_digest(bad),
                    Err(OciError::Oci(OciCode::DigestInvalid, _))
                ),
                "{bad}"
            );
        }
    }
}
