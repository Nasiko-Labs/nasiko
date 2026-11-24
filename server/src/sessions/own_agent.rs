//! Each user's own harness agent (`nasiko_runtime::harness_agent`): the agent every session a user
//! opens on a harness is bound to, whatever the sandbox provider.
//!
//! The harness catalog (`GET /api/harnesses`) is what a user picks from; opening a session
//! (`POST /api/sessions`) creates their agent for that harness if they have none, and every
//! message of the session is sent to it. Because that agent is the user's own row, everything
//! keyed on a session's agent follows the user: the model router resolves the user's settings for
//! it (a config attached to it, else the user's default, on the user's keys), the MCP gateway
//! applies the user's connectors and this agent's own tool rules, whether the harness asks before
//! acting is this agent's `hitl_enabled` (the user's choice, off until they turn it on), and the
//! session's sandbox time is metered as the user's.

use nasiko_runtime::harness_agent;
use sqlx::PgPool;
use uuid::Uuid;

use super::catalog::{Harness, qualified_image};

/// Why the user's harness agent could not be had.
#[derive(Debug, thiserror::Error)]
pub enum OwnAgentError {
    /// Another agent of the user's already holds the name the harness agent takes.
    #[error("you already have an agent named {0}; rename it to open a session of this harness")]
    NameTaken(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The user's harness agent for `harness_id`, if they have one.
pub async fn find(db: &PgPool, user_id: Uuid, harness_id: &str) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "SELECT id FROM agents \
         WHERE owner_id = $1 AND name = $2 AND workload_kind = 'harness' AND harness_id = $3 \
           AND deleted_at IS NULL",
    )
    .bind(user_id)
    .bind(harness_agent::name(harness_id))
    .bind(harness_id)
    .fetch_optional(db)
    .await
}

/// Whether the user's next session of `harness_id` creates their harness agent: no live agent of
/// theirs holds its name. An edition that counts agents against limits checks those before such a
/// session opens.
pub async fn would_create(db: &PgPool, user_id: Uuid, harness_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM agents \
                            WHERE owner_id = $1 AND name = $2 AND deleted_at IS NULL)",
    )
    .bind(user_id)
    .bind(harness_agent::name(harness_id))
    .fetch_one(db)
    .await
}

/// The user's harness agent for `harness`, created if they have none: parked (it runs no
/// workload until a provider that runs sessions inside it starts one), never public, parkable and
/// writable, with approvals off (`hitl_enabled`, the column's default) until the user turns them
/// on. Ask the session admission first: creating it may be what the admission refuses.
pub async fn ensure(
    db: &PgPool,
    user_id: Uuid,
    harness: &Harness,
    image_prefix: &str,
) -> Result<Uuid, OwnAgentError> {
    let name = harness_agent::name(&harness.id);
    let created: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO agents (name, display_name, description, owner_id, image, status, is_public, \
                             workload_kind, harness_id, parking_policy, writable) \
         VALUES ($1, $2, $3, $4, $5, 'parked', false, 'harness', $6, 'auto', true) \
         ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO NOTHING \
         RETURNING id",
    )
    .bind(&name)
    .bind(harness_agent::display_name(&harness.name))
    .bind(harness_agent::description(&harness.name))
    .bind(user_id)
    .bind(qualified_image(&harness.image, image_prefix))
    .bind(&harness.id)
    .fetch_optional(db)
    .await?;
    if let Some(id) = created {
        tracing::info!(agent_id = %id, %user_id, harness = %harness.id, "created the user's harness agent");
        return Ok(id);
    }
    // A racing request created it, or an agent of another kind holds the name.
    find(db, user_id, &harness.id)
        .await?
        .ok_or(OwnAgentError::NameTaken(name))
}
