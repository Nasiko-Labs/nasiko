//! SQL for agent state.
//!
//! Every statement filters on `agent_id`, which callers take from the authenticated credential
//! and from nowhere else: that filter is the whole isolation story, so another agent's row is
//! not forbidden, it is simply never selected.
//!
//! Reads ignore a row once `expires_at` has passed, whether or not the sweeper has dealt with it
//! yet, and ignore a tombstone always. Writes to one entry are serialised by an advisory lock and
//! run read, check, write inside one transaction, which is what makes a version number mean
//! something under concurrency.
//!
//! Versions never restart. A delete does not remove the row: it empties it into a tombstone
//! (`deleted_at` set, payload gone) that keeps the version, and the sweeper does the same to a row
//! that expires. The next write of that entry continues from the kept version, so an `If-Match`
//! taken before the delete or expiry can never match the new row by accident. A tombstone is
//! kept for one TTL, then forgotten; a version older than that is beyond any precondition this
//! API promises to honour.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::cursor::{KeyCursor, TaskCursor};
use super::precondition::Precondition;

/// Advisory-lock class for entry write locks. Postgres advisory locks are one shared integer key
/// space with no namespacing of its own, so every user of it picks a class of its own.
const ENTRY_LOCK_CLASS: i32 = 0x0A9E_517A;

/// The two stores, which share every version, expiry and locking rule.
#[derive(Clone, Copy, Debug)]
pub(super) enum Store {
    Tasks,
    Kv,
}

impl Store {
    pub(super) const ALL: [Self; 2] = [Self::Tasks, Self::Kv];

    /// The table that holds the store's rows.
    pub(super) fn table(self) -> &'static str {
        match self {
            Self::Tasks => "agent_tasks",
            Self::Kv => "agent_kv",
        }
    }

    /// The column that names a row within one agent's share of the store.
    pub(super) fn name_column(self) -> &'static str {
        match self {
            Self::Tasks => "task_id",
            Self::Kv => "key",
        }
    }

    /// Names the store inside a row's lock key and its ciphertext's associated data, so a task and
    /// a key of the same name never contend and never open as each other. Part of the stored
    /// format: changing it makes every stored row unreadable.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Tasks => "task",
            Self::Kv => "kv",
        }
    }

    /// The assignments that empty a row into a tombstone: everything the agent wrote except the
    /// name and the version. (A task's `state` stays; it is a protocol word, not content.)
    pub(super) fn emptied_columns(self) -> &'static str {
        match self {
            Self::Tasks => "body = NULL, context_id = NULL",
            Self::Kv => "value = NULL",
        }
    }

    fn versions_sql(self) -> String {
        let (table, name) = (self.table(), self.name_column());
        format!(
            "SELECT version, deleted_at IS NULL AND expires_at > now() AS live
             FROM {table} WHERE agent_id = $1 AND {name} = $2"
        )
    }

    /// Empties the entry into a tombstone kept for `$3` days.
    fn tombstone_sql(self) -> String {
        let (table, name, emptied) = (self.table(), self.name_column(), self.emptied_columns());
        format!(
            "UPDATE {table}
             SET {emptied},
                 deleted_at = clock_timestamp(),
                 expires_at = clock_timestamp() + make_interval(days => $3)
             WHERE agent_id = $1 AND {name} = $2"
        )
    }
}

/// One entry of one agent's store: what a write locks, versions and changes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Entry<'a> {
    pub(super) agent_id: Uuid,
    pub(super) store: Store,
    pub(super) name: &'a str,
}

/// A store's upsert, bound to everything but the version, which the write flow supplies.
type Upsert<'q> = QueryAs<'q, Postgres, Stamp, PgArguments>;

/// What a writer of one entry needs to know of it before it writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EntryVersions {
    /// Version of the live row, or 0 when there is none (absent, expired or a tombstone). This is
    /// what a precondition is checked against.
    current: i64,
    /// Highest version the entry has had, live or not, or 0 when nothing of it is kept.
    latest: i64,
}

impl EntryVersions {
    fn from_row(row: Option<(i64, bool)>) -> Self {
        match row {
            None => Self::default(),
            Some((version, live)) => Self {
                current: if live { version } else { 0 },
                latest: version,
            },
        }
    }

    /// The version the next write gets: one past every version the entry has had.
    fn next(self) -> i64 {
        self.latest + 1
    }
}

/// When a write landed: the row's new version and its timestamps.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct Stamp {
    pub(super) version: i64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
}

#[derive(Debug)]
pub(super) enum PutOutcome {
    Stored(Stamp),
    /// The precondition failed; nothing was written. 0 means the row is absent.
    VersionConflict {
        current_version: i64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DeleteOutcome {
    Deleted,
    NotFound,
    VersionConflict { current_version: i64 },
}

/// One page of rows, and whether another follows it.
#[derive(Debug)]
pub(super) struct Rows<T> {
    pub(super) rows: Vec<T>,
    pub(super) has_more: bool,
}

/// A row as a listing fetches it: the row, and how many rows the listing considered (the page
/// plus one probe row, before the byte bound), which is the same on every row it returns.
#[derive(Debug, sqlx::FromRow)]
struct Listed<T> {
    #[sqlx(flatten)]
    row: T,
    candidates: i64,
}

impl<T> Rows<T> {
    /// Makes a page of what a listing fetched. The listing considers one row beyond the page size
    /// to learn whether more exist, and may also have left rows out to stay within its byte
    /// bound; either way, more follow when it considered more rows than the page keeps.
    fn from_listing(listed: Vec<Listed<T>>, page_size: i64) -> Self {
        let candidates = listed.first().map_or(0, |first| first.candidates);
        let rows = listed.into_iter().map(|listed| listed.row).collect();
        Self::trimmed(rows, candidates, page_size)
    }

    /// Drops the probe row, if fetched, and reports whether any considered row is not on the page.
    fn trimmed(mut rows: Vec<T>, candidates: i64, page_size: i64) -> Self {
        rows.truncate(usize::try_from(page_size).unwrap_or_default());
        let kept = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        Self {
            rows,
            has_more: candidates > kept,
        }
    }
}

/// Wraps a listing's candidate query, ordered by `order` and limited already, into the statement
/// that bounds what it fetches by stored bytes. `sealed` is its stored payload column.
///
/// A row is fetched only while the stored size of the rows before it is within the budget (the
/// placeholder `budget_param`), so a page of large rows fetches a few of them, not a
/// page size's worth of them to throw most away after decrypting. The first row is always fetched,
/// however large, so a listing always makes progress. `octet_length` reads the stored size
/// without decompressing or fetching the value, and only fetched rows are read in full.
/// `count(*) OVER ()` reports how many rows were considered, so the caller knows whether the
/// byte bound or the page size left any out.
fn bounded_listing(candidates: &str, order: &str, sealed: &str, budget_param: &str) -> String {
    format!(
        "SELECT * FROM (
            SELECT candidate.*,
                   count(*) OVER () AS candidates,
                   coalesce(sum(octet_length(candidate.{sealed})) OVER (
                       ORDER BY {order}
                       ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS sealed_before
            FROM ({candidates}) candidate
        ) measured
        WHERE sealed_before <= {budget_param}
        ORDER BY {order}"
    )
}

// ─── Registration ───────────────────────────────────────────────────────────

/// Records that the agent keeps its tasks durably. Idempotent, and a no-op for an agent that
/// already is, so an SDK registering at every start does not rewrite the agent row each time.
/// `true` only when this call changed the agent from memory to durable.
pub(super) async fn mark_durable(db: &PgPool, agent_id: Uuid) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE agents SET state_mode = 'durable' WHERE id = $1 AND state_mode <> 'durable'",
    )
    .bind(agent_id)
    .execute(db)
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// Records that the agent keeps its tasks in memory again, as every deploy starts it: a new image
/// may not register a task store, and one that does re-registers when it starts.
pub(super) async fn mark_memory(db: &PgPool, agent_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agents SET state_mode = 'memory' WHERE id = $1 AND state_mode <> 'memory'")
        .bind(agent_id)
        .execute(db)
        .await?;
    Ok(())
}

// ─── Tasks ──────────────────────────────────────────────────────────────────

/// A task as an agent wants it stored. `sealed_body` is already ciphertext.
pub(super) struct NewTask<'a> {
    pub(super) task_id: &'a str,
    pub(super) context_id: Option<&'a str>,
    pub(super) state: &'a str,
    pub(super) sealed_body: &'a str,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct TaskRow {
    pub(super) task_id: String,
    pub(super) context_id: Option<String>,
    pub(super) state: String,
    pub(super) sealed_body: String,
    pub(super) version: i64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
}

/// Which tasks to list and where to start. Built from a validated request (`wire`).
#[derive(Debug)]
pub(super) struct TaskQuery {
    pub(super) context_id: Option<String>,
    pub(super) state: Option<String>,
    pub(super) after: Option<TaskCursor>,
    pub(super) page_size: i64,
}

const GET_TASK: &str =
    "SELECT task_id, context_id, state, body AS sealed_body, version, updated_at, expires_at
    FROM agent_tasks
    WHERE agent_id = $1 AND task_id = $2 AND deleted_at IS NULL AND expires_at > now()";

/// Newest first, then by id. `task_id` is a `COLLATE "C"` column, so the tie-break is byte order
/// whatever the database's locale, and `idx_agent_tasks_listing` serves this order and the
/// cursor's range without a sort.
const TASK_ORDER: &str = "updated_at DESC, task_id ASC";

/// The candidates of a task listing, before the byte bound (`bounded_listing`, budget `$7`).
const TASK_CANDIDATES: &str =
    "SELECT task_id, context_id, state, body AS sealed_body, version, updated_at, expires_at
    FROM agent_tasks
    WHERE agent_id = $1
      AND deleted_at IS NULL
      AND expires_at > now()
      AND ($2::text IS NULL OR context_id = $2)
      AND ($3::text IS NULL OR state = $3)
      AND ($4::timestamptz IS NULL
           OR updated_at < $4
           OR (updated_at = $4 AND task_id > $5))
    ORDER BY updated_at DESC, task_id ASC
    LIMIT $6";

/// Stamps with `clock_timestamp()`, not `now()`: `now()` is when the transaction began, which can
/// predate the moment this writer won the entry's lock, and a later version must never carry an
/// earlier `updated_at` than the one before it. The `created_at` expression is the exception, and
/// deliberately uses `now()` so it agrees with the liveness check made earlier in the same
/// transaction: it restarts a row that had expired or been deleted and keeps a live one's.
const UPSERT_TASK: &str = "INSERT INTO agent_tasks AS t
        (agent_id, task_id, context_id, state, body, version, updated_at, expires_at)
    VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp(), clock_timestamp() + make_interval(days => $7))
    ON CONFLICT (agent_id, task_id) DO UPDATE SET
        context_id = EXCLUDED.context_id,
        state      = EXCLUDED.state,
        body       = EXCLUDED.body,
        version    = EXCLUDED.version,
        created_at = CASE WHEN t.deleted_at IS NULL AND t.expires_at > now()
                          THEN t.created_at ELSE now() END,
        updated_at = EXCLUDED.updated_at,
        expires_at = EXCLUDED.expires_at,
        deleted_at = NULL
    RETURNING version, updated_at, expires_at";

pub(super) async fn get_task(
    db: &PgPool,
    agent_id: Uuid,
    task_id: &str,
) -> Result<Option<TaskRow>, sqlx::Error> {
    sqlx::query_as(GET_TASK)
        .bind(agent_id)
        .bind(task_id)
        .fetch_optional(db)
        .await
}

/// One page of tasks, fetching rows only while their stored payloads total at most
/// `sealed_bytes_budget` before the row (see `bounded_listing`).
pub(super) async fn list_tasks(
    db: &PgPool,
    agent_id: Uuid,
    query: &TaskQuery,
    sealed_bytes_budget: i64,
) -> Result<Rows<TaskRow>, sqlx::Error> {
    let sql = bounded_listing(TASK_CANDIDATES, TASK_ORDER, "sealed_body", "$7");
    let listed = sqlx::query_as(&sql)
        .bind(agent_id)
        .bind(query.context_id.as_deref())
        .bind(query.state.as_deref())
        .bind(query.after.as_ref().map(|cursor| cursor.updated_at))
        .bind(query.after.as_ref().map(|cursor| cursor.task_id.as_str()))
        .bind(query.page_size + 1)
        .bind(sealed_bytes_budget)
        .fetch_all(db)
        .await?;
    Ok(Rows::from_listing(listed, query.page_size))
}

/// Creates or replaces a task, at version 1 for a new one and one above the current otherwise.
pub(super) async fn put_task(
    db: &PgPool,
    agent_id: Uuid,
    task: &NewTask<'_>,
    precondition: Precondition,
    ttl_days: i32,
) -> Result<PutOutcome, sqlx::Error> {
    let entry = Entry {
        agent_id,
        store: Store::Tasks,
        name: task.task_id,
    };
    put_versioned(db, entry, precondition, |version| {
        sqlx::query_as(UPSERT_TASK)
            .bind(agent_id)
            .bind(task.task_id)
            .bind(task.context_id)
            .bind(task.state)
            .bind(task.sealed_body)
            .bind(version)
            .bind(ttl_days)
    })
    .await
}

// ─── Key-value ──────────────────────────────────────────────────────────────

/// An entry as an agent wants it stored. `sealed_value` is already ciphertext.
pub(super) struct NewEntry<'a> {
    pub(super) key: &'a str,
    pub(super) sealed_value: &'a str,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct KvRow {
    pub(super) key: String,
    pub(super) sealed_value: String,
    pub(super) version: i64,
    pub(super) updated_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
}

/// Which entries to list and where to start. Built from a validated request (`wire`).
#[derive(Debug)]
pub(super) struct KvQuery {
    pub(super) prefix: Option<String>,
    pub(super) after: Option<KeyCursor>,
    pub(super) page_size: i64,
}

const GET_KV: &str = "SELECT key, value AS sealed_value, version, updated_at, expires_at
    FROM agent_kv
    WHERE agent_id = $1 AND key = $2 AND deleted_at IS NULL AND expires_at > now()";

/// By key. `key` is a `COLLATE "C"` column, so the order is bytes, as for tasks.
const KV_ORDER: &str = "key ASC";

/// The candidates of a key-value listing, before the byte bound (`bounded_listing`, budget `$5`).
///
/// `starts_with`, not `LIKE`: a prefix is a literal, and `%` or `_` in it must not act as
/// wildcards. The primary key serves the order, the cursor's range and the prefix (Postgres can
/// use a btree for `starts_with` only under the C collation).
const KV_CANDIDATES: &str = "SELECT key, value AS sealed_value, version, updated_at, expires_at
    FROM agent_kv
    WHERE agent_id = $1
      AND deleted_at IS NULL
      AND expires_at > now()
      AND ($2::text IS NULL OR starts_with(key, $2))
      AND ($3::text IS NULL OR key > $3)
    ORDER BY key ASC
    LIMIT $4";

/// Stamps as `UPSERT_TASK` does, for the same reasons.
const UPSERT_KV: &str = "INSERT INTO agent_kv AS e
        (agent_id, key, value, version, updated_at, expires_at)
    VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp() + make_interval(days => $5))
    ON CONFLICT (agent_id, key) DO UPDATE SET
        value      = EXCLUDED.value,
        version    = EXCLUDED.version,
        created_at = CASE WHEN e.deleted_at IS NULL AND e.expires_at > now()
                          THEN e.created_at ELSE now() END,
        updated_at = EXCLUDED.updated_at,
        expires_at = EXCLUDED.expires_at,
        deleted_at = NULL
    RETURNING version, updated_at, expires_at";

pub(super) async fn get_kv(
    db: &PgPool,
    agent_id: Uuid,
    key: &str,
) -> Result<Option<KvRow>, sqlx::Error> {
    sqlx::query_as(GET_KV)
        .bind(agent_id)
        .bind(key)
        .fetch_optional(db)
        .await
}

/// One page of entries, bounded in fetched bytes as [`list_tasks`] is.
pub(super) async fn list_kv(
    db: &PgPool,
    agent_id: Uuid,
    query: &KvQuery,
    sealed_bytes_budget: i64,
) -> Result<Rows<KvRow>, sqlx::Error> {
    let sql = bounded_listing(KV_CANDIDATES, KV_ORDER, "sealed_value", "$5");
    let listed = sqlx::query_as(&sql)
        .bind(agent_id)
        .bind(query.prefix.as_deref())
        .bind(query.after.as_ref().map(|cursor| cursor.key.as_str()))
        .bind(query.page_size + 1)
        .bind(sealed_bytes_budget)
        .fetch_all(db)
        .await?;
    Ok(Rows::from_listing(listed, query.page_size))
}

/// Creates or replaces an entry, at version 1 for a new one and one above the current otherwise.
pub(super) async fn put_kv(
    db: &PgPool,
    agent_id: Uuid,
    entry: &NewEntry<'_>,
    precondition: Precondition,
    ttl_days: i32,
) -> Result<PutOutcome, sqlx::Error> {
    let target = Entry {
        agent_id,
        store: Store::Kv,
        name: entry.key,
    };
    put_versioned(db, target, precondition, |version| {
        sqlx::query_as(UPSERT_KV)
            .bind(agent_id)
            .bind(entry.key)
            .bind(entry.sealed_value)
            .bind(version)
            .bind(ttl_days)
    })
    .await
}

// ─── Shared by both stores ──────────────────────────────────────────────────

/// The write flow both stores share: take the entry's turn, check the precondition against its
/// live version, write it one past every version it has had, commit. `upsert` gets that version.
async fn put_versioned<'q>(
    db: &PgPool,
    entry: Entry<'_>,
    precondition: Precondition,
    upsert: impl FnOnce(i64) -> Upsert<'q>,
) -> Result<PutOutcome, sqlx::Error> {
    let mut tx = begin_locked(db, entry).await?;
    let versions = entry_versions(&mut tx, entry).await?;
    if !precondition.holds_for(versions.current) {
        tx.rollback().await?;
        return Ok(PutOutcome::VersionConflict {
            current_version: versions.current,
        });
    }
    let stamp = upsert(versions.next()).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(PutOutcome::Stored(stamp))
}

/// Empties a live row into a tombstone kept for `ttl_days`, so the entry's next write continues
/// its version. An absent, expired or already deleted one is `NotFound`, whatever the
/// precondition says.
pub(super) async fn delete(
    db: &PgPool,
    entry: Entry<'_>,
    precondition: Precondition,
    ttl_days: i32,
) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = begin_locked(db, entry).await?;
    let current_version = entry_versions(&mut tx, entry).await?.current;
    if current_version == 0 {
        tx.rollback().await?;
        return Ok(DeleteOutcome::NotFound);
    }
    if !precondition.holds_for(current_version) {
        tx.rollback().await?;
        return Ok(DeleteOutcome::VersionConflict { current_version });
    }
    sqlx::query(&entry.store.tombstone_sql())
        .bind(entry.agent_id)
        .bind(entry.name)
        .bind(ttl_days)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

/// Opens a transaction that holds the write lock for one entry until it ends, so concurrent
/// writers of that entry take turns and each reads the version the one before it committed.
///
/// A row lock cannot do this: when the entry does not exist yet there is no row to lock, and two
/// creators would both conclude they are first. The lock is per entry, so unrelated entries never
/// wait on each other; a hash collision only makes two entries share one, which costs a little
/// serialisation and breaks nothing.
///
/// The transaction is READ COMMITTED, said explicitly (in its own `BEGIN`, which saves a round
/// trip) because a database or role default can say otherwise. Taking turns only helps if each statement sees what the writer before committed:
/// under REPEATABLE READ or SERIALIZABLE the snapshot is fixed when the lock statement starts,
/// before the wait, so a writer that waited would read the version from before it and then fail
/// to serialize on write.
async fn begin_locked(
    db: &PgPool,
    entry: Entry<'_>,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let Entry {
        agent_id,
        store,
        name,
    } = entry;
    let mut tx = db
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(ENTRY_LOCK_CLASS)
        .bind(format!("{agent_id}/{}/{name}", store.label()))
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

async fn entry_versions(
    conn: &mut PgConnection,
    entry: Entry<'_>,
) -> Result<EntryVersions, sqlx::Error> {
    let row = sqlx::query_as(&entry.store.versions_sql())
        .bind(entry.agent_id)
        .bind(entry.name)
        .fetch_optional(conn)
        .await?;
    Ok(EntryVersions::from_row(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_never_written_starts_at_version_one() {
        let versions = EntryVersions::from_row(None);
        assert_eq!(versions.current, 0);
        assert_eq!(versions.next(), 1);
    }

    #[test]
    fn a_live_row_is_current_and_the_next_write_follows_it() {
        let versions = EntryVersions::from_row(Some((4, true)));
        assert_eq!(versions.current, 4);
        assert_eq!(versions.next(), 5);
    }

    #[test]
    fn a_tombstone_or_expired_row_is_absent_but_its_version_carries_on() {
        let versions = EntryVersions::from_row(Some((4, false)));
        assert_eq!(versions.current, 0, "a precondition sees no row");
        assert_eq!(versions.next(), 5, "the next write does not restart at 1");
    }

    #[test]
    fn a_full_page_with_a_probe_row_reports_more_and_drops_the_probe() {
        let page = Rows::trimmed(vec![1, 2, 3], 3, 2);
        assert_eq!(page.rows, [1, 2]);
        assert!(page.has_more);
    }

    #[test]
    fn a_page_without_a_probe_row_is_the_last() {
        let exact = Rows::trimmed(vec![1, 2], 2, 2);
        assert_eq!(exact.rows, [1, 2]);
        assert!(!exact.has_more);

        let short = Rows::trimmed(vec![1], 1, 2);
        assert_eq!(short.rows, [1]);
        assert!(!short.has_more);

        let empty = Rows::<i32>::trimmed(vec![], 0, 2);
        assert!(empty.rows.is_empty());
        assert!(!empty.has_more);
    }

    #[test]
    fn rows_the_byte_bound_left_out_mean_more_follow() {
        // Three rows considered, but only the first fitted the bound: the rest come next page.
        let cut = Rows::trimmed(vec![1], 3, 2);
        assert_eq!(cut.rows, [1]);
        assert!(cut.has_more);
    }

    #[test]
    fn a_listing_reports_what_it_considered_through_any_of_its_rows() {
        let listed = vec![
            Listed {
                row: "a",
                candidates: 3,
            },
            Listed {
                row: "b",
                candidates: 3,
            },
        ];
        let page = Rows::from_listing(listed, 5);
        assert_eq!(page.rows, ["a", "b"]);
        assert!(page.has_more, "a third row was considered and left out");

        let none = Rows::<&str>::from_listing(vec![], 5);
        assert!(none.rows.is_empty());
        assert!(!none.has_more);
    }
}
