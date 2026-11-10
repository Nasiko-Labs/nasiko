//! The sandbox token's bookkeeping: the token a session's sandbox is created with, kept
//! encrypted at rest so every acquire can rebuild the sandbox's full env from it, and revoking it.
//!
//! The token is sealed with the platform master key (`nasiko_secrets`, AES-GCM, as agent secrets
//! are) under its own key scope, per agent and bound to the session id, so a ciphertext copied to another
//! session's row does not open. Its `jti` is kept beside it so revocation needs no decryption.
//! Free functions over the pool rather than methods on `SessionStore`: the store's pool is
//! private to `model.rs`, and this table access is the token's, not the store's.
//!
//! The token is a 30-day bearer in the sandbox's env, readable by any process inside it. It is
//! revoked only when:
//! - the reaper finds the sandbox gone and clears the row ([`revoke_sandbox_token`]);
//! - the session is deleted (`chat/routes.rs`, by the `jti` the delete returns);
//! - its user is deleted, before the cascade (`sessions::teardown`);
//! - its agent is hard-deleted (`sessions::teardown::revoke_tokens_of_agent`; the sandbox itself
//!   is not released there);
//! - the turn replaces it (missing, unverifiable, revoked or near expiry), by
//!   [`revoke_sandbox_token_for_turn`], which only acts for the turn that still holds the lease
//!   and still sees the token it loaded.
//!
//! Never because an acquire failed: a container or pod that survived the failure still carries it.

use chrono::{Duration, Utc};
use nasiko_secrets::SecretsCrypto;
use sqlx::PgPool;
use uuid::Uuid;

use super::identity::SANDBOX_TOKEN_TTL_SECS;
use super::keys::session_key;
use super::model::SessionRow;
use crate::coding::lifecycle::{self, TurnLease};
use crate::coding::revocation::revoke_session_token;
use crate::coding::turn::CodingTurnError;
use crate::state::AppState;

/// A token with less than this left is replaced (the sandbox is released first so the new env
/// lands), so a sandbox never runs on a token about to lapse.
pub const RENEW_WITHIN_SECS: u64 = 24 * 3600;
const _: () = assert!(RENEW_WITHIN_SECS < SANDBOX_TOKEN_TTL_SECS);

/// Why the stored token could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("sandbox token cannot be sealed or opened: {0}")]
    Crypto(#[from] nasiko_secrets::SecretsError),
}

fn crypto(agent_id: Uuid) -> Result<SecretsCrypto, nasiko_secrets::SecretsError> {
    SecretsCrypto::try_for_sandbox_token(agent_id)
}

/// Seal and store the session's sandbox token with its `jti`, replacing any earlier one, on behalf
/// of the turn holding `lease`. `Ok(false)` means the lease is no longer the row's (a suspend
/// claim, a reset or a delete got there first) and nothing was written; the caller revokes the
/// token it minted and stops.
pub async fn store_sandbox_token(
    db: &PgPool,
    lease: &TurnLease,
    agent_id: Uuid,
    token: &str,
    jti: &str,
) -> Result<bool, TokenStoreError> {
    let sealed = crypto(agent_id)?.encrypt_with_aad(token, lease.session_id.as_bytes());
    let done = sqlx::query(
        "UPDATE chat_sessions SET sandbox_token_jti = $2, sandbox_token_enc = $3 \
         WHERE session_id = $1 AND turn_fence = $4 AND turn_owner = $5 \
           AND runtime_state <> 'stopping' AND deleted_at IS NULL",
    )
    .bind(&lease.session_id)
    .bind(jti)
    .bind(sealed.into_bytes())
    .bind(lease.fence)
    .bind(&lease.owner)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Whether `jti` has been revoked.
pub async fn is_revoked(db: &PgPool, jti: &str) -> sqlx::Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM revoked_session_tokens WHERE jti = $1")
            .bind(jti)
            .fetch_optional(db)
            .await?;
    Ok(found.is_some())
}

/// The session's sandbox token, opened; `None` when none was minted or it was revoked.
pub async fn load_sandbox_token(
    db: &PgPool,
    session_id: &str,
    agent_id: Uuid,
) -> Result<Option<String>, TokenStoreError> {
    let sealed: Option<Option<Vec<u8>>> =
        sqlx::query_scalar("SELECT sandbox_token_enc FROM chat_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_optional(db)
            .await?;
    let Some(sealed) = sealed.flatten() else {
        return Ok(None);
    };
    let sealed = String::from_utf8_lossy(&sealed);
    Ok(Some(
        crypto(agent_id)?.decrypt_with_aad(&sealed, session_id.as_bytes())?,
    ))
}

/// Revoke the session's sandbox token (the router and MCP gateway then refuse it) and forget it,
/// in one transaction so a crash cannot leave a revoked token still stored for reuse. A no-op
/// when there is none. The revocation row is kept for the token's full lifetime, since it was not
/// recorded with its own expiry.
pub async fn revoke_sandbox_token(db: &PgPool, session_id: &str) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let jti: Option<Option<String>> = sqlx::query_scalar(
        "SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1 FOR UPDATE",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(jti) = jti.flatten() else {
        return Ok(());
    };
    let expires_at = Utc::now() + Duration::seconds(SANDBOX_TOKEN_TTL_SECS as i64);
    sqlx::query(
        "INSERT INTO revoked_session_tokens (jti, expires_at) VALUES ($1, $2) \
         ON CONFLICT (jti) DO NOTHING",
    )
    .bind(&jti)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_token_jti = NULL, sandbox_token_enc = NULL \
         WHERE session_id = $1",
    )
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Revoke a sandbox token by `jti` alone, for callers that already hold it (a deleted row).
pub async fn revoke_jti(db: &PgPool, jti: &str) -> sqlx::Result<()> {
    let expires_at = Utc::now() + Duration::seconds(SANDBOX_TOKEN_TTL_SECS as i64);
    revoke_session_token(db, jti, expires_at).await
}

/// The `jti` of the session's stored sandbox token, if any.
pub async fn stored_jti(db: &PgPool, session_id: &str) -> sqlx::Result<Option<String>> {
    let jti: Option<Option<String>> =
        sqlx::query_scalar("SELECT sandbox_token_jti FROM chat_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_optional(db)
            .await?;
    Ok(jti.flatten())
}

/// Revoke the token a turn loaded and forget it, for the turn that holds `lease`, in one
/// transaction. Acts only while the row is still the lease's (fence and owner) and still stores
/// `expected_jti`; otherwise nothing is revoked and `Ok(false)` says so. A stale turn therefore
/// cannot revoke the token its successor minted.
pub async fn revoke_sandbox_token_for_turn(
    db: &PgPool,
    lease: &TurnLease,
    expected_jti: &str,
) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let current: Option<Option<String>> = sqlx::query_scalar(
        "SELECT sandbox_token_jti FROM chat_sessions \
         WHERE session_id = $1 AND turn_fence = $2 AND turn_owner = $3 FOR UPDATE",
    )
    .bind(&lease.session_id)
    .bind(lease.fence)
    .bind(&lease.owner)
    .fetch_optional(&mut *tx)
    .await?;
    if current.flatten().as_deref() != Some(expected_jti) {
        return Ok(false);
    }
    let expires_at = Utc::now() + Duration::seconds(SANDBOX_TOKEN_TTL_SECS as i64);
    sqlx::query(
        "INSERT INTO revoked_session_tokens (jti, expires_at) VALUES ($1, $2) \
         ON CONFLICT (jti) DO NOTHING",
    )
    .bind(expected_jti)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE chat_sessions SET sandbox_token_jti = NULL, sandbox_token_enc = NULL \
         WHERE session_id = $1 AND turn_fence = $2 AND turn_owner = $3",
    )
    .bind(&lease.session_id)
    .bind(lease.fence)
    .bind(&lease.owner)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether a token expiring at `exp` (seconds since the epoch) still has enough life at `now`.
pub fn has_life(exp: u64, now: u64) -> bool {
    exp > now.saturating_add(RENEW_WITHIN_SECS)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Why a stored sandbox token cannot be used as it is.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenUse {
    Usable,
    Unverifiable(String),
    NearExpiry,
    Revoked,
}

/// What a turn needs to get the session's sandbox token.
pub struct TokenNeeds<'a> {
    pub state: &'a AppState,
    pub lease: &'a TurnLease,
    pub session: &'a SessionRow,
    pub agent_id: Uuid,
    pub user_id: Uuid,
    pub flow_id: &'a str,
}

fn setup(what: &str, error: impl std::fmt::Display) -> CodingTurnError {
    CodingTurnError::Setup(format!("{what}: {error}"))
}

/// The session's sandbox token: the stored one when it verifies, is not revoked and has life,
/// otherwise a new one. Replacing a token that a sandbox may be carrying releases that sandbox
/// first (the provider applies the env it is given when it creates the sandbox, so the new token
/// lands with the new sandbox) and revokes the old token. Why a token was replaced is logged.
///
/// Releasing and revoking are destructive and happen on this turn's say-so, so when there is
/// anything to destroy the lease is confirmed first, and the revoke acts only on the token this
/// turn loaded.
pub async fn ensure_sandbox_token(needs: &TokenNeeds<'_>) -> Result<String, CodingTurnError> {
    use nasiko_secrets::SecretsError;
    let TokenNeeds {
        state,
        lease,
        session,
        agent_id,
        ..
    } = *needs;
    let session_id = session.session_id.as_str();
    let replace_because = match load_sandbox_token(&state.db, session_id, agent_id).await {
        Ok(Some(token)) => match token_use(state, &token).await? {
            TokenUse::Usable => return Ok(token),
            other => format!("{other:?}"),
        },
        Ok(None) => "none stored".to_owned(),
        // A missing or malformed master key is a deployment fault: releasing the sandbox would
        // not fix it and would only throw away a working one.
        Err(TokenStoreError::Crypto(
            e @ (SecretsError::MissingKey | SecretsError::InvalidKeyLength),
        )) => return Err(setup("sandbox credential", e)),
        Err(TokenStoreError::Crypto(e)) => format!("unreadable: {e}"),
        Err(e) => return Err(setup("sandbox credential", e)),
    };
    tracing::info!(%session_id, reason = %replace_because, "replacing the session's sandbox token");
    let old_jti = stored_jti(&state.db, session_id).await?;
    if session.sandbox_ref.is_some() || old_jti.is_some() {
        confirm_lease(state, lease).await?;
        release_sandbox(state, session).await?;
    }
    if let Some(old) = old_jti
        && !revoke_sandbox_token_for_turn(&state.db, lease, &old).await?
    {
        return Err(CodingTurnError::Superseded);
    }
    let minted = crate::sessions::identity::mint_sandbox_token(
        &state.config,
        agent_id,
        needs.user_id,
        session_id,
        needs.flow_id,
    )
    .map_err(|e| setup("sandbox credential", e))?;
    let stored = store_sandbox_token(&state.db, lease, agent_id, &minted.token, &minted.jti)
        .await
        .map_err(|e| setup("sandbox credential", e))?;
    if !stored {
        // The row is no longer this turn's: the token was never stored, so nothing can carry it.
        let _ = revoke_jti(&state.db, &minted.jti).await;
        return Err(CodingTurnError::Superseded);
    }
    Ok(minted.token)
}

/// The lease is renewed (so still ours), or the turn is superseded. An unreachable database does
/// not stop the turn here; the fenced writes that follow are the real check.
async fn confirm_lease(state: &AppState, lease: &TurnLease) -> Result<(), CodingTurnError> {
    let budget =
        std::time::Duration::from_secs(state.config.coding_agent_lease_heartbeat_secs.max(1));
    match lifecycle::heartbeat_retrying(
        &state.db,
        lease,
        state.config.coding_agent_turn_lease_secs,
        budget,
    )
    .await
    {
        lifecycle::Beat::Lost => Err(CodingTurnError::Superseded),
        lifecycle::Beat::Renewed | lifecycle::Beat::Unreachable => Ok(()),
    }
}

/// Whether a stored token verifies against the current secret, is not revoked and has life
/// left. A rotated `AGENT_JWT_SECRET` fails verification, which is what rebuilds every sandbox
/// after a rotation. A database error is an error, not "unusable": it must not release a good
/// sandbox.
pub async fn token_use(state: &AppState, token: &str) -> Result<TokenUse, CodingTurnError> {
    let algorithm = nasiko_session_token::parse_algorithm(&state.config.agent_jwt_algorithm);
    let claims = match nasiko_session_token::verify_session_token(
        token,
        &state.config.agent_jwt_secret,
        algorithm,
    ) {
        Ok(claims) => claims,
        Err(e) => return Ok(TokenUse::Unverifiable(e.to_string())),
    };
    if is_revoked(&state.db, &claims.jti).await? {
        return Ok(TokenUse::Revoked);
    }
    Ok(if has_life(claims.exp, unix_now()) {
        TokenUse::Usable
    } else {
        TokenUse::NearExpiry
    })
}

/// Release the session's sandbox if it has one, so the next acquire builds it fresh. A row with
/// no recorded `sandbox_ref` (a first acquire that never got to write it) is found by key among
/// the provider's sandboxes.
pub async fn release_sandbox(
    state: &AppState,
    session: &SessionRow,
) -> Result<(), CodingTurnError> {
    let key = session_key(session);
    if let Some(sandbox_ref) = session.sandbox_ref.as_deref() {
        let handle = nasiko_runtime::SandboxHandle::parse(sandbox_ref, key)?;
        state.sandbox.release(&handle).await?;
        return Ok(());
    }
    for info in state.sandbox.list().await? {
        if info.handle.key == key {
            state.sandbox.release(&info.handle).await?;
        }
    }
    Ok(())
}
