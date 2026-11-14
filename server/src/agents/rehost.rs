//! Moving stored image references onto the registry's current host after a rename.
//!
//! An agent's image is stored with the registry host it was pushed to
//! (`<host>/u-<owner>/<slug>:<tag>`). When the control plane is renamed, its registry keeps
//! answering on the old name (`AGENT_IMAGE_REGISTRY_ALIASES`) for a while, but every stored
//! reference has to move to the new one before the old name goes. The registry itself keeps layers
//! by digest and manifests by repository, so nothing is re-pushed: only references change.
//!
//! At startup, [`rewrite_references`] moves every reference that names an alias, in one
//! transaction, and queues each live agent it moved in `agent_registry_rehosts`. [`redeploy_queued`]
//! then redeploys them: an agent's pull Secret is keyed by host, and its credential's token is only
//! ever shown once, so the old credential is revoked and the redeploy mints one for the new host. An
//! agent with no workload (parked before parking kept workloads, or failed) is only revoked: its next
//! deploy or wake builds from the row and mints its own. A row leaves the queue only when its
//! redeploy succeeded, so a failed one is retried ([`retry_until_drained`]).

use std::time::Duration;

use nasiko_runtime::{ContainerId, RuntimeError};
use tracing::{info, warn};
use uuid::Uuid;

use crate::agents::catalog_deploy;
use crate::state::AppState;

/// Every column that stores a full image reference, host included.
const IMAGE_COLUMNS: &[(&str, &str)] = &[
    ("agents", "image"),
    ("agent_deployments", "spec_image"),
    ("agent_builds", "image_reference"),
    ("agent_versions", "image_tag"),
    ("mcp_connectors", "container_image_tag"),
    ("mcp_connector_builds", "image_tag"),
];

/// How long a pass that left agents queued waits before the next.
const RETRY_INTERVAL: Duration = Duration::from_secs(600);

/// Moves every stored reference naming one of `aliases` onto `registry` and queues the live
/// agents whose image moved. Returns how many agents were queued. A no-op without a registry or
/// aliases, and on every start after the first (nothing names an alias any more).
pub async fn rewrite_references(
    db: &sqlx::PgPool,
    registry: &str,
    aliases: &[String],
) -> sqlx::Result<u64> {
    if registry.is_empty() || aliases.is_empty() {
        return Ok(0);
    }
    let mut tx = db.begin().await?;
    let mut queued = 0;
    for alias in aliases {
        let moved: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE agents SET image = $2 || substr(image, length($1) + 1) \
             WHERE left(image, length($1) + 1) = $1 || '/' RETURNING id",
        )
        .bind(alias)
        .bind(registry)
        .fetch_all(&mut *tx)
        .await?;
        queued += sqlx::query(
            "INSERT INTO agent_registry_rehosts (agent_id) \
             SELECT id FROM agents WHERE id = ANY($1) AND deleted_at IS NULL \
             ON CONFLICT DO NOTHING",
        )
        .bind(&moved)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        for (table, column) in &IMAGE_COLUMNS[1..] {
            sqlx::query(&format!(
                "UPDATE {table} SET {column} = $2 || substr({column}, length($1) + 1) \
                 WHERE left({column}, length($1) + 1) = $1 || '/'"
            ))
            .bind(alias)
            .bind(registry)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(queued)
}

/// One pass over the queue: redeploys each queued agent once. Returns how many are still queued
/// (their redeploy failed). Each row is held locked while its agent redeploys, so two replicas
/// never redeploy the same agent, and is only removed once the redeploy succeeded.
pub async fn redeploy_queued(state: &AppState) -> sqlx::Result<usize> {
    let mut failed: Vec<Uuid> = Vec::new();
    loop {
        let mut tx = state.db.begin().await?;
        let next: Option<Uuid> = sqlx::query_scalar(
            "SELECT agent_id FROM agent_registry_rehosts WHERE agent_id <> ALL($1) \
             ORDER BY queued_at, agent_id LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .bind(&failed)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(agent_id) = next else {
            return Ok(failed.len());
        };
        match redeploy(state, agent_id).await {
            Ok(()) => {
                sqlx::query("DELETE FROM agent_registry_rehosts WHERE agent_id = $1")
                    .bind(agent_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                info!(%agent_id, "registry rehost: agent moved to the current registry host");
            }
            Err(e) => {
                tx.rollback().await?;
                warn!(%agent_id, error = %e, "registry rehost: redeploy failed, will retry");
                failed.push(agent_id);
            }
        }
    }
}

/// Runs passes until the queue is empty, waiting [`RETRY_INTERVAL`] after a pass that left some.
pub async fn retry_until_drained(state: AppState) {
    loop {
        match redeploy_queued(&state).await {
            Ok(0) => return,
            Ok(left) => warn!(left, "registry rehost: agents still queued, retrying later"),
            Err(e) => warn!(error = %e, "registry rehost: queue unreadable, retrying later"),
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

/// Revokes the agent's pull credential and, when it has a workload, redeploys it from its row
/// (the deploy mints a credential for the current host; a workload scaled to zero stays at zero).
async fn redeploy(state: &AppState, agent_id: Uuid) -> Result<(), String> {
    nasiko_oci::pull_credentials::revoke(&state.db, agent_id)
        .await
        .map_err(|e| format!("revoking the pull credential: {e}"))?;
    let Some(agent) = catalog_deploy::load(&state.db, agent_id)
        .await
        .map_err(|e| format!("loading the agent: {e}"))?
    else {
        return Ok(());
    };
    match state
        .runtime
        .status(&ContainerId::from_uuid(agent_id))
        .await
    {
        Err(RuntimeError::ContainerNotFound(_)) => return Ok(()),
        Err(e) => return Err(format!("reading the workload: {e}")),
        Ok(_) => {}
    }
    let deployed = catalog_deploy::deploy(state, &agent)
        .await
        .map_err(|e| format!("redeploying: {e}"))?;
    if !deployed.url.is_empty() {
        sqlx::query("UPDATE agents SET url = $2, updated_at = now() WHERE id = $1")
            .bind(agent_id)
            .bind(&deployed.url)
            .execute(&state.db)
            .await
            .map_err(|e| format!("recording the url: {e}"))?;
    }
    Ok(())
}
