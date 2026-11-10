//! The pending-approval pointer on a session row (`chat_sessions.pending_permission`): written
//! when a turn blocks on a human, cleared when the wait ends.
//!
//! The write is fenced on the turn's lease (fence and owner) and refuses a `stopping` row, so a
//! turn a suspend claim has superseded cannot touch the pointer. The clear is compare-and-clear
//! on the request's `hitl_id` and the fence, and deliberately not the owner: a wait that is
//! abandoned when the turn ends clears the pointer after `release_turn` has already cleared the
//! owner, and must still land. The `hitl_id` already ties the clear to its own wait, so a wait
//! that ends late cannot wipe the pointer of a newer one. Free functions over the pool for the same reason as
//! [`super::token`]: the store's pool is private to `model.rs`.

use sqlx::PgPool;
use uuid::Uuid;

use crate::coding::lifecycle::TurnLease;

/// Point the session at the approval it is now waiting on.
pub async fn set_pending(
    db: &PgPool,
    lease: &TurnLease,
    hitl_id: Uuid,
    tool_call_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions SET pending_permission = $2 \
         WHERE session_id = $1 AND deleted_at IS NULL \
           AND turn_fence = $3 AND turn_owner = $4 AND runtime_state <> 'stopping'",
    )
    .bind(&lease.session_id)
    .bind(serde_json::json!({ "hitl_id": hitl_id, "tool_call_id": tool_call_id }))
    .bind(lease.fence)
    .bind(&lease.owner)
    .execute(db)
    .await
    .map(|_| ())
}

/// Clear the pointer, but only if it still names `hitl_id` and the row is still on the turn's
/// fence (see the module note on why not the owner).
pub async fn clear_pending(db: &PgPool, lease: &TurnLease, hitl_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE chat_sessions SET pending_permission = NULL \
         WHERE session_id = $1 AND pending_permission->>'hitl_id' = $2 \
           AND turn_fence = $3 AND runtime_state <> 'stopping'",
    )
    .bind(&lease.session_id)
    .bind(hitl_id.to_string())
    .bind(lease.fence)
    .execute(db)
    .await
    .map(|_| ())
}
