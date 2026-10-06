//! Per-agent gateway credentials (`MCP_GATEWAY_TOKEN`) — the agent-identity
//! half of the gateway's two-factor auth (docs/MCP_GATEWAY_AGENT_AUTH.md §2.2).
//!
//! Minted either at deploy time (injected into the container env) or, for a
//! CLI-bound coding-agent row that is never deployed, on demand by its owner
//! via `POST /api/agents/{id}/mcp-token` (`oss/server/src/agents/llm_config.rs`).
//! Either way the agent presents it as `Authorization: Bearer <token>` on
//! every `/api/mcp` call. Only the SHA-256 hex hash is stored
//! (`agent_gateway_tokens`, mirroring `oci_pull_credentials`); the plaintext
//! itself travels once — into the container env at deploy time, or in the
//! mint response body (and from there into a local MCP client's own config)
//! for the on-demand path — and is never persisted anywhere else. Every
//! deploy/restart re-mints and rotates a deployed agent's credential; `DELETE
//! /api/agents/{id}/mcp-token` is the on-demand row's equivalent kill switch.
//! Destroy tombstones it either way.

use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// How long a just-superseded credential keeps working after a rotation.
///
/// `mint` runs before the new workload is known to be live, so between the mint
/// and a healthy rollout the *old* container is still serving with the *old*
/// plaintext. Rejecting it immediately turns any slow or failed deploy into a
/// wave of 401s from an agent that is otherwise fine. One window covers the
/// rollout; past it, rotation is absolute again — an indefinite grace would
/// mean redeploy never actually revokes anything.
pub const ROTATION_GRACE_SECS: i64 = 900;

/// SHA-256 hex digest — the stored form of a gateway token.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Mint a fresh gateway token for `agent_id`, superseding any previous one —
/// on redeploy for a deployed agent (the old plaintext lived only in the
/// container env being replaced, so there is nothing worth keeping
/// long-term), or on an owner's on-demand `POST /api/agents/{id}/mcp-token`
/// for a CLI-bound row. Returns the plaintext exactly once: a deploy-time
/// caller injects it into the deployment env immediately; the on-demand
/// route instead returns it in the response body, for the caller to drop
/// into a local MCP client's config.
///
/// The superseded hash is retained as `prev_token_hash` and stays accepted for
/// [`ROTATION_GRACE_SECS`], because this runs *before* the new workload is known
/// to be live — see the constant. A hash that was already revoked is not carried
/// forward: re-minting for a previously destroyed agent must not resurrect the
/// credential that destroy tombstoned.
///
/// Generic over the executor (not concretely `&PgPool`) so a caller whose agent
/// row was inserted earlier in an as-yet-uncommitted transaction can pass
/// `&mut *tx` — the `agent_gateway_tokens_agent_id_fkey` insert must see that row,
/// which a separate pool connection can't until the transaction commits.
pub async fn mint(db: impl sqlx::PgExecutor<'_>, agent_id: Uuid) -> Result<String, sqlx::Error> {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    let token = format!("ngt_{}", hex::encode(buf));

    sqlx::query(
        "INSERT INTO agent_gateway_tokens (agent_id, token_hash)
         VALUES ($1, $2)
         ON CONFLICT (agent_id) DO UPDATE SET
             token_hash = EXCLUDED.token_hash,
             prev_token_hash = CASE
                 WHEN agent_gateway_tokens.revoked_at IS NULL
                 THEN agent_gateway_tokens.token_hash
             END,
             rotated_at = now(),
             created_at = now(),
             revoked_at = NULL",
    )
    .bind(agent_id)
    .bind(hash_token(&token))
    .execute(db)
    .await?;

    Ok(token)
}

/// Resolve a presented bearer token to its agent, if it matches a live
/// (non-revoked) credential belonging to a live (non-deleted) agent. `None` =
/// unknown, revoked, or the agent is gone → the caller must answer 401.
///
/// The `agents` join is what makes deletion fail closed. `revoke` runs on the
/// destroy path but is best-effort, so a transient failure there would
/// otherwise leave a destroyed agent's credential usable for the lifetime of
/// any flow it still appears in. Soft-delete is the authoritative signal;
/// `revoked_at` is the fast path, not the only one.
pub async fn authenticate(db: &PgPool, token: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT t.agent_id FROM agent_gateway_tokens t
         JOIN agents a ON a.id = t.agent_id
         WHERE t.revoked_at IS NULL
           AND a.deleted_at IS NULL
           AND (t.token_hash = $1
                OR (t.prev_token_hash = $1
                    AND t.rotated_at > now() - make_interval(secs => $2)))",
    )
    .bind(hash_token(token))
    .bind(ROTATION_GRACE_SECS as f64)
    .fetch_optional(db)
    .await
}

/// Tombstone an agent's gateway credential (agent destroy path). No-op when
/// none exists — destroy must be safe to re-run.
pub async fn revoke(db: &PgPool, agent_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_gateway_tokens
         SET revoked_at = now(), prev_token_hash = NULL
         WHERE agent_id = $1 AND revoked_at IS NULL",
    )
    .bind(agent_id)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hash_token;

    #[test]
    fn hash_is_stable_hex_sha256() {
        // Locks the storage convention: lowercase hex SHA-256 of the raw bytes.
        assert_eq!(
            hash_token("ngt_test"),
            "7e23252e185ed0461e4a4ec05c041e3c5904e12e5bede624b84ef5e9b141b1e7"
        );
    }
}
