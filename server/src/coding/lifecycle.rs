//! Coding-agent session lifecycle: the "registry" (state carried on the
//! `chat_sessions` row), the one-active-turn lease, and startup reconciliation.
//!
//! There is deliberately **no separate registry service** — all of this lives on
//! the existing session row, so these are thin helpers over `UPDATE`s with a
//! fence-token guard that makes the lease safe across control-plane replicas.

use std::collections::HashSet;

use chrono::{Duration, Utc};
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
pub async fn acquire_turn(
    db: &PgPool,
    session_id: &str,
    owner: &str,
    turn_id: &str,
    lease_secs: i64,
) -> sqlx::Result<Option<TurnLease>> {
    // The existing lease's freshness is judged by the *database* clock (`now()`),
    // so replicas with skewed clocks still agree on whether a lease has lapsed.
    let expires = Utc::now() + Duration::seconds(lease_secs);
    let fence: Option<i64> = sqlx::query_scalar(
        "UPDATE chat_sessions \
         SET turn_owner = $2, lease_expires_at = $3, turn_fence = turn_fence + 1, \
             active_turn_id = $4 \
         WHERE session_id = $1 \
           AND (turn_owner IS NULL OR lease_expires_at < now()) \
         RETURNING turn_fence",
    )
    .bind(session_id)
    .bind(owner)
    .bind(expires)
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
    let expires = Utc::now() + Duration::seconds(lease_secs);
    let rows = sqlx::query(
        "UPDATE chat_sessions SET lease_expires_at = $3 \
         WHERE session_id = $1 AND turn_owner = $2 AND turn_fence = $4",
    )
    .bind(&lease.session_id)
    .bind(&lease.owner)
    .bind(expires)
    .bind(lease.fence)
    .execute(db)
    .await?
    .rows_affected();
    Ok(rows == 1)
}

/// Release the lease at turn end: clear the lock and stamp `last_turn_at` (which
/// drives the idle-shutdown decision). Fenced on `turn_fence`, so releasing a
/// lease we no longer hold is a safe no-op.
pub async fn release_turn(db: &PgPool, lease: &TurnLease) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions \
         SET turn_owner = NULL, lease_expires_at = NULL, active_turn_id = NULL, \
             last_turn_at = now() \
         WHERE session_id = $1 AND turn_fence = $2",
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
/// can `session/load` it.
pub async fn record_acp_session(
    db: &PgPool,
    session_id: &str,
    acp_session_id: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE chat_sessions SET acp_session_id = $2 WHERE session_id = $1")
        .bind(session_id)
        .bind(acp_session_id)
        .execute(db)
        .await?;
    Ok(())
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
/// Any session whose row claims a live container (`runtime_state <> 'parked'`)
/// but whose handle is not in `live_handles` is reset to `parked` with its lease
/// cleared — otherwise a stale lease from a replica that died mid-turn would
/// block that session's next turn forever. Returns the number of rows reset.
pub async fn reconcile_startup(db: &PgPool, live_handles: &HashSet<String>) -> sqlx::Result<u64> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT session_id, runtime_handle FROM chat_sessions WHERE runtime_state <> 'parked'",
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
             lease_expires_at = NULL, active_turn_id = NULL \
         WHERE session_id = ANY($1)",
    )
    .bind(&dead)
    .execute(db)
    .await?
    .rows_affected();
    Ok(reset)
}

#[cfg(test)]
mod tests {
    use super::SessionRuntimeState;

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
