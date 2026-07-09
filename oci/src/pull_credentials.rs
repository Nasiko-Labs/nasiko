//! Per-agent credentials for pulling that agent's image out of this
//! registry from a real Kubernetes node — see
//! `oss/migrations/018_oci_pull_credentials.sql`.
//!
//! The registry's normal auth is bearer-JWT (the host's session token),
//! which doesn't fit the `kubernetes.io/dockerconfigjson` shape kubelet/
//! containerd need for `imagePullSecrets`. This module mints/verifies a
//! separate, per-agent HTTP Basic-auth credential instead, scoped to
//! exactly one agent's repository — see [`PullOnlyIdentity`] in `authz.rs`
//! for how a verified credential flows into request handling.

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{OciError, Result};

/// A freshly-minted credential's plaintext — only ever returned once, by
/// [`get_or_create`], at the moment its hash is first stored. Nothing in
/// this module (or its callers) retains the plaintext afterward; only the
/// K8s Secret it gets seeded into remains a durable copy.
pub struct PlaintextCredential {
    pub username: String,
    pub token: String,
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Mints a pull credential for `agent_id` if it doesn't already have a live
/// one. Returns `Some` only on first mint (or after a prior credential was
/// revoked) — callers must seed the plaintext into a K8s Secret immediately,
/// since it is never retrievable again. Returns `None` when a live
/// credential already exists — nothing new needs seeding, the Secret from
/// the original mint is still valid.
pub async fn get_or_create(pool: &PgPool, agent_id: Uuid) -> Result<Option<PlaintextCredential>> {
    let has_live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM oci_pull_credentials WHERE agent_id = $1 AND revoked_at IS NULL)",
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await
    .map_err(OciError::Database)?;
    if has_live {
        return Ok(None);
    }

    let username = agent_id.to_string();
    let token = nasiko_auth::generate_access_secret();
    let token_hash = hash_token(&token);

    sqlx::query(
        "INSERT INTO oci_pull_credentials (agent_id, username, token_hash)
         VALUES ($1, $2, $3)
         ON CONFLICT (agent_id) DO UPDATE SET
             username = EXCLUDED.username,
             token_hash = EXCLUDED.token_hash,
             revoked_at = NULL,
             created_at = now()",
    )
    .bind(agent_id)
    .bind(&username)
    .bind(&token_hash)
    .execute(pool)
    .await
    .map_err(OciError::Database)?;

    Ok(Some(PlaintextCredential { username, token }))
}

/// Verifies an `Authorization: Basic` (username, password) pair against a
/// live credential, returning the bound `agent_id` on success. `username`
/// is checked alongside the token hash (not just the hash alone) so a
/// revoked-then-reissued credential for a different agent can never collide.
pub async fn verify(pool: &PgPool, username: &str, password: &str) -> Result<Option<Uuid>> {
    let token_hash = hash_token(password);
    let agent_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT agent_id FROM oci_pull_credentials
         WHERE username = $1 AND token_hash = $2 AND revoked_at IS NULL",
    )
    .bind(username)
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(OciError::Database)?;
    Ok(agent_id)
}

/// Revokes an agent's pull credential (agent destroy path). No-op if none
/// exists — destroy must be safe to re-run.
pub async fn revoke(pool: &PgPool, agent_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE oci_pull_credentials SET revoked_at = now() WHERE agent_id = $1 AND revoked_at IS NULL")
        .bind(agent_id)
        .execute(pool)
        .await
        .map_err(OciError::Database)?;
    Ok(())
}
