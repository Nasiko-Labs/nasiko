//! Deploying an agent without a workload.
//!
//! A parkable agent keeps its image and its `agents` row at rest and gets a sandbox only
//! while it serves a request (the resolver acquires one on the first request). Every deploy
//! path asks [`deployment_mode`] once, after the image is ready and before it builds the
//! deploy env, and either deploys as always or calls [`park_agent`].
//!
//! A parked deploy skips `deploy_env::build`, which is what resets `state_mode` for a new
//! image, so a parked redeploy keeps the agent's `state_mode` as it was.

use std::collections::HashMap;

use nasiko_runtime::{
    ContainerId, RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxState,
};
use uuid::Uuid;

use crate::agent_scaling::ParkingEligibility;
use crate::state::AppState;

/// Why parking an agent failed.
#[derive(Debug, thiserror::Error)]
pub enum ParkError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("runtime: {0}")]
    Runtime(#[from] RuntimeError),
}

/// How a deploy leaves the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentMode {
    /// A running workload, as every agent had before parking.
    Workload,
    /// No workload; the first request acquires a sandbox.
    Parked(ParkingEligibility),
}

/// Whether the caller's request carries configuration a woken sandbox cannot yet apply (its
/// own env, or ports other than the default), so the agent must deploy a workload.
///
/// TODO NAS-1105: drop this once the wake spec can carry a caller's env and ports.
pub fn names_a_workload(env: &HashMap<String, String>, ports: &[u16]) -> bool {
    let default_ports = ports.is_empty() || ports == [crate::agents::DEFAULT_AGENT_PORT];
    !env.is_empty() || !default_ports
}

/// The deploy decision: parked only when the agent may park **and** the sandbox provider that
/// would wake it is available. Logs one line at info; lifecycle callers use [`parks`].
pub async fn deployment_mode(state: &AppState, agent_id: Uuid) -> sqlx::Result<DeploymentMode> {
    mode(state, agent_id, true).await
}

/// [`deployment_mode`] for callers that are not deploying (stop, start, restart, scale,
/// reconcile): whether the agent runs without a workload. Logs at debug.
pub async fn parks(state: &AppState, agent_id: Uuid) -> sqlx::Result<bool> {
    Ok(matches!(
        mode(state, agent_id, false).await?,
        DeploymentMode::Parked(_)
    ))
}

async fn mode(state: &AppState, agent_id: Uuid, deploying: bool) -> sqlx::Result<DeploymentMode> {
    let eligibility = state.agent_scaling.parking_eligibility(agent_id).await?;
    let available = state
        .sandbox
        .is_available_for(&SandboxKey::Agent { agent_id });
    let mode = decide(eligibility.clone(), available);
    let (label, reason) = match (&mode, &eligibility) {
        (DeploymentMode::Parked(_), _) => ("parked", "parkable".to_owned()),
        (_, ParkingEligibility::Resident(why)) => ("workload", format!("resident: {why:?}")),
        (_, ParkingEligibility::Parkable) => ("workload", "sandbox unavailable".to_owned()),
    };
    if deploying {
        tracing::info!(%agent_id, mode = label, %reason, "deployment mode");
    } else {
        tracing::debug!(%agent_id, mode = label, %reason, "deployment mode");
    }
    Ok(mode)
}

fn decide(eligibility: ParkingEligibility, sandbox_available: bool) -> DeploymentMode {
    match eligibility {
        ParkingEligibility::Parkable if sandbox_available => DeploymentMode::Parked(eligibility),
        _ => DeploymentMode::Workload,
    }
}

/// Parks the agent if [`deployment_mode`] says so; `Ok(true)` when it did.
///
/// `image` is written to the row when given (a deploy that carries a new image).
pub async fn park_if_eligible(
    state: &AppState,
    agent_id: Uuid,
    image: Option<&str>,
) -> Result<bool, ParkError> {
    match deployment_mode(state, agent_id).await? {
        DeploymentMode::Workload => Ok(false),
        DeploymentMode::Parked(_) => {
            park_agent(state, agent_id, image).await?;
            Ok(true)
        }
    }
}

/// Leaves the agent parked: no workload, no stale sandbox, row at rest.
///
/// The sandbox is looked up **before** the row flips, then the row goes (with the new image),
/// then the workload is destroyed, then only the sandbox that was found is released. A request
/// that arrives after the row flipped wakes the new image into a sandbox this call never saw,
/// so it is not released; a stale old-image sandbox is. A request that wakes the same key
/// before the release cannot be told apart from the stale one and is released with it; the
/// next request wakes it again. Failures are returned: a workload or sandbox left behind
/// would still serve the old image. The stored credentials are cleared once any sandbox is
/// released (or none was found).
pub async fn park_agent(
    state: &AppState,
    agent_id: Uuid,
    image: Option<&str>,
) -> Result<(), ParkError> {
    let stale = agent_sandbox(state, agent_id).await?;
    mark_parked(&state.db, agent_id, image).await?;
    destroy_workload(state, agent_id).await?;
    if let Some(info) = stale {
        state.sandbox.release(&info.handle).await?;
    }
    // Cleared even when no sandbox was found: a parked redeploy must mint for the new image.
    crate::agents::sandbox_credentials::clear_agent_sandbox_credentials(&state.db, agent_id)
        .await?;
    Ok(())
}

/// Releases the agent's `a-{id}` sandbox, if it has one, and then forgets its stored
/// credentials: the sandbox and the env it was created with are gone, so the next wake is
/// created from the current image with newly minted credentials. The credentials are cleared
/// only after a successful release, so a failed release leaves a sandbox that still works.
/// Every new image must pass through here (`stop` only suspends, and keeps them).
pub async fn release_agent_sandbox(state: &AppState, agent_id: Uuid) -> Result<(), ParkError> {
    if let Some(info) = agent_sandbox(state, agent_id).await? {
        state.sandbox.release(&info.handle).await?;
    }
    crate::agents::sandbox_credentials::clear_agent_sandbox_credentials(&state.db, agent_id)
        .await?;
    Ok(())
}

/// What reconcile found when it looked at an agent with no workload.
#[derive(Debug, PartialEq, Eq)]
pub enum Unserved {
    /// The agent cannot park (or no sandbox provider is available): redeploy it.
    NotParkable,
    /// A woken agent is serving from its sandbox: leave it alone.
    Serving,
    /// It was resting without a workload: its row now says `parked`.
    Parked,
}

/// For an agent whose `running` row has no workload: parks it only if it is parkable and not
/// serving from a sandbox. Never destroys or releases anything, since a woken agent has no
/// workload but is live. A sandbox list that cannot be read is an error, so the caller skips.
/// The row is parked through the reaper's fenced claim on the `last_request_at` read here, so a
/// request (or another replica's reconcile) that got there first wins.
pub async fn park_if_unserved(state: &AppState, agent_id: Uuid) -> Result<Unserved, ParkError> {
    if !parks(state, agent_id).await? {
        return Ok(Unserved::NotParkable);
    }
    let last_request: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_request_at FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    if let Some(info) = agent_sandbox(state, agent_id).await?
        && matches!(info.state, SandboxState::Running | SandboxState::Starting)
    {
        return Ok(Unserved::Serving);
    }
    if crate::sessions::reaper::agents::claim_park(&state.db, agent_id, last_request).await? {
        Ok(Unserved::Parked)
    } else {
        Ok(Unserved::Serving)
    }
}

/// The state of the agent's own sandbox, if it has one.
pub async fn sandbox_state(
    state: &AppState,
    agent_id: Uuid,
) -> Result<Option<SandboxState>, RuntimeError> {
    Ok(agent_sandbox(state, agent_id).await?.map(|info| info.state))
}

/// Destroys the agent's workload. Idempotent: no workload is `Ok`.
pub async fn destroy_workload(state: &AppState, agent_id: Uuid) -> Result<(), RuntimeError> {
    state
        .runtime
        .destroy(&ContainerId::from_uuid(agent_id))
        .await
}

/// Writes the resting state in one transaction: `parked`, no address, no last request, and no
/// live deployment row for the crash guardian to probe.
pub async fn mark_parked(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    image: Option<&str>,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "UPDATE agents SET status = 'parked', url = NULL, last_request_at = NULL, \
         image = COALESCE($2, image), updated_at = now() WHERE id = $1",
    )
    .bind(agent_id)
    .bind(image)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE agent_deployments SET status = 'stopped', updated_at = now() \
         WHERE agent_id = $1 AND status <> 'stopped'",
    )
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// The agent's sandbox, if the provider has one.
///
/// A [`SandboxHandle`] needs the backend's own ref, which is not stored for plain agents, so
/// it is found by key in the provider's `list`.
async fn agent_sandbox(
    state: &AppState,
    agent_id: Uuid,
) -> Result<Option<SandboxInfo>, RuntimeError> {
    // An unavailable provider has no sandboxes, and its `list` always errors.
    if !state
        .sandbox
        .is_available_for(&SandboxKey::Agent { agent_id })
    {
        tracing::debug!(%agent_id, "sandbox provider unavailable: no agent sandbox to find");
        return Ok(None);
    }
    let key = SandboxKey::Agent { agent_id };
    Ok(state
        .sandbox
        .list()
        .await?
        .into_iter()
        .find(|info| info.handle.key == key))
}

/// Suspends the agent's sandbox, if any. Best effort: a failure is logged, because `stop`'s
/// job is the row and the workload.
pub async fn suspend_sandbox(state: &AppState, agent_id: Uuid) {
    let handle: Option<SandboxHandle> = match agent_sandbox(state, agent_id).await {
        Ok(info) => info.map(|info| info.handle),
        Err(e) => {
            tracing::warn!(%agent_id, error = %e, "could not list sandboxes");
            return;
        }
    };
    if let Some(handle) = handle
        && let Err(e) = state.sandbox.suspend(&handle).await
    {
        tracing::warn!(%agent_id, error = %e, "could not suspend the agent's sandbox");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_scaling::ResidentReason;

    #[test]
    fn parkable_with_a_sandbox_parks() {
        assert_eq!(
            decide(ParkingEligibility::Parkable, true),
            DeploymentMode::Parked(ParkingEligibility::Parkable)
        );
    }

    #[test]
    fn parkable_without_a_sandbox_deploys_a_workload() {
        assert_eq!(
            decide(ParkingEligibility::Parkable, false),
            DeploymentMode::Workload
        );
    }

    #[test]
    fn resident_with_a_sandbox_deploys_a_workload() {
        let resident = ParkingEligibility::Resident(ResidentReason::PolicyResident);
        assert_eq!(decide(resident, true), DeploymentMode::Workload);
    }

    #[test]
    fn resident_without_a_sandbox_deploys_a_workload() {
        let resident = ParkingEligibility::Resident(ResidentReason::InMemoryState);
        assert_eq!(decide(resident, false), DeploymentMode::Workload);
    }

    #[test]
    fn caller_env_or_non_default_ports_name_a_workload() {
        let none = HashMap::new();
        assert!(!names_a_workload(&none, &[]));
        assert!(!names_a_workload(
            &none,
            &[crate::agents::DEFAULT_AGENT_PORT]
        ));
        assert!(names_a_workload(&none, &[9000]));
        let env = HashMap::from([("A".to_owned(), "b".to_owned())]);
        assert!(names_a_workload(&env, &[]));
    }
}
