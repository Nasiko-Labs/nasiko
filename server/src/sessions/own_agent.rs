//! Each user's own harness agent (`nasiko_runtime::harness_agent`): the agent every session a user
//! opens on a harness is bound to, whatever the sandbox provider.
//!
//! A platform harness agent (the one boot seeding names after its harness) is only the door every
//! user knocks on: a user's message to it runs as their own agent, which their first session
//! creates. Because that agent is the user's own row, everything keyed on a session's agent
//! follows the user: the model router resolves the user's settings for it (a config attached to
//! it, else the user's default, on the user's keys), the MCP gateway applies the user's connectors
//! and this agent's own tool rules, and the session's sandbox time is metered as the user's.
//!
//! A session opened on the platform's agent before users had their own stays bound to it.

use nasiko_runtime::harness_agent;
use sqlx::PgPool;
use uuid::Uuid;

use super::catalog::{Harness, HarnessCatalog, PLATFORM_HARNESS_AGENT, qualified_image};
use super::model::SessionStore;
use crate::coding::proxy::HarnessTarget;

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
/// writable, starting with the platform agent's approval setting. Ask the session admission
/// first: creating it may be what the admission refuses.
pub async fn ensure(
    db: &PgPool,
    user_id: Uuid,
    harness: &Harness,
    image_prefix: &str,
) -> Result<Uuid, OwnAgentError> {
    let name = harness_agent::name(&harness.id);
    let created: Option<Uuid> = sqlx::query_scalar(&format!(
        "INSERT INTO agents (name, display_name, description, owner_id, image, status, is_public, \
                             workload_kind, harness_id, parking_policy, writable, hitl_enabled) \
         VALUES ($1, $2, $3, $4, $5, 'parked', false, 'harness', $6, 'auto', true, \
                 COALESCE((SELECT hitl_enabled FROM agents \
                           WHERE {PLATFORM_HARNESS_AGENT} AND harness_id = $6 \
                             AND deleted_at IS NULL \
                           ORDER BY created_at LIMIT 1), false)) \
         ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO NOTHING \
         RETURNING id"
    ))
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

/// The agent a user's message to the harness agent `target` runs as: `None` keeps `target`,
/// otherwise the user's own harness agent, created now if need be. `target` is kept when it is not
/// a platform harness agent, when the message continues a session of the user's opened on it
/// before users had their own, and when its contextId is someone else's (refused where it was
/// addressed, without creating anything for the caller). Ask after the session admission.
pub async fn for_message(
    db: &PgPool,
    image_prefix: &str,
    user_id: Uuid,
    target: &HarnessTarget,
    context_id: Option<&str>,
) -> Result<Option<Uuid>, OwnAgentError> {
    if !target.is_platform() {
        return Ok(None);
    }
    if let Some(context_id) = context_id
        && let Some((holder, agent)) = SessionStore::new(db.clone()).holder(context_id).await?
        && (holder != Some(user_id) || agent == Some(target.agent_id))
    {
        return Ok(None);
    }
    // A harness withdrawn from the catalog is refused by the turn itself; nothing to create here.
    let Some(harness) = HarnessCatalog::new(db.clone())
        .get(&target.harness_id)
        .await?
    else {
        return Ok(None);
    };
    ensure(db, user_id, &harness, image_prefix).await.map(Some)
}

/// The agents whose sessions a user's `tasks/*` request to `target` may name: the addressed one,
/// and, for a platform harness agent, the user's own agent its messages run as.
pub async fn task_agents(
    db: &PgPool,
    user_id: Uuid,
    target: &HarnessTarget,
) -> sqlx::Result<Vec<Uuid>> {
    let mut agents = vec![target.agent_id];
    if target.is_platform()
        && let Some(own) = find(db, user_id, &target.harness_id).await?
    {
        agents.push(own);
    }
    Ok(agents)
}
