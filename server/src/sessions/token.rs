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
//!   and still sees the token it loaded. The sandbox is stopped first ([`stop_for_new_env`]) so
//!   the new token reaches it; its files stay.
//!
//! Never because an acquire failed: a container or pod that survived the failure still carries it.

use chrono::{Duration, Utc};
use nasiko_runtime::{SandboxHandle, SandboxProvider};
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

/// A token with less than this left is replaced (the sandbox is stopped first so the new env
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
/// otherwise a new one. Replacing a token that a sandbox may be carrying stops that sandbox first
/// ([`stop_for_new_env`], which keeps its files) so the next acquire brings it up on the new
/// token, and revokes the old token. Why a token was replaced is logged.
///
/// Stopping and revoking are destructive and happen on this turn's say-so, so when there is
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
        // A missing or malformed master key is a deployment fault: replacing the token would
        // not fix it and would only stop a working sandbox.
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
        stop_for_new_env(state.sandbox.as_ref(), session).await?;
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
    .map_err(|e| CodingTurnError::from_credential("sandbox credential", e))?;
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
/// left. A rotated `AGENT_JWT_SECRET` fails verification, which is what puts every sandbox on a
/// new token after a rotation. A database error is an error, not "unusable": it must not stop a
/// good sandbox.
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

/// Stop the session's sandbox, if it has one, so the next acquire brings it up on a new env, and
/// keep its files. Where a resumed sandbox runs on the resuming env it is only suspended: the
/// bare pod recreates its pod from the resuming spec, and in a harness agent (which never reads
/// the spec's env) the restarted shim starts unconfigured and the turn's handshake configures it
/// with the new env. A release there can take the files with it (a harness agent purges the
/// session's directory). Elsewhere (Docker, whose resumed container keeps the env
/// it was created with) it is released, the only way its env changes; the persistent workspace,
/// which holds the harness's home too, survives that.
///
/// On the bare pod a suspend differs from the release it replaces: the pod is deleted with its
/// full termination grace (up to 30 s, against 5 s on release), and the resumed pod keeps the
/// image, command and resources its marker recorded at creation, so a changed harness image is
/// not picked up here. Both are acceptable for a renewal that comes about once a month.
///
/// A row with no recorded `sandbox_ref` (a first acquire that never got to write it) is found by
/// key among the provider's sandboxes.
pub async fn stop_for_new_env(
    provider: &dyn SandboxProvider,
    session: &SessionRow,
) -> Result<(), CodingTurnError> {
    for handle in session_sandboxes(provider, session).await? {
        if provider.resume_takes_new_env(&handle).await? {
            provider.suspend(&handle).await?;
        } else {
            provider.release(&handle).await?;
        }
    }
    Ok(())
}

/// The session's sandbox: the one its row records, else every one the provider lists under its
/// key.
async fn session_sandboxes(
    provider: &dyn SandboxProvider,
    session: &SessionRow,
) -> Result<Vec<SandboxHandle>, CodingTurnError> {
    let key = session_key(session);
    if let Some(sandbox_ref) = session.sandbox_ref.as_deref() {
        return Ok(vec![SandboxHandle::parse(sandbox_ref, key)?]);
    }
    Ok(provider
        .list()
        .await?
        .into_iter()
        .map(|info| info.handle)
        .filter(|handle| handle.key == key)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::identity::session_env;
    use nasiko_runtime::sandbox_fake::FakeSandboxProvider;
    use nasiko_runtime::{SandboxKey, SandboxSpec, SandboxState, TransportKind};

    const TOKEN_KEY: &str = "NASIKO_SESSION_TOKEN";

    fn session() -> SessionRow {
        SessionRow {
            session_id: Uuid::new_v4().to_string(),
            user_id: Uuid::nil(),
            agent_id: Some(Uuid::new_v4()),
            acp_session_id: None,
            runtime_handle: None,
            runtime_state: "running".into(),
            volume_ref: None,
            turn_owner: None,
            lease_expires_at: None,
            turn_fence: 1,
            last_turn_at: None,
            active_turn_id: None,
            sandbox_ref: None,
            suspended_at: None,
            workspace_repo: None,
            pending_permission: None,
            created_at: Utc::now(),
        }
    }

    /// The session's sandbox spec, its env carrying `token` as every acquire builds it.
    fn spec(key: &SandboxKey, token: &str) -> SandboxSpec {
        SandboxSpec {
            key: key.clone(),
            image: "harness:1".into(),
            command: vec![],
            env: session_env(token, "", ""),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![7000],
            transport: TransportKind::WebSocket,
            image_pull_secret: None,
        }
    }

    /// A session whose sandbox is up on `old-token`, with its ref recorded.
    async fn running_on_old_token(fake: &FakeSandboxProvider) -> (SessionRow, SandboxHandle) {
        let row = session();
        let handle = fake
            .acquire(&spec(&session_key(&row), "old-token"))
            .await
            .unwrap();
        let row = SessionRow {
            sandbox_ref: Some(handle.sandbox_ref()),
            ..row
        };
        (row, handle)
    }

    fn token_in(fake: &FakeSandboxProvider, key: &SandboxKey) -> String {
        fake.spec_of(key).expect("sandbox exists").env[TOKEN_KEY].clone()
    }

    #[tokio::test]
    async fn a_new_token_suspends_a_sandbox_that_resumes_on_the_new_env_so_its_files_stay() {
        let fake = FakeSandboxProvider::default();
        fake.set_resume_takes_new_env(true);
        let (row, handle) = running_on_old_token(&fake).await;

        stop_for_new_env(&fake, &row).await.unwrap();
        assert_eq!(
            fake.status(&handle).await.unwrap(),
            SandboxState::Suspended,
            "suspended, not released: the sandbox and its files are kept"
        );

        let key = session_key(&row);
        fake.acquire(&spec(&key, "new-token")).await.unwrap();
        assert_eq!(token_in(&fake, &key), "new-token");
        assert_eq!(fake.creations(&key), 1, "resumed, not rebuilt");
    }

    #[tokio::test]
    async fn a_new_token_releases_a_sandbox_whose_env_is_fixed_and_the_rebuilt_one_carries_it() {
        let fake = FakeSandboxProvider::default();
        let (row, handle) = running_on_old_token(&fake).await;

        stop_for_new_env(&fake, &row).await.unwrap();
        assert_eq!(fake.status(&handle).await.unwrap(), SandboxState::Absent);

        let key = session_key(&row);
        fake.acquire(&spec(&key, "new-token")).await.unwrap();
        assert_eq!(token_in(&fake, &key), "new-token");
        assert_eq!(fake.creations(&key), 2, "rebuilt");
    }

    #[tokio::test]
    async fn a_row_without_a_recorded_ref_stops_only_the_sandbox_under_its_key() {
        let fake = FakeSandboxProvider::default();
        fake.set_resume_takes_new_env(true);
        let (_, other) = running_on_old_token(&fake).await;
        let row = session();
        let mine = fake
            .acquire(&spec(&session_key(&row), "old-token"))
            .await
            .unwrap();

        stop_for_new_env(&fake, &row).await.unwrap();
        assert_eq!(fake.status(&mine).await.unwrap(), SandboxState::Suspended);
        assert_eq!(fake.status(&other).await.unwrap(), SandboxState::Running);
    }
}
