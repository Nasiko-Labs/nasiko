//! Coding-agent session lifecycle: the "registry" (state carried on the
//! `chat_sessions` row), the one-active-turn lease, and startup reconciliation.
//!
//! There is deliberately **no separate registry service** — all of this lives on
//! the existing session row, so these are thin helpers over `UPDATE`s with a
//! fence-token guard that makes the lease safe across control-plane replicas.

use std::collections::HashSet;

use sqlx::PgPool;

/// Where a coding-agent session's container is in its lifecycle. Mirrors the
/// `runtime_state` CHECK constraint on `chat_sessions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRuntimeState {
    /// No container running — the default, and the cold-start state.
    Parked,
    /// A container is being brought up for this session.
    Starting,
    /// A container is live and serving.
    Running,
    /// A container is being torn down.
    Stopping,
}

impl SessionRuntimeState {
    /// The exact token stored in `chat_sessions.runtime_state`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parked => "parked",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
        }
    }
}

/// Proof that this control-plane replica holds the single active-turn lock for a
/// session.
///
/// Carries the `fence` token stamped at acquire time. Every later lease
/// operation re-checks it, so a replica that was presumed dead (its lease taken
/// over by another) cannot resurrect and corrupt the new owner's turn — its
/// fence no longer matches the row.
#[derive(Debug, Clone)]
pub struct TurnLease {
    pub session_id: String,
    pub owner: String,
    pub fence: i64,
}

/// Try to take the one-active-turn lock for `session_id`.
///
/// Succeeds only when no live lease exists — no current owner, or the previous
/// lease has already lapsed. On success the fence token is bumped and returned
/// inside a [`TurnLease`]; `Ok(None)` means another turn holds the session right
/// now, so the caller rejects or queues (never runs a second concurrent turn).
///
/// A session whose sandbox is being suspended (`runtime_state = 'stopping'`, set by the idle
/// reaper's fenced first step) is not acquirable either: starting a turn against a sandbox that
/// is about to be suspended would attach to a dying shim. It reads as `Ok(None)` like a busy
/// session; the harness turn path waits for the row to become `parked` and tries again.
pub async fn acquire_turn(
    db: &PgPool,
    session_id: &str,
    owner: &str,
    turn_id: &str,
    lease_secs: i64,
) -> sqlx::Result<Option<TurnLease>> {
    // Both the existing lease's freshness and the new expiry are judged by the *database*
    // clock (`now()`), so replicas with skewed clocks still agree on when a lease lapses.
    let fence: Option<i64> = sqlx::query_scalar(
        "UPDATE chat_sessions \
         SET turn_owner = $2, lease_expires_at = now() + ($3::bigint * interval '1 second'), \
             turn_fence = turn_fence + 1, active_turn_id = $4, cancel_requested_at = NULL \
         WHERE session_id = $1 \
           AND runtime_state <> 'stopping' \
           AND (turn_owner IS NULL OR lease_expires_at < now()) \
         RETURNING turn_fence",
    )
    .bind(session_id)
    .bind(owner)
    .bind(lease_secs)
    .bind(turn_id)
    .fetch_optional(db)
    .await?;

    Ok(fence.map(|fence| TurnLease {
        session_id: session_id.to_owned(),
        owner: owner.to_owned(),
        fence,
    }))
}

/// Renew a held lease so a long turn never lets its own lock lapse.
///
/// Returns `false` if the lease was lost — fenced out by a takeover, meaning
/// another owner may now be driving the session. The caller must then stop the
/// turn rather than keep writing to an agent it no longer owns.
pub async fn heartbeat(db: &PgPool, lease: &TurnLease, lease_secs: i64) -> sqlx::Result<bool> {
    Ok(heartbeat_flag(db, lease, lease_secs).await?.is_some())
}

/// [`heartbeat`] that also reports, from the same statement, whether a `tasks/cancel` asked for
/// this turn to stop: `Some(cancel_requested)` when the lease was renewed, `None` when it is lost.
/// The request sets `cancel_requested_at` on the row, so a turn on another replica than the one
/// that served it learns of it here, within one heartbeat. Cleared when the next turn acquires
/// the session.
pub async fn heartbeat_flag(
    db: &PgPool,
    lease: &TurnLease,
    lease_secs: i64,
) -> sqlx::Result<Option<bool>> {
    sqlx::query_scalar(
        "UPDATE chat_sessions \
         SET lease_expires_at = now() + ($3::bigint * interval '1 second') \
         WHERE session_id = $1 AND turn_owner = $2 AND turn_fence = $4 \
         RETURNING cancel_requested_at IS NOT NULL",
    )
    .bind(&lease.session_id)
    .bind(&lease.owner)
    .bind(lease_secs)
    .bind(lease.fence)
    .fetch_optional(db)
    .await
}

/// The outcome of one heartbeat tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    /// The lease was renewed.
    Renewed,
    /// The database answered and the lease is no longer ours (taken over, reset, or the session
    /// deleted). The only outcome that ends a turn.
    Lost,
    /// The database could not be reached within the retry budget. Says nothing about the lease:
    /// the next tick tries again, and a takeover in the meantime shows up as [`Beat::Lost`].
    Unreachable,
}

/// [`heartbeat`] that retries a database error with a bounded backoff (200 ms doubling to 2 s)
/// for up to `budget`, so a blip does not end a live turn. `Ok(false)` is never retried.
pub async fn heartbeat_retrying(
    db: &PgPool,
    lease: &TurnLease,
    lease_secs: i64,
    budget: std::time::Duration,
) -> Beat {
    heartbeat_retrying_flag(db, lease, lease_secs, budget)
        .await
        .0
}

/// [`heartbeat_retrying`] that also returns whether a cancel was requested (see
/// [`heartbeat_flag`]); `false` unless the lease was renewed.
pub async fn heartbeat_retrying_flag(
    db: &PgPool,
    lease: &TurnLease,
    lease_secs: i64,
    budget: std::time::Duration,
) -> (Beat, bool) {
    let deadline = tokio::time::Instant::now() + budget;
    let mut backoff = std::time::Duration::from_millis(200);
    loop {
        match heartbeat_flag(db, lease, lease_secs).await {
            Ok(Some(cancel)) => return (Beat::Renewed, cancel),
            Ok(None) => return (Beat::Lost, false),
            Err(e) => {
                tracing::warn!(session_id = %lease.session_id, error = %e, "lease heartbeat failed");
                if tokio::time::Instant::now() + backoff >= deadline {
                    return (Beat::Unreachable, false);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(std::time::Duration::from_secs(2));
            }
        }
    }
}

/// How a turn ended, as `tasks/get` reports it afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Failed,
    Canceled,
}

impl TurnOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

/// Record the finished turn and how it ended on the row, for `tasks/get`. Fenced on the lease, so
/// a turn that was superseded writes nothing; call it before [`release_turn`], which clears the
/// live turn id.
pub async fn record_outcome(
    db: &PgPool,
    lease: &TurnLease,
    turn_id: &str,
    outcome: TurnOutcome,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions SET last_turn_id = $4, last_turn_state = $5 \
         WHERE session_id = $1 AND turn_fence = $2 AND turn_owner = $3",
    )
    .bind(&lease.session_id)
    .bind(lease.fence)
    .bind(&lease.owner)
    .bind(turn_id)
    .bind(outcome.as_str())
    .execute(db)
    .await
    .map(|_| ())
}

/// Release the lease at turn end: clear the lock and stamp `last_turn_at` (which
/// drives the idle-shutdown decision). Fenced on `turn_fence` and refused on a `stopping` row,
/// so releasing a lease we no longer hold, or one a suspend claim has since taken over, is a
/// safe no-op: it cannot clear the claim's deadline.
pub async fn release_turn(db: &PgPool, lease: &TurnLease) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions \
         SET turn_owner = NULL, lease_expires_at = NULL, active_turn_id = NULL, \
             last_turn_at = now() \
         WHERE session_id = $1 AND turn_fence = $2 AND runtime_state <> 'stopping'",
    )
    .bind(&lease.session_id)
    .bind(lease.fence)
    .execute(db)
    .await?;
    Ok(())
}

/// Record the container now backing a session and its lifecycle state. A `None`
/// `runtime_handle` leaves the existing one untouched, so a plain state
/// transition need not re-supply it.
pub async fn set_runtime_state(
    db: &PgPool,
    session_id: &str,
    state: SessionRuntimeState,
    runtime_handle: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions \
         SET runtime_state = $2, \
             runtime_handle = COALESCE($3, runtime_handle) \
         WHERE session_id = $1",
    )
    .bind(session_id)
    .bind(state.as_str())
    .bind(runtime_handle)
    .execute(db)
    .await?;
    Ok(())
}

/// Persist the agent's own session id (from `session/new`) so a later cold turn
/// can `session/load` it. Acts only while the lease's fence still
/// matches and the row is not `stopping`, so a turn a suspend claim has superseded cannot write.
/// `false` means nothing was written.
pub async fn record_acp_session_fenced(
    db: &PgPool,
    lease: &TurnLease,
    acp_session_id: &str,
) -> sqlx::Result<bool> {
    sqlx::query(
        "UPDATE chat_sessions SET acp_session_id = $2 \
         WHERE session_id = $1 AND turn_fence = $3 AND turn_owner = $4 \
           AND runtime_state <> 'stopping'",
    )
    .bind(&lease.session_id)
    .bind(acp_session_id)
    .bind(lease.fence)
    .bind(&lease.owner)
    .execute(db)
    .await
    .map(|done| done.rows_affected() == 1)
}

/// The agent session id previously recorded for `session_id`, if any — the cue
/// for a cold turn to `session/load` rather than `session/new`.
pub async fn acp_session_id(db: &PgPool, session_id: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT acp_session_id FROM chat_sessions WHERE session_id = $1")
        .bind(session_id)
        .fetch_optional(db)
        .await
        .map(Option::flatten)
}

/// At control-plane startup, reconcile session rows against what is actually
/// running.
///
/// Any exec-based coding session whose row claims a live container (`runtime_state <> 'parked'`)
/// but whose handle is not in `live_handles`, and whose turn lease has lapsed, is reset to
/// `parked` with its lease cleared — otherwise a stale lease from a replica that died mid-turn
/// would block that session's next turn forever. Returns the number of rows reset.
///
/// Three things keep it off turns that are alive. Harness sessions are excluded: they have no
/// `runtime_handle` by design, and are reconciled by the sessions reaper through the provider.
/// A row with a live lease is left alone, and that is re-checked inside the `UPDATE` so a turn
/// that renews or starts between the read and the write is not reset. And the reset bumps the
/// fence, so a turn that was in fact dead cannot write under its old one afterwards.
pub async fn reconcile_startup(db: &PgPool, live_handles: &HashSet<String>) -> sqlx::Result<u64> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT s.session_id, s.runtime_handle FROM chat_sessions s \
         LEFT JOIN agents a ON a.id = s.agent_id \
         WHERE s.runtime_state <> 'parked' AND s.sandbox_ref IS NULL \
           AND (a.workload_kind IS NULL OR a.workload_kind <> 'harness') \
           AND (s.lease_expires_at IS NULL OR s.lease_expires_at < now())",
    )
    .fetch_all(db)
    .await?;

    let dead: Vec<String> = rows
        .into_iter()
        .filter(|(_, handle)| match handle {
            Some(h) => !live_handles.contains(h),
            // Claims a live state but names no container → nothing is alive.
            None => true,
        })
        .map(|(session_id, _)| session_id)
        .collect();

    if dead.is_empty() {
        return Ok(0);
    }

    let reset = sqlx::query(
        "UPDATE chat_sessions \
         SET runtime_state = 'parked', runtime_handle = NULL, turn_owner = NULL, \
             lease_expires_at = NULL, active_turn_id = NULL, turn_fence = turn_fence + 1 \
         WHERE session_id = ANY($1) AND runtime_state <> 'parked' \
           AND (lease_expires_at IS NULL OR lease_expires_at < now())",
    )
    .bind(&dead)
    .execute(db)
    .await?
    .rows_affected();
    Ok(reset)
}

#[cfg(test)]
mod tests {
    use super::{SessionRuntimeState, TurnOutcome};

    #[test]
    fn turn_outcome_strings_are_the_ones_the_migration_allows() {
        let migration = include_str!("../../../migrations/0089_session_turn_outcome.sql");
        for outcome in [
            TurnOutcome::Completed,
            TurnOutcome::Failed,
            TurnOutcome::Canceled,
        ] {
            assert!(
                migration.contains(&format!("'{}'", outcome.as_str())),
                "{outcome:?} is not in the last_turn_state CHECK"
            );
        }
    }

    #[test]
    fn runtime_state_strings_match_the_db_check_constraint() {
        // These must stay exactly in sync with the CHECK constraint in
        // migration 0048, or an UPDATE would be rejected at runtime.
        assert_eq!(SessionRuntimeState::Parked.as_str(), "parked");
        assert_eq!(SessionRuntimeState::Starting.as_str(), "starting");
        assert_eq!(SessionRuntimeState::Running.as_str(), "running");
        assert_eq!(SessionRuntimeState::Stopping.as_str(), "stopping");
    }
}
