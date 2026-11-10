//! The sandbox key a session runs under, and recording it on the row before the sandbox exists.
//!
//! `sandbox_ref` is only written once the provider has answered, so during a first acquire
//! nothing on the row says which sandbox is being created. [`set_sandbox_key`] records
//! `SandboxKey::name()` first, under the turn's lease, so whatever finds a session from its
//! sandbox (the orphan pass, metering) can match it even then.

use nasiko_runtime::SandboxKey;
use sqlx::PgPool;

use super::model::SessionRow;
use crate::coding::lifecycle::TurnLease;

/// The sandbox key a session runs under.
pub fn session_key(session: &SessionRow) -> SandboxKey {
    SandboxKey::Session {
        session_id: session.sandbox_uuid(),
    }
}

/// Record `key` on the session row on behalf of the turn holding `lease`. `Ok(false)` means the
/// lease is no longer the row's (a suspend claim, a reset or a delete got there first) and nothing
/// was written; the turn must stop before it acquires anything.
pub async fn set_sandbox_key(
    db: &PgPool,
    lease: &TurnLease,
    key: &SandboxKey,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE chat_sessions SET sandbox_key = $2 \
         WHERE session_id = $1 AND turn_fence = $3 AND turn_owner = $4 \
           AND runtime_state <> 'stopping' AND deleted_at IS NULL",
    )
    .bind(&lease.session_id)
    .bind(key.name())
    .bind(lease.fence)
    .bind(&lease.owner)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}
