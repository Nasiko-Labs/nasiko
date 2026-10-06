//! The Postgres side of the workspace: everything *about* a file, never its bytes.
//!
//! Five tables (`oss/migrations/0056_workspace_catalog.sql`, indexes in `0057`):
//! - `workspace_files` — the one CURRENT pointer per `(agent_id, user_id, rel_path)`
//! - `workspace_file_versions` — pointers this path has superseded, newest last
//! - `workspace_uploads` — staging rows between `begin_upload` and `finish_upload`
//! - `workspace_file_grants` — view-only shares of a single file
//! - `agent_workspace_settings` — the per-agent limits every write is checked against
//!
//! **Blob first, row second.** A `blob_key` is minted here, the caller writes the
//! blob to the object store, and only then does it install the pointer. A crash
//! between the two leaves an unreferenced blob (swept later) rather than a pointer
//! to bytes that were never written — the failure a reader cannot recover from.
//! Blob keys are derived from identifiers alone, never from a user-supplied path.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// The current pointer for one path, as `workspace_files` stores it.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct FileRow {
    /// Stable file identity: survives overwrites, renames and tombstoning.
    pub id: Uuid,
    /// Agent whose workspace holds the file.
    pub agent_id: Uuid,
    /// Owner of the file within that workspace.
    pub user_id: Uuid,
    /// Owner-visible relative path, already validated by [`super::paths`].
    pub rel_path: String,
    /// Object-store key of the current bytes; immutable once written.
    pub blob_key: String,
    /// Size of the current bytes, counted against the per-`(agent, user)` quota.
    pub size_bytes: i64,
    /// MIME type recorded at write time.
    pub mime_type: String,
    /// `sha256` of the current bytes when the write path knew it.
    pub digest: Option<String>,
    /// How the file arrived: `agent`, `upload`, `cascade` or `shared_copy`.
    pub origin: String,
    /// Agent that produced the file, when it was not the owning agent.
    pub origin_agent_id: Option<Uuid>,
    /// File this one was copied from, for `shared_copy` origins.
    pub source_file_id: Option<Uuid>,
    /// W3C trace id of the flow that wrote this pointer.
    pub trace_id: String,
    /// Chat session the write belonged to, when there was one.
    pub session_id: Option<String>,
    /// `staged` (in the inbox, expires) or `kept` (permanent).
    pub state: String,
    /// When a `staged` file stops being answerable; `NULL` once kept.
    pub staged_until: Option<DateTime<Utc>>,
    /// First time this path was written.
    pub created_at: DateTime<Utc>,
    /// Last time this pointer changed.
    pub updated_at: DateTime<Utc>,
    /// Tombstone. Set by [`tombstone`], cleared by the next [`install_pointer`].
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One agent's workspace limits, as [`settings`] returns them.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct SettingsRow {
    /// Agent these limits belong to.
    pub agent_id: Uuid,
    /// Whether the agent has a workspace at all.
    pub enabled: bool,
    /// Ceiling on current + history bytes per `(agent, user)`.
    pub quota_bytes: i64,
    /// Largest single file accepted over a transfer URL.
    pub file_max_bytes: i64,
    /// Largest single file accepted (or returned) inline through a tool call.
    pub inline_max_bytes: i64,
    /// How long superseded versions are retained.
    pub history_retention_days: i32,
    /// Whether uploads land as `kept` instead of `staged`.
    pub auto_keep_uploads: bool,
    /// Whether tool results carry `resource_link` blocks.
    pub result_links: bool,
}

/// Everything [`install_pointer`] needs to make a path point at new bytes.
pub struct NewPointer<'a> {
    /// Agent whose workspace receives the file.
    pub agent_id: Uuid,
    /// Owner of the file within that workspace.
    pub user_id: Uuid,
    /// Validated relative path to install at.
    pub rel_path: &'a str,
    /// Key of the blob the caller has already written.
    pub blob_key: &'a str,
    /// Size of those bytes.
    pub size_bytes: i64,
    /// MIME type to record.
    pub mime_type: &'a str,
    /// `sha256` of the bytes, when known.
    pub digest: Option<&'a str>,
    /// `agent`, `upload`, `cascade` or `shared_copy`.
    pub origin: &'a str,
    /// Producing agent, when it was not the owning agent.
    pub origin_agent_id: Option<Uuid>,
    /// Source file, for `shared_copy` origins.
    pub source_file_id: Option<Uuid>,
    /// W3C trace id of the writing flow.
    pub trace_id: &'a str,
    /// Chat session the write belongs to, when there is one.
    pub session_id: Option<&'a str>,
    /// `staged` or `kept`.
    pub state: &'a str,
    /// Inbox expiry, set only alongside `state = "staged"`.
    pub staged_until: Option<DateTime<Utc>>,
}

/// Mints the object-store key for one upload's bytes.
///
/// Derived from identifiers only — never from `rel_path` — so no user-supplied
/// name can shape a key, and a rename never has to move a blob.
pub fn blob_key(agent_id: Uuid, user_id: Uuid, upload_id: Uuid) -> String {
    format!("workspace/{agent_id}/{user_id}/blobs/{upload_id}")
}

/// The object-store prefix holding every blob of one `(agent, user)` workspace.
pub fn user_prefix(agent_id: Uuid, user_id: Uuid) -> String {
    format!("workspace/{agent_id}/{user_id}/")
}

/// Reads one agent's workspace limits, inserting the schema defaults on first use.
///
/// The upsert's no-op `DO UPDATE` is what makes the row readable in the same
/// statement that creates it: `ON CONFLICT DO NOTHING` returns nothing when the
/// row already exists.
pub async fn settings(db: &PgPool, agent_id: Uuid) -> sqlx::Result<SettingsRow> {
    sqlx::query_as(
        "INSERT INTO agent_workspace_settings (agent_id) VALUES ($1)
         ON CONFLICT (agent_id) DO UPDATE SET updated_at = agent_workspace_settings.updated_at
         RETURNING agent_id, enabled, quota_bytes, file_max_bytes, inline_max_bytes, history_retention_days, auto_keep_uploads, result_links",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await
}

/// Bytes one `(agent, user)` workspace occupies: live pointers **and** retained
/// history, since both still hold blobs. Tombstoned pointers are excluded; their
/// superseded versions are not, until the sweeper removes them.
pub async fn bytes_used(db: &PgPool, agent_id: Uuid, user_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(size_bytes),0)::BIGINT FROM (
            SELECT size_bytes FROM workspace_files WHERE agent_id=$1 AND user_id=$2 AND deleted_at IS NULL
            UNION ALL
            SELECT v.size_bytes FROM workspace_file_versions v JOIN workspace_files f ON f.id=v.file_id
             WHERE f.agent_id=$1 AND f.user_id=$2) t",
    )
    .bind(agent_id)
    .bind(user_id)
    .fetch_one(db)
    .await
}

/// Blob first, row second (the caller stored the blob already). One transaction:
/// current pointer → history, new pointer installed, tombstone cleared. Returns
/// the row and its new version number (1 + number of history rows).
pub async fn install_pointer(db: &PgPool, p: NewPointer<'_>) -> sqlx::Result<(FileRow, i64)> {
    let mut tx: Transaction<'_, Postgres> = db.begin().await?;
    let existing: Option<FileRow> = sqlx::query_as(
        "SELECT * FROM workspace_files WHERE agent_id=$1 AND user_id=$2 AND rel_path=$3 FOR UPDATE",
    )
    .bind(p.agent_id)
    .bind(p.user_id)
    .bind(p.rel_path)
    .fetch_optional(&mut *tx)
    .await?;
    let row: FileRow = match existing {
        Some(old) => {
            // A tombstoned pointer is not archived: the user deleted those bytes,
            // so re-using the path must not resurrect them as history.
            if old.deleted_at.is_none() {
                sqlx::query(
                    "INSERT INTO workspace_file_versions (file_id, blob_key, size_bytes, digest, origin, trace_id, session_id, created_at)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(old.id)
                .bind(&old.blob_key)
                .bind(old.size_bytes)
                .bind(&old.digest)
                .bind(&old.origin)
                .bind(&old.trace_id)
                .bind(&old.session_id)
                .bind(old.updated_at)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query_as(
                "UPDATE workspace_files SET blob_key=$2, size_bytes=$3, mime_type=$4, digest=$5, origin=$6, origin_agent_id=$7,
                    source_file_id=$8, trace_id=$9, session_id=$10, state=$11, staged_until=$12, deleted_at=NULL, updated_at=now()
                 WHERE id=$1 RETURNING *",
            )
            .bind(old.id)
            .bind(p.blob_key)
            .bind(p.size_bytes)
            .bind(p.mime_type)
            .bind(p.digest)
            .bind(p.origin)
            .bind(p.origin_agent_id)
            .bind(p.source_file_id)
            .bind(p.trace_id)
            .bind(p.session_id)
            .bind(p.state)
            .bind(p.staged_until)
            .fetch_one(&mut *tx)
            .await?
        }
        None => {
            sqlx::query_as(
                "INSERT INTO workspace_files (agent_id,user_id,rel_path,blob_key,size_bytes,mime_type,digest,origin,origin_agent_id,source_file_id,trace_id,session_id,state,staged_until)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING *",
            )
            .bind(p.agent_id)
            .bind(p.user_id)
            .bind(p.rel_path)
            .bind(p.blob_key)
            .bind(p.size_bytes)
            .bind(p.mime_type)
            .bind(p.digest)
            .bind(p.origin)
            .bind(p.origin_agent_id)
            .bind(p.source_file_id)
            .bind(p.trace_id)
            .bind(p.session_id)
            .bind(p.state)
            .bind(p.staged_until)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_file_versions WHERE file_id=$1")
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok((row, versions + 1))
}

/// Looks a live pointer up by path. Tombstoned rows are invisible here, so a
/// deleted path reads as absent.
pub async fn get_by_path(
    db: &PgPool,
    agent_id: Uuid,
    user_id: Uuid,
    rel_path: &str,
) -> sqlx::Result<Option<FileRow>> {
    sqlx::query_as(
        "SELECT * FROM workspace_files WHERE agent_id=$1 AND user_id=$2 AND rel_path=$3 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(rel_path)
    .fetch_optional(db)
    .await
}

/// Looks a pointer up by id, **including** tombstoned rows — callers that act on
/// a known file id (grants, history, the sweeper) must still see a deleted one.
pub async fn get_by_id(db: &PgPool, id: Uuid) -> sqlx::Result<Option<FileRow>> {
    sqlx::query_as("SELECT * FROM workspace_files WHERE id=$1")
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Which slice of a workspace [`list`] returns.
pub enum Scope {
    /// Every kept file.
    All,
    /// Kept files written during one chat session.
    Session(String),
    /// The inbox: staged files only, never kept ones.
    Inbox,
}

/// Lists a user's files in one agent's workspace, newest first.
///
/// `prefix` matches `rel_path` from the left; `Scope::Inbox` returns `staged`
/// rows and every other scope returns `kept` rows, so the inbox and the
/// workspace proper are always disjoint. Tombstoned rows are never returned.
pub async fn list(
    db: &PgPool,
    agent_id: Uuid,
    user_id: Uuid,
    prefix: Option<&str>,
    scope: Scope,
    limit: i64,
) -> sqlx::Result<Vec<FileRow>> {
    let (session, inbox) = match scope {
        Scope::All => (None, false),
        Scope::Session(s) => (Some(s), false),
        Scope::Inbox => (None, true),
    };
    sqlx::query_as(
        "SELECT * FROM workspace_files
         WHERE agent_id=$1 AND user_id=$2 AND deleted_at IS NULL
           AND ($3::text IS NULL OR rel_path LIKE $3 || '%')
           AND ($4::text IS NULL OR session_id = $4)
           AND (state = CASE WHEN $5 THEN 'staged' ELSE 'kept' END)
         ORDER BY updated_at DESC LIMIT $6",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(prefix)
    .bind(session)
    .bind(inbox)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Files one flow produced, oldest first — the set a turn can cite as results.
/// Only agent-written origins qualify; a user's own upload is not a result.
pub async fn files_for_trace(db: &PgPool, trace_id: &str) -> sqlx::Result<Vec<FileRow>> {
    sqlx::query_as(
        "SELECT * FROM workspace_files WHERE trace_id=$1 AND deleted_at IS NULL AND origin IN ('agent','cascade') ORDER BY updated_at",
    )
    .bind(trace_id)
    .fetch_all(db)
    .await
}

/// Marks a pointer deleted. The row and its blob survive for the sweeper; the
/// next write to the same path clears the tombstone instead of archiving it.
pub async fn tombstone(db: &PgPool, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE workspace_files SET deleted_at=now(), updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(db)
        .await
        .map(|_| ())
}

/// Moves a pointer to a new path. The blob does not move — keys never encode a path.
pub async fn rename(db: &PgPool, id: Uuid, new_rel_path: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE workspace_files SET rel_path=$2, updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(new_rel_path)
        .execute(db)
        .await
        .map(|_| ())
}

/// Promotes a staged file out of the inbox, dropping its expiry.
pub async fn keep(db: &PgPool, id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE workspace_files SET state='kept', staged_until=NULL, updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .execute(db)
    .await
    .map(|_| ())
}

// ---- uploads ----

/// One in-flight upload, as `workspace_uploads` stages it between
/// `begin_upload` and `finish_upload`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UploadRow {
    /// Identity of the upload; also names its blob (see [`blob_key`]).
    pub upload_id: Uuid,
    /// Agent whose workspace the file is destined for.
    pub agent_id: Uuid,
    /// Owner the finished file will belong to.
    pub user_id: Uuid,
    /// Validated relative path the pointer will be installed at.
    pub rel_path: String,
    /// Size the client announced, before any bytes arrived.
    pub declared_size: Option<i64>,
    /// MIME type the client announced.
    pub mime_type: Option<String>,
    /// Key the bytes are written to.
    pub blob_key: String,
    /// `direct` (client writes to the store) or `relay` (bytes pass through us).
    pub mode: String,
    /// Hash of the one-shot transfer capability, for relay mode.
    pub capability_hash: Option<String>,
    /// `pending`, `received`, `finished` or `abandoned`.
    pub state: String,
    /// Bytes actually received, once they have been.
    pub received_size: Option<i64>,
    /// `sha256` of the received bytes.
    pub received_digest: Option<String>,
    /// W3C trace id of the flow that began the upload.
    pub trace_id: String,
    /// Chat session the upload belongs to, when there is one.
    pub session_id: Option<String>,
    /// When an unfinished upload becomes sweepable.
    pub expires_at: DateTime<Utc>,
}

/// Stages a new upload. Always starts `pending`; the row's `state`,
/// `received_size` and `received_digest` fields are set later, not inserted.
pub async fn create_upload(db: &PgPool, u: &UploadRow) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO workspace_uploads (upload_id,agent_id,user_id,rel_path,declared_size,mime_type,blob_key,mode,capability_hash,state,trace_id,session_id,expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending',$10,$11,$12)",
    )
    .bind(u.upload_id)
    .bind(u.agent_id)
    .bind(u.user_id)
    .bind(&u.rel_path)
    .bind(u.declared_size)
    .bind(&u.mime_type)
    .bind(&u.blob_key)
    .bind(&u.mode)
    .bind(&u.capability_hash)
    .bind(&u.trace_id)
    .bind(&u.session_id)
    .bind(u.expires_at)
    .execute(db)
    .await
    .map(|_| ())
}

/// Reads one staging row by id, in any state.
pub async fn get_upload(db: &PgPool, upload_id: Uuid) -> sqlx::Result<Option<UploadRow>> {
    sqlx::query_as("SELECT * FROM workspace_uploads WHERE upload_id=$1")
        .bind(upload_id)
        .fetch_optional(db)
        .await
}

/// Resolves a relay transfer's capability to its staging row. One-shot: only a
/// `pending`, unexpired upload matches, so a replayed capability finds nothing.
pub async fn get_upload_by_capability(
    db: &PgPool,
    capability_hash: &str,
) -> sqlx::Result<Option<UploadRow>> {
    sqlx::query_as(
        "SELECT * FROM workspace_uploads WHERE capability_hash=$1 AND state='pending' AND expires_at > now()",
    )
    .bind(capability_hash)
    .fetch_optional(db)
    .await
}

/// Records the bytes that arrived. Guarded on `pending`, so two concurrent
/// transfers of one capability cannot both claim the upload.
pub async fn mark_received(
    db: &PgPool,
    upload_id: Uuid,
    size: i64,
    digest: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE workspace_uploads SET state='received', received_size=$2, received_digest=$3 WHERE upload_id=$1 AND state='pending'",
    )
    .bind(upload_id)
    .bind(size)
    .bind(digest)
    .execute(db)
    .await
    .map(|_| ())
}

/// Moves an upload to `state`, stamping `finished_at` only on `finished`.
pub async fn mark_upload_state(db: &PgPool, upload_id: Uuid, state: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE workspace_uploads SET state=$2, finished_at=CASE WHEN $2='finished' THEN now() ELSE finished_at END WHERE upload_id=$1",
    )
    .bind(upload_id)
    .bind(state)
    .execute(db)
    .await
    .map(|_| ())
}

/// A batch of expired uploads that never finished — the sweeper's work list of
/// orphaned blobs. Capped so one pass cannot read the whole backlog.
pub async fn unfinished_uploads_past(
    db: &PgPool,
    now: DateTime<Utc>,
) -> sqlx::Result<Vec<UploadRow>> {
    sqlx::query_as(
        "SELECT * FROM workspace_uploads WHERE state IN ('pending','received') AND expires_at <= $1 LIMIT 200",
    )
    .bind(now)
    .fetch_all(db)
    .await
}

// ---- grants ----

/// One view-only share of a single file.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct GrantRow {
    /// Identity of the grant, as [`remove_grant`] addresses it.
    pub id: Uuid,
    /// File being shared.
    pub file_id: Uuid,
    /// `user`, `team`, `department` or `public`.
    pub grant_type: String,
    /// Who the share is for — an id as text, or `*` for `public`.
    pub grantee_id: String,
    /// User who granted it.
    pub granted_by: Uuid,
    /// Agent through which it was granted, when one was involved.
    pub granting_agent_id: Option<Uuid>,
    /// When the grant was first created.
    pub created_at: DateTime<Utc>,
}

/// Shares a file, or re-attributes an existing identical share. Idempotent on
/// `(file_id, grant_type, grantee_id)`: re-granting returns the same row.
pub async fn add_grant(
    db: &PgPool,
    file_id: Uuid,
    grant_type: &str,
    grantee_id: &str,
    granted_by: Uuid,
    granting_agent_id: Option<Uuid>,
) -> sqlx::Result<GrantRow> {
    sqlx::query_as(
        "INSERT INTO workspace_file_grants (file_id, grant_type, grantee_id, granted_by, granting_agent_id) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (file_id, grant_type, grantee_id) DO UPDATE SET granted_by = EXCLUDED.granted_by RETURNING *",
    )
    .bind(file_id)
    .bind(grant_type)
    .bind(grantee_id)
    .bind(granted_by)
    .bind(granting_agent_id)
    .fetch_one(db)
    .await
}

/// Every share of one file, oldest first.
pub async fn grants_for_file(db: &PgPool, file_id: Uuid) -> sqlx::Result<Vec<GrantRow>> {
    sqlx::query_as("SELECT * FROM workspace_file_grants WHERE file_id=$1 ORDER BY created_at")
        .bind(file_id)
        .fetch_all(db)
        .await
}

/// Revokes a share. Returns the number of rows deleted — `0` when the grant was
/// already gone, which lets callers tell a revoke from a no-op.
pub async fn remove_grant(db: &PgPool, grant_id: Uuid) -> sqlx::Result<u64> {
    sqlx::query("DELETE FROM workspace_file_grants WHERE id=$1")
        .bind(grant_id)
        .execute(db)
        .await
        .map(|r| r.rows_affected())
}

/// Files shared *to* this user directly or publicly. Team/department resolution is
/// the enterprise access policy's job; OSS answers with `user` + `public` rows.
///
/// Excludes the user's own files: an owner reaches those through [`list`], and a
/// self-grant must not make a file appear twice.
pub async fn shared_with_user(
    db: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> sqlx::Result<Vec<FileRow>> {
    sqlx::query_as(
        "SELECT f.* FROM workspace_files f JOIN workspace_file_grants g ON g.file_id=f.id
         WHERE f.deleted_at IS NULL AND f.state='kept' AND f.user_id <> $1
           AND ((g.grant_type='user' AND g.grantee_id=$1::text) OR g.grant_type='public')
         ORDER BY f.updated_at DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
}
