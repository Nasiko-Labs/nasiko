//! The platform credentials an agent's sandbox carries, minted once and kept.
//!
//! The gateway and state credentials are single-live-token: minting one supersedes the last
//! (valid for a short grace only). A sandbox's environment is fixed at creation and a resume
//! keeps it, so minting on every wake would leave a running sandbox with tokens the platform has
//! since rotated away. The values are minted on the first wake, stored encrypted
//! (`agent_sandbox_credentials`, migration 0082) and returned unchanged on every later one, until
//! [`clear_agent_sandbox_credentials`] (an update of the agent, or its sandbox released).
//!
//! What is stored is the platform's own env delta (gateway token, state URL and token, LLM router
//! wiring), never the agent's secrets: those are rebuilt fresh on every acquire.
//!
//! `parking::release_agent_sandbox` releases the `a-{id}` sandbox and clears these on update,
//! rollback, restart and park; `stop` only suspends, and keeps them.

use std::collections::HashMap;

use nasiko_secrets::SecretsCrypto;
use uuid::Uuid;

use crate::agents::deploy_env;
use crate::state::AppState;

/// Associated data binding a stored blob to its agent and purpose.
fn aad(agent_id: Uuid) -> Vec<u8> {
    let mut aad = b"agent-sandbox-credentials-v1:".to_vec();
    aad.extend_from_slice(agent_id.as_bytes());
    aad
}

/// The platform env this agent's sandbox carries and whether this call minted it.
pub struct SandboxCredentials {
    pub env: HashMap<String, String>,
    /// True when none was stored, so this is the first wake since the agent was deployed,
    /// updated or released.
    pub minted: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("credential encryption: {0}")]
    Crypto(String),
}

/// The agent's stored credential env, minted and stored first if there is none. Concurrent first
/// wakes (this server or another replica) are serialised by a transaction-scoped advisory lock,
/// and everything runs on that one transaction's connection, so only one mints and a pool of one
/// connection is enough.
pub async fn load_or_mint(
    state: &AppState,
    agent_id: Uuid,
    owner_id: Uuid,
) -> Result<SandboxCredentials, CredentialError> {
    if let Some(env) = load(&state.db, agent_id).await? {
        return Ok(SandboxCredentials { env, minted: false });
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("agent-sandbox-credentials:{agent_id}"))
        .execute(&mut *tx)
        .await?;
    // Another waker may have minted while this one waited for the lock.
    if let Some(env) = load(&mut *tx, agent_id).await? {
        return Ok(SandboxCredentials { env, minted: false });
    }
    let env = deploy_env::platform_env(state, &mut tx, agent_id, owner_id).await;
    store(&mut *tx, agent_id, &env).await?;
    tx.commit().await?;
    Ok(SandboxCredentials { env, minted: true })
}

/// The stored env. A blob that no longer decrypts (the master key rotated) reads as absent, so
/// the next wake mints afresh instead of failing for good.
async fn load(
    db: impl sqlx::PgExecutor<'_>,
    agent_id: Uuid,
) -> Result<Option<HashMap<String, String>>, CredentialError> {
    let stored: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT env_enc FROM agent_sandbox_credentials WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_optional(db)
            .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let crypto = SecretsCrypto::try_for_sandbox_token(agent_id)
        .map_err(|e| CredentialError::Crypto(e.to_string()))?;
    let decoded = String::from_utf8(stored)
        .map_err(|e| e.to_string())
        .and_then(|enc| {
            crypto
                .decrypt_with_aad(&enc, &aad(agent_id))
                .map_err(|e| e.to_string())
        })
        .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()));
    match decoded {
        Ok(env) => Ok(Some(env)),
        Err(reason) => {
            tracing::warn!(%agent_id, %reason, "stored sandbox credentials unreadable; minting afresh");
            Ok(None)
        }
    }
}

async fn store(
    db: impl sqlx::PgExecutor<'_>,
    agent_id: Uuid,
    env: &HashMap<String, String>,
) -> Result<(), CredentialError> {
    let crypto = SecretsCrypto::try_for_sandbox_token(agent_id)
        .map_err(|e| CredentialError::Crypto(e.to_string()))?;
    let json = serde_json::to_string(env).map_err(|e| CredentialError::Crypto(e.to_string()))?;
    let enc = crypto.encrypt_with_aad(&json, &aad(agent_id));
    sqlx::query(
        "INSERT INTO agent_sandbox_credentials (agent_id, env_enc) VALUES ($1, $2) \
         ON CONFLICT (agent_id) DO UPDATE SET env_enc = EXCLUDED.env_enc, minted_at = now()",
    )
    .bind(agent_id)
    .bind(enc.into_bytes())
    .execute(db)
    .await?;
    Ok(())
}

/// Forget the stored credentials, so the agent's next wake mints new ones. Call it when the
/// agent is updated and when its sandbox is released: the old sandbox, and the env it was
/// created with, are gone either way.
pub async fn clear_agent_sandbox_credentials(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_sandbox_credentials WHERE agent_id = $1")
        .bind(agent_id)
        .execute(db)
        .await?;
    Ok(())
}
