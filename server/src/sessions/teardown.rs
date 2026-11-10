//! What must happen to a session's sandbox and credential when the session row goes away: the
//! sandbox is released at the provider and the sandbox token is revoked. Both are best effort
//! (the delete itself must not fail because a provider is down), and each failure is logged.
//!
//! A row that is deleted, or has its `agent_id` nulled by an agent delete, stops being visible to
//! the reaper's session queries, so this is the prompt path; the reaper's orphan pass
//! (`reaper::recover_orphans`) is the backstop that releases a sandbox no row claims, should a
//! release here fail or the process die first.

use super::model::{SessionRow, SessionStore};
use super::token::{revoke_jti, revoke_sandbox_token};
use crate::state::AppState;

/// Release the sandbox of every session in `session_ids` and revoke its token, before the rows
/// are deleted. Sessions without a sandbox or token are skipped.
pub async fn release_and_revoke(state: &AppState, session_ids: &[String]) {
    let store = SessionStore::new(state.db.clone());
    for session_id in session_ids {
        release_sandbox_of(state, &store, session_id).await;
        if let Err(e) = revoke_sandbox_token(&state.db, session_id).await {
            tracing::warn!(%session_id, error = %e, "could not revoke a sandbox token on teardown");
        }
    }
}

/// The ids of `user_id`'s sessions that hold a sandbox or a sandbox token.
pub async fn sessions_of_user_with_sandbox(
    state: &AppState,
    user_id: uuid::Uuid,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT session_id FROM chat_sessions WHERE user_id = $1 \
         AND (sandbox_ref IS NOT NULL OR sandbox_token_jti IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
}

/// Release the sandbox a session holds, found by reading its row. Used for a row about to be
/// deleted; a row that cannot be read or a provider that refuses is logged and left.
pub async fn release_sandbox_of(state: &AppState, store: &SessionStore, session_id: &str) {
    let row = match store.get(session_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "could not read a session to release its sandbox");
            return;
        }
    };
    release_row_sandbox(state, &row).await;
}

/// Release the sandbox a row names, if any. For a caller that read the row before deleting it.
pub async fn release_row_sandbox(state: &AppState, row: &SessionRow) {
    if row.sandbox_ref.is_none() {
        return;
    }
    let session_id = &row.session_id;
    match row.handle(state.sandbox.kind()) {
        Ok(handle) => {
            if let Err(e) = state.sandbox.release(&handle).await {
                tracing::warn!(%session_id, error = %e, "could not release a deleted session's sandbox");
            }
        }
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "unusable sandbox_ref on a deleted session")
        }
    }
}

/// Revoke a sandbox token by `jti`, logging a failure. For a row already deleted, whose `jti` came
/// back from the delete.
pub async fn revoke_deleted(state: &AppState, session_id: &str, jti: &str) {
    if let Err(e) = revoke_jti(&state.db, jti).await {
        tracing::warn!(%session_id, error = %e, "could not revoke a deleted session's sandbox token");
    }
}

/// Revoke the sandbox token of every session of `agent_id` that has one, before the agent is
/// hard-deleted (which nulls `agent_id` on its sessions and hides them from every later pass).
/// The sandboxes themselves are not released here: the callers have no provider.
pub async fn revoke_tokens_of_agent(db: &sqlx::PgPool, agent_id: uuid::Uuid) {
    let sessions: Vec<String> = sqlx::query_scalar(
        "SELECT session_id FROM chat_sessions WHERE agent_id = $1 AND sandbox_token_jti IS NOT NULL",
    )
    .bind(agent_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    for session_id in sessions {
        if let Err(e) = revoke_sandbox_token(db, &session_id).await {
            tracing::warn!(%agent_id, %session_id, error = %e, "could not revoke a sandbox token");
        }
    }
}
