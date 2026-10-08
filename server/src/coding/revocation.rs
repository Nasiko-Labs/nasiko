//! Revocation of per-turn coding-agent session tokens.
//!
//! A session token (NAS-839) is a short-lived signed credential verified
//! offline, so to kill one before its `exp` the control plane records its `jti`
//! in `revoked_session_tokens`; the MCP gateway and LLM router reject any token
//! whose `jti` is present. This module is the **producer** side of that table
//! (the readers live in `mcp::handlers::gateway` and the LLM router):
//!
//!   - [`revoke_session_token`] records one `jti`. The turn path calls it when a
//!     turn ends, so the per-turn credential dies with the turn instead of
//!     lingering its full TTL inside the container — a copy that escaped the
//!     agent process (logged, cached, captured by a tool) can't be replayed.
//!   - [`sweep_expired`] / [`sweep_loop`] purge rows past their `expires_at`, so
//!     the table only ever holds tokens that could still verify.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// A minted token's revocation coordinates: the `jti` to record and the `exp` it
/// was signed with (so a revocation row can be swept once the token lapses).
#[derive(Debug, Clone)]
pub struct RevokedTokenId {
    pub jti: String,
    pub expires_at: DateTime<Utc>,
}

/// How often [`sweep_loop`] purges revocation rows that can no longer verify.
/// Session tokens are short-lived (an hour by default), so an hourly sweep keeps
/// the table trimmed without churn.
const SWEEP_INTERVAL: Duration = Duration::from_secs(3600);

/// Record a session token's `jti` as revoked.
///
/// Idempotent: revoking the same token twice is a no-op (the turn-end and the
/// start-failure paths can both fire for one token). `expires_at` is the token's
/// own `exp`, kept only so [`sweep_expired`] can drop the row once the token can
/// no longer verify regardless.
pub async fn revoke_session_token(
    db: &PgPool,
    jti: &str,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO revoked_session_tokens (jti, expires_at) VALUES ($1, $2) \
         ON CONFLICT (jti) DO NOTHING",
    )
    .bind(jti)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Delete revocation rows whose token has already expired (it can no longer
/// verify, so the row is dead weight). Returns how many were purged.
pub async fn sweep_expired(db: &PgPool) -> sqlx::Result<u64> {
    let result = sqlx::query("DELETE FROM revoked_session_tokens WHERE expires_at < now()")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

/// Spawned once at server startup (`main.rs`), same shape as the coding-agent
/// reconciler and every other periodic sweep in this codebase.
pub async fn sweep_loop(db: PgPool) {
    loop {
        tokio::time::sleep(SWEEP_INTERVAL).await;
        match sweep_expired(&db).await {
            Ok(n) if n > 0 => {
                tracing::info!(purged = n, "session-token revocation: swept expired rows")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "session-token revocation sweep failed"),
        }
    }
}
