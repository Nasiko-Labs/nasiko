//! Row access for harness sessions on `chat_sessions`.
//!
//! `session_id` is the A2A contextId and the table's primary key, so a session is unique by
//! that id alone; `(agent_id, user_id)` are checked on top of it, never used to widen it.
//! Queries are runtime-checked (`sqlx::query`), like the rest of the server.

use chrono::{DateTime, Utc};
use nasiko_runtime::{RuntimeError, SandboxHandle, SandboxKey, SandboxProviderKind};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

pub use crate::coding::lifecycle::SessionRuntimeState;
use crate::coding::lifecycle::TurnLease;

// The config crate checks `AGENT_CALL_TIMEOUT_SECS` against its own copy of the session token
// lifetime (it sits below this crate); the two must not drift.
const _: () =
    assert!(super::identity::SESSION_TOKEN_TTL_SECS == nasiko_config::SESSION_TOKEN_TTL_SECS);

/// Namespace for deriving a sandbox key from a contextId that is not a UUID. Custom rather
/// than a standard one so the derivation is ours alone; changing it would orphan the
/// sandboxes of every non-UUID session.
const SANDBOX_KEY_NAMESPACE: Uuid = Uuid::from_u128(0x6e61_7369_6b6f_5f73_6573_7369_6f6e_0001);

/// How long a suspend claim (`stopping`) is honoured before another sweep may re-drive it. It
/// must cover the worst case of the two provider calls a suspend makes in a row: `status`, then
/// `suspend`. At default settings the worst cases are Docker: 30 s operation timeout for `status`
/// plus 40 s for `suspend` (30 s plus the 10 s stop grace); pod: 30 s plus 30 s. A remote provider
/// is the slow one: `status` and `suspend` can each run under a 180 s call timeout, and `suspend`
/// can chain several calls. 240 s covers the typical
/// case with margin for the database writes around the calls; a pathological suspend that
/// outlasts it is taken over and re-run, which is harmless because `suspend` is idempotent.
///
/// Two orderings are not enforced anywhere and are recorded here instead. The provider's
/// `operation_timeout` is configurable: a larger one needs a larger budget, or a slow but live
/// suspend is taken over and run twice (harmless, since `suspend` is idempotent, but wasteful).
/// The reaper interval (clamped to 5-600 s) is independent of this budget: it only decides how
/// soon a lapsed claim is noticed, not whether it has lapsed.
pub const SUSPEND_BUDGET_SECS: i64 = 240;

/// Harness sessions only: a `chat_sessions` row joined to the `agents` row of kind `harness`.
/// Every read goes through this, so no other kind of session is ever visible here.
const FROM: &str =
    "chat_sessions s JOIN agents a ON a.id = s.agent_id AND a.workload_kind = 'harness'";

const COLUMNS: &str = "s.session_id, s.user_id, s.agent_id, s.acp_session_id, \
    s.runtime_handle, s.runtime_state, s.volume_ref, s.turn_owner, s.lease_expires_at, \
    s.turn_fence, s.last_turn_at, s.active_turn_id, s.sandbox_ref, s.suspended_at, \
    s.workspace_repo, s.pending_permission, s.created_at";

/// One `chat_sessions` row, with the harness-session columns.
#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub session_id: String,
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub acp_session_id: Option<String>,
    pub runtime_handle: Option<String>,
    pub runtime_state: String,
    pub volume_ref: Option<String>,
    pub turn_owner: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub turn_fence: i64,
    pub last_turn_at: Option<DateTime<Utc>>,
    pub active_turn_id: Option<String>,
    /// `"<provider>:<backend_ref>"` from the `SandboxProvider`.
    pub sandbox_ref: Option<String>,
    pub suspended_at: Option<DateTime<Utc>>,
    pub workspace_repo: Option<String>,
    pub pending_permission: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl SessionRow {
    /// The UUID a sandbox is keyed by. `SandboxKey::Session` wants a UUID but a contextId may
    /// be client-minted text, so a UUID-shaped id is used as is and anything else is hashed
    /// deterministically (same id, same sandbox, across restarts).
    pub fn sandbox_uuid(&self) -> Uuid {
        Uuid::parse_str(&self.session_id)
            .unwrap_or_else(|_| Uuid::new_v5(&SANDBOX_KEY_NAMESPACE, self.session_id.as_bytes()))
    }

    /// When the session's sandbox last did anything the platform knows of: its last turn's end,
    /// or, for a session still marked as held by a turn (a crashed one, since only those reach
    /// the reaper), the later of that and when the lease lapsed. Calls the sandbox made before
    /// this are the dead turn's own, not signs of life. `None` when there is neither.
    pub fn idle_since(&self) -> Option<DateTime<Utc>> {
        match (
            self.turn_owner.is_some(),
            self.last_turn_at,
            self.lease_expires_at,
        ) {
            (true, last, lease) => last.max(lease),
            (false, last, _) => last,
        }
    }

    /// The session's sandbox handle, or why there is none: no `sandbox_ref`, a malformed one,
    /// or one owned by a provider other than the `wired` one (a `fake:` row must never be
    /// acted on by a real provider, nor the reverse). Always `RuntimeError::Internal`, since
    /// each case is a bad stored row, not a caller mistake.
    pub fn handle(&self, wired: SandboxProviderKind) -> Result<SandboxHandle, RuntimeError> {
        let sandbox_ref = self
            .sandbox_ref
            .as_deref()
            .ok_or_else(|| RuntimeError::Internal("session has no sandbox_ref".into()))?;
        let key = SandboxKey::Session {
            session_id: self.sandbox_uuid(),
        };
        let handle = SandboxHandle::parse(sandbox_ref, key)?;
        if handle.provider != wired {
            return Err(RuntimeError::Internal(format!(
                "sandbox_ref belongs to provider '{}', not the wired '{wired}'",
                handle.provider
            )));
        }
        Ok(handle)
    }
}

/// A listed sandbox, by every name a session row may know it under.
#[derive(Debug, Clone)]
pub struct SandboxLookup {
    /// `SandboxKey::name()`, recorded in `chat_sessions.sandbox_key` before the sandbox exists.
    pub key_name: String,
    /// `"<provider>:<backend_ref>"`, recorded once the sandbox is up.
    pub sandbox_ref: String,
    /// The UUID the key was derived from (the session id itself when it is UUID-shaped).
    pub key_uuid: Uuid,
}

/// A session row found for a sandbox, with whether it is deleted and the key it recorded.
#[derive(Debug, FromRow)]
pub struct SandboxOwner {
    #[sqlx(flatten)]
    pub session: SessionRow,
    pub sandbox_key: Option<String>,
    pub deleted: bool,
}

/// The row that owns `lookup`'s sandbox, by the strongest name first: the recorded key, then
/// the recorded ref, then the session id. The key is the only one written before the sandbox is
/// created and the only one that survives a non-UUID session id (whose sandbox UUID is a hash),
/// so a first turn that has not recorded its ref yet is still found.
pub fn owner_of<'a>(
    owners: &'a [SandboxOwner],
    lookup: &SandboxLookup,
) -> Option<&'a SandboxOwner> {
    let by_key = |o: &&SandboxOwner| o.sandbox_key.as_deref() == Some(lookup.key_name.as_str());
    let by_ref =
        |o: &&SandboxOwner| o.session.sandbox_ref.as_deref() == Some(lookup.sandbox_ref.as_str());
    let by_id = |o: &&SandboxOwner| o.session.session_id == lookup.key_uuid.to_string();
    owners
        .iter()
        .find(by_key)
        .or_else(|| owners.iter().find(by_ref))
        .or_else(|| owners.iter().find(by_id))
}

/// A session with its harness id and the key it sorts by in listings.
#[derive(Debug, FromRow)]
pub struct ListedSession {
    #[sqlx(flatten)]
    pub row: SessionRow,
    pub harness_id: Option<String>,
    pub sort_at: DateTime<Utc>,
}

/// Optional narrowing of a session listing.
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionFilter<'a> {
    pub harness: Option<&'a str>,
    pub state: Option<&'a str>,
}

/// Reads and writes harness sessions.
#[derive(Clone)]
pub struct SessionStore {
    db: PgPool,
}

impl SessionStore {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Open a fresh parked session for `user_id` on a harness agent, with its workspace repo
    /// set in the same statement. The session id (the contextId) is a new UUID.
    pub async fn create(
        &self,
        agent_id: Uuid,
        user_id: Uuid,
        repo: Option<&str>,
    ) -> sqlx::Result<SessionRow> {
        sqlx::query_as(&format!(
            "WITH s AS (INSERT INTO chat_sessions \
                 (session_id, user_id, agent_id, title, runtime_state, workspace_repo) \
                 VALUES ($1, $2, $3, 'Coding session', 'parked', $4) RETURNING *) \
             SELECT {COLUMNS} FROM s"
        ))
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(agent_id)
        .bind(repo)
        .fetch_one(&self.db)
        .await
    }

    /// The caller's session for `(agent, context)`, creating it parked if absent (the A2A
    /// path, where the client supplies the contextId).
    ///
    /// `None` means the contextId already belongs to someone else (or another agent); the
    /// caller must not learn whose, so it is reported as a conflict, not a row.
    pub async fn find_or_create_for_harness(
        &self,
        agent_id: Uuid,
        user_id: Uuid,
        context_id: &str,
    ) -> sqlx::Result<Option<SessionRow>> {
        sqlx::query(
            "INSERT INTO chat_sessions (session_id, user_id, agent_id, title, runtime_state) \
             VALUES ($1, $2, $3, 'Coding session', 'parked') ON CONFLICT (session_id) DO NOTHING",
        )
        .bind(context_id)
        .bind(user_id)
        .bind(agent_id)
        .execute(&self.db)
        .await?;
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM {FROM} \
             WHERE s.session_id = $1 AND s.user_id = $2 AND s.agent_id = $3 \
               AND s.deleted_at IS NULL"
        ))
        .bind(context_id)
        .bind(user_id)
        .bind(agent_id)
        .fetch_optional(&self.db)
        .await
    }

    /// A harness session by id, with the catalog id of the harness its agent runs; any other
    /// kind of chat session is `None`.
    pub async fn get_with_harness(
        &self,
        session_id: &str,
    ) -> sqlx::Result<Option<(SessionRow, Option<String>)>> {
        #[derive(FromRow)]
        struct WithHarness {
            #[sqlx(flatten)]
            row: SessionRow,
            harness_id: Option<String>,
        }
        let found: Option<WithHarness> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, a.harness_id FROM {FROM} \
             WHERE s.session_id = $1 AND s.deleted_at IS NULL"
        ))
        .bind(session_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(found.map(|f| (f.row, f.harness_id)))
    }

    /// A harness session by id; any other kind of chat session is `None`.
    pub async fn get(&self, session_id: &str) -> sqlx::Result<Option<SessionRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM {FROM} WHERE s.session_id = $1 AND s.deleted_at IS NULL"
        ))
        .bind(session_id)
        .fetch_optional(&self.db)
        .await
    }

    /// One page of `user_id`'s harness sessions, newest first by `COALESCE(last_turn_at,
    /// created_at)` with the session id as tiebreak, each with its harness id. `after` is the
    /// sort key and id of the last row of the previous page (keyset, so concurrent writes do
    /// not skew pages). Asks for `limit + 1` rows so the caller can tell whether more follow.
    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        filter: &SessionFilter<'_>,
        after: Option<&(DateTime<Utc>, String)>,
        limit: i64,
    ) -> sqlx::Result<Vec<ListedSession>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS}, a.harness_id, COALESCE(s.last_turn_at, s.created_at) AS sort_at \
             FROM {FROM} \
             WHERE s.user_id = $1 AND s.deleted_at IS NULL \
               AND ($2::text IS NULL OR a.harness_id = $2) \
               AND ($3::text IS NULL OR s.runtime_state = $3) \
               AND ($4::timestamptz IS NULL \
                    OR (COALESCE(s.last_turn_at, s.created_at), s.session_id) < ($4, $5)) \
             ORDER BY sort_at DESC, s.session_id DESC LIMIT $6"
        ))
        .bind(user_id)
        .bind(filter.harness)
        .bind(filter.state)
        .bind(after.map(|(at, _)| *at))
        .bind(after.map(|(_, id)| id.as_str()))
        .bind(limit + 1)
        .fetch_all(&self.db)
        .await
    }

    /// Delete a session row unless a turn or a suspend claim holds it, and give back the
    /// sandbox-token `jti` it carried (`Some(None)`: deleted, no token). The delete is fenced on
    /// `fence`, the value the caller read, so a turn that started since makes it miss.
    /// `None` means nothing was deleted: the row is gone, or busy, or its fence moved.
    pub async fn delete_idle(
        &self,
        session_id: &str,
        fence: i64,
    ) -> sqlx::Result<Option<Option<String>>> {
        sqlx::query_scalar(
            "DELETE FROM chat_sessions \
             WHERE session_id = $1 AND turn_fence = $2 \
               AND NOT ((turn_owner IS NOT NULL OR runtime_state = 'stopping') \
                        AND (lease_expires_at IS NULL OR lease_expires_at > now())) \
             RETURNING sandbox_token_jti",
        )
        .bind(session_id)
        .bind(fence)
        .fetch_optional(&self.db)
        .await
    }

    /// Claim the right to suspend a session's sandbox: `running` or `starting` → `stopping`,
    /// only if no live turn holds the lease (none, or one that has lapsed: a crashed turn),
    /// the turn fence still equals `fence` (the value the caller read when it decided), and a
    /// sandbox is recorded. `Some(claim_fence)` means this caller won and may call the
    /// provider; `None` means a turn started or another replica got there first, so the
    /// caller must leave the sandbox alone.
    ///
    /// A win takes the lease and the fence away from whatever turn held them: the owner and
    /// active turn are cleared and the fence is bumped, so the old turn's `heartbeat` and
    /// `release_turn` (fenced on the old value) miss and cannot disturb the claim. The returned
    /// fence is the claim's own, and every later call for this suspend carries it.
    ///
    /// The claim has a deadline ([`SUSPEND_BUDGET_SECS`]) in `lease_expires_at`, so a claimant
    /// that dies mid-suspend does not strand the row: once it passes,
    /// [`reclaim_suspend`](Self::reclaim_suspend) lets a later sweep finish the job.
    ///
    /// This is a compare-and-swap, and it is only half of the protocol: it holds because
    /// `coding::lifecycle::acquire_turn` treats `stopping` as not acquirable. A turn that
    /// could still take the lease on a `stopping` row would race the provider call.
    pub async fn begin_suspend(&self, session_id: &str, fence: i64) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar(
            "UPDATE chat_sessions \
             SET runtime_state = 'stopping', turn_owner = NULL, active_turn_id = NULL, \
                 turn_fence = turn_fence + 1, \
                 lease_expires_at = now() + ($3::bigint * interval '1 second') \
             WHERE session_id = $1 AND turn_fence = $2 AND sandbox_ref IS NOT NULL \
               AND deleted_at IS NULL AND runtime_state IN ('running', 'starting') \
               AND (turn_owner IS NULL OR lease_expires_at < now()) \
             RETURNING turn_fence",
        )
        .bind(session_id)
        .bind(fence)
        .bind(SUSPEND_BUDGET_SECS)
        .fetch_optional(&self.db)
        .await
    }

    /// Take over a `stopping` row whose claim deadline has passed (its claimant crashed or
    /// failed to settle it): push the deadline out and bump the fence, which cuts off the
    /// previous claimant. `Some(claim_fence)` means this caller may re-drive the suspend (it is
    /// idempotent at the provider); `None` means the deadline still holds or someone else won.
    pub async fn reclaim_suspend(&self, session_id: &str, fence: i64) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar(
            "UPDATE chat_sessions \
             SET turn_fence = turn_fence + 1, \
                 lease_expires_at = now() + ($3::bigint * interval '1 second') \
             WHERE session_id = $1 AND turn_fence = $2 AND runtime_state = 'stopping' \
               AND lease_expires_at < now() AND deleted_at IS NULL \
             RETURNING turn_fence",
        )
        .bind(session_id)
        .bind(fence)
        .bind(SUSPEND_BUDGET_SECS)
        .fetch_optional(&self.db)
        .await
    }

    /// Give back a claim whose suspend did not happen: `stopping` → `restore_to` (the state the
    /// session was claimed from: `running` or `starting`), with no lease left behind, so the
    /// session can be used or reaped again. Acts only on the claim's own fence, so a superseded
    /// claimant misses.
    pub async fn release_suspend(
        &self,
        session_id: &str,
        claim_fence: i64,
        restore_to: SessionRuntimeState,
    ) -> sqlx::Result<bool> {
        let done = sqlx::query(
            "UPDATE chat_sessions \
             SET runtime_state = $3, turn_owner = NULL, lease_expires_at = NULL, \
                 active_turn_id = NULL \
             WHERE session_id = $1 AND turn_fence = $2 AND runtime_state = 'stopping' \
               AND deleted_at IS NULL",
        )
        .bind(session_id)
        .bind(claim_fence)
        .bind(restore_to.as_str())
        .execute(&self.db)
        .await?
        .rows_affected()
            == 1;
        if !done {
            tracing::warn!(%session_id, claim_fence, "release_suspend: claim superseded; nothing written");
        }
        Ok(done)
    }

    /// Settle a claimed session as suspended: parked, with the moment it happened, and the
    /// claim's lease cleared. The `sandbox_ref` stays, because suspension keeps the sandbox and
    /// `acquire` resumes it. Acts only on a `stopping` row with the claim's own fence.
    pub async fn settle_suspended(&self, session_id: &str, claim_fence: i64) -> sqlx::Result<bool> {
        self.settle(
            "settle_suspended",
            "UPDATE chat_sessions \
             SET runtime_state = 'parked', suspended_at = now(), turn_owner = NULL, \
                 lease_expires_at = NULL, active_turn_id = NULL \
             WHERE session_id = $1 AND turn_fence = $2 AND runtime_state = 'stopping' \
               AND deleted_at IS NULL",
            session_id,
            claim_fence,
        )
        .await
    }

    /// Settle a claimed session whose sandbox the provider no longer has: parked, with no
    /// `sandbox_ref` and no suspension time, so the next turn creates a fresh sandbox. Acts
    /// only on a `stopping` row with the claim's own fence.
    pub async fn settle_cleared(&self, session_id: &str, claim_fence: i64) -> sqlx::Result<bool> {
        self.settle(
            "settle_cleared",
            "UPDATE chat_sessions \
             SET runtime_state = 'parked', sandbox_ref = NULL, suspended_at = NULL, \
                 turn_owner = NULL, lease_expires_at = NULL, active_turn_id = NULL \
             WHERE session_id = $1 AND turn_fence = $2 AND runtime_state = 'stopping' \
               AND deleted_at IS NULL",
            session_id,
            claim_fence,
        )
        .await
    }

    /// Run a claim-scoped update; a zero-row result means the claim was superseded, which is
    /// logged at warn because the caller's provider call has already happened.
    async fn settle(
        &self,
        what: &str,
        sql: &str,
        session_id: &str,
        claim_fence: i64,
    ) -> sqlx::Result<bool> {
        let done = sqlx::query(sql)
            .bind(session_id)
            .bind(claim_fence)
            .execute(&self.db)
            .await?
            .rows_affected()
            == 1;
        if !done {
            tracing::warn!(%session_id, claim_fence, "{what}: claim superseded; nothing written");
        }
        Ok(done)
    }

    /// Record the sandbox a turn started, but only while that turn still holds the session:
    /// the lease's fence and owner must match and the row must not be `stopping`. `false` means
    /// the turn was superseded (a suspend claim took the lease), so nothing was written.
    pub async fn set_sandbox_ref_fenced(
        &self,
        lease: &TurnLease,
        sandbox_ref: &str,
    ) -> sqlx::Result<bool> {
        sqlx::query(
            "UPDATE chat_sessions SET sandbox_ref = $2 \
             WHERE session_id = $1 AND turn_fence = $3 AND turn_owner = $4 \
               AND runtime_state <> 'stopping' AND deleted_at IS NULL",
        )
        .bind(&lease.session_id)
        .bind(sandbox_ref)
        .bind(lease.fence)
        .bind(&lease.owner)
        .execute(&self.db)
        .await
        .map(|done| done.rows_affected() == 1)
    }

    /// Move a session's runtime state on behalf of the turn holding `lease`; same guard as
    /// [`set_sandbox_ref_fenced`](Self::set_sandbox_ref_fenced).
    pub async fn set_runtime_state_fenced(
        &self,
        lease: &TurnLease,
        state: SessionRuntimeState,
    ) -> sqlx::Result<bool> {
        sqlx::query(
            "UPDATE chat_sessions SET runtime_state = $2 \
             WHERE session_id = $1 AND turn_fence = $3 AND turn_owner = $4 \
               AND runtime_state <> 'stopping' AND deleted_at IS NULL",
        )
        .bind(&lease.session_id)
        .bind(state.as_str())
        .bind(lease.fence)
        .bind(&lease.owner)
        .execute(&self.db)
        .await
        .map(|done| done.rows_affected() == 1)
    }

    /// The harness sessions that claim any of `lookups`' sandboxes, in one query. A row is
    /// returned if its recorded `sandbox_key`, `sandbox_ref` or session id matches a lookup;
    /// [`owner_of`] picks which row belongs to which sandbox.
    pub async fn find_for_sandboxes(
        &self,
        lookups: &[SandboxLookup],
    ) -> sqlx::Result<Vec<SandboxOwner>> {
        let keys: Vec<&str> = lookups.iter().map(|l| l.key_name.as_str()).collect();
        let refs: Vec<&str> = lookups.iter().map(|l| l.sandbox_ref.as_str()).collect();
        let ids: Vec<String> = lookups.iter().map(|l| l.key_uuid.to_string()).collect();
        sqlx::query_as(&format!(
            "SELECT {COLUMNS}, s.sandbox_key, s.deleted_at IS NOT NULL AS deleted FROM {FROM} \
             WHERE s.sandbox_key = ANY($1) OR s.sandbox_ref = ANY($2) OR s.session_id = ANY($3)"
        ))
        .bind(&keys)
        .bind(&refs)
        .bind(&ids)
        .fetch_all(&self.db)
        .await
    }

    /// Claim a session whose sandbox is live though the row says no turn is using it: `parked`,
    /// or `running`/`starting` with no `sandbox_ref` recorded. The same claim as
    /// [`begin_suspend`](Self::begin_suspend) (fence-bumped, lease-checked, with a deadline),
    /// and it records `sandbox_ref` so the row names the sandbox it is about to suspend.
    /// `None` when a turn took the session, or another sweep got there first.
    pub async fn claim_parked_with_live_sandbox(
        &self,
        session_id: &str,
        fence: i64,
        sandbox_ref: &str,
    ) -> sqlx::Result<Option<i64>> {
        sqlx::query_scalar(
            "UPDATE chat_sessions \
             SET runtime_state = 'stopping', turn_owner = NULL, active_turn_id = NULL, \
                 turn_fence = turn_fence + 1, sandbox_ref = COALESCE(sandbox_ref, $4), \
                 lease_expires_at = now() + ($3::bigint * interval '1 second') \
             WHERE session_id = $1 AND turn_fence = $2 AND deleted_at IS NULL \
               AND (turn_owner IS NULL OR lease_expires_at < now()) \
               AND (runtime_state = 'parked' \
                    OR (runtime_state IN ('running', 'starting') AND sandbox_ref IS NULL)) \
             RETURNING turn_fence",
        )
        .bind(session_id)
        .bind(fence)
        .bind(SUSPEND_BUDGET_SECS)
        .bind(sandbox_ref)
        .fetch_optional(&self.db)
        .await
    }

    /// Live sessions the idle sweep should look at: running or starting ones with a sandbox
    /// whose idle time (see [`SessionRow::idle_since`]) is more than `idle_ttl_secs` ago (judged by
    /// the database clock, which stamped it) and which no
    /// live turn holds (no lease, or a lapsed one: a crashed turn).
    pub async fn list_idle(&self, idle_ttl_secs: i64) -> sqlx::Result<Vec<SessionRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM {FROM} \
             WHERE s.sandbox_ref IS NOT NULL AND s.deleted_at IS NULL \
               AND s.runtime_state IN ('running', 'starting') \
               AND (s.turn_owner IS NULL OR s.lease_expires_at < now()) \
               AND COALESCE( \
                     CASE WHEN s.turn_owner IS NOT NULL \
                          THEN GREATEST(s.last_turn_at, s.lease_expires_at) \
                          ELSE s.last_turn_at END, \
                     s.created_at) < now() - ($1::bigint * interval '1 second')"
        ))
        .bind(idle_ttl_secs)
        .fetch_all(&self.db)
        .await
    }

    /// `stopping` sessions whose claim deadline has passed: a suspend whose claimant died or
    /// failed to settle it.
    pub async fn list_stuck(&self) -> sqlx::Result<Vec<SessionRow>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM {FROM} \
             WHERE s.runtime_state = 'stopping' AND s.lease_expires_at < now() \
               AND s.sandbox_ref IS NOT NULL AND s.deleted_at IS NULL"
        ))
        .fetch_all(&self.db)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(session_id: &str, key: Option<&str>, sandbox_ref: Option<&str>) -> SandboxOwner {
        let mut session = row(session_id);
        session.sandbox_ref = sandbox_ref.map(str::to_owned);
        SandboxOwner {
            session,
            sandbox_key: key.map(str::to_owned),
            deleted: false,
        }
    }

    fn lookup() -> SandboxLookup {
        SandboxLookup {
            key_name: "s-abc".into(),
            sandbox_ref: "docker:c1".into(),
            key_uuid: Uuid::nil(),
        }
    }

    #[test]
    fn the_recorded_key_finds_a_non_uuid_session_before_its_ref_exists() {
        let owners = vec![owner("ses_free_text", Some("s-abc"), None)];
        assert_eq!(
            owner_of(&owners, &lookup()).map(|o| o.session.session_id.as_str()),
            Some("ses_free_text")
        );
    }

    #[test]
    fn the_key_outranks_the_ref_and_the_ref_outranks_the_id() {
        let by_id = owner(&Uuid::nil().to_string(), None, None);
        let by_ref = owner("by_ref", None, Some("docker:c1"));
        let by_key = owner("by_key", Some("s-abc"), None);
        let all = vec![by_id, by_ref, by_key];
        assert_eq!(
            owner_of(&all, &lookup()).unwrap().session.session_id,
            "by_key"
        );
        assert_eq!(
            owner_of(&all[..2], &lookup()).unwrap().session.session_id,
            "by_ref"
        );
        assert_eq!(
            owner_of(&all[..1], &lookup()).unwrap().session.session_id,
            Uuid::nil().to_string()
        );
        assert!(owner_of(&[], &lookup()).is_none());
    }

    fn row(session_id: &str) -> SessionRow {
        SessionRow {
            session_id: session_id.into(),
            user_id: Uuid::nil(),
            agent_id: None,
            acp_session_id: None,
            runtime_handle: None,
            runtime_state: "parked".into(),
            volume_ref: None,
            turn_owner: None,
            lease_expires_at: None,
            turn_fence: 0,
            last_turn_at: None,
            active_turn_id: None,
            sandbox_ref: None,
            suspended_at: None,
            workspace_repo: None,
            pending_permission: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn uuid_session_id_is_its_own_sandbox_key() {
        let id = Uuid::new_v4();
        assert_eq!(row(&id.to_string()).sandbox_uuid(), id);
    }

    #[test]
    fn free_text_session_id_hashes_deterministically() {
        assert_eq!(row("ses_abc").sandbox_uuid(), row("ses_abc").sandbox_uuid());
        assert_ne!(row("ses_abc").sandbox_uuid(), row("ses_abd").sandbox_uuid());
    }
}
