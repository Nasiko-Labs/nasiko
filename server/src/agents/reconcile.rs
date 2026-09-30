//! Startup reconciliation: redeploys any agent whose DB row says
//! `status = 'running'` but has no live runtime resource — e.g. after a
//! tenant cluster restore recreates the database and rustfs/registry data
//! but not the Kubernetes Deployments/Services (the restore path only restarts
//! `nasiko-server` itself; it never touches individual agent workloads — this
//! closes that gap from the side that actually has the runtime handle).
//!
//! General-purpose, not restore-specific: this also repairs any cluster
//! whose deployments were wiped by drift outside a restore. Mirrors
//! `seed.rs`'s existing `needs_deploy` check (same per-agent `runtime.status`
//! probe, same redeploy path) rather than inventing a new mechanism.

use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentStatus, RuntimeError, RuntimeState};

/// The bounded slice of an `agents` row this pass actually needs — not the
/// full `Agent` model (~30 columns), most of which are irrelevant here.
#[derive(sqlx::FromRow)]
struct ReconcilableAgent {
    id: Uuid,
    name: String,
    image: Option<String>,
    owner_id: Uuid,
    writable: bool,
    writable_path: Option<String>,
}

/// Runs once at startup (see `AppState::init`). Safe to call on every boot —
/// it's a no-op wherever nothing is actually missing, at the cost of one
/// `runtime.status()` call per `running` agent; acceptable since `seed.rs`
/// already pays the same per-agent cost for the (usually much smaller) seed
/// list on every boot.
pub async fn reconcile_agents_on_startup(state: &AppState) {
    let agents = match sqlx::query_as::<_, ReconcilableAgent>(
        "SELECT id, name, image, owner_id, writable, writable_path FROM agents \
         WHERE status = 'running' AND deleted_at IS NULL AND image IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "agent reconciliation: failed to list running agents, skipping");
            return;
        }
    };

    for agent in agents {
        let Some(image) = agent.image.clone() else {
            continue;
        };
        let container_id = ContainerId::from_uuid(agent.id);
        if !agent_needs_redeploy(state.runtime.status(&container_id).await) {
            continue;
        }

        info!(
            agent_id = %agent.id,
            name = %agent.name,
            "agent reconciliation: no live workload for a DB-running agent, redeploying"
        );

        let mut env = state.agent_env(agent.id).await;
        // Same env-wiring every other deploy path applies (rollback, seed,
        // upload) — a redeployed-but-recovered agent must behave identically
        // to one that was deployed normally, not miss router wiring.
        crate::llm_router::wiring::inject_agent_llm_env(
            &state.db,
            &mut env,
            agent.id,
            Some(agent.owner_id),
        )
        .await;

        let qualified_image =
            crate::agents::qualify_deploy_image(&state.config.agent_image_registry, &image);
        let mut spec = crate::agents::build_agent_spec(
            agent.id,
            &agent.name,
            qualified_image.clone(),
            vec![],
            env,
            &state.config.agent_default_memory,
            state.config.agent_max_replicas,
            agent.writable,
            agent.writable_path.clone(),
            agent.owner_id,
        );
        crate::agents::attach_pull_credential(
            &state.db,
            &state.config.agent_runtime,
            &state.config.agent_image_registry,
            &mut spec,
            agent.id,
        )
        .await;

        match state.runtime.deploy(&spec).await {
            Ok(deploy_status) => {
                let agent_url = crate::agents::resolve_agent_url(
                    &state.runtime,
                    &deploy_status,
                    &spec.container_id,
                )
                .await;
                let _ = sqlx::query("UPDATE agents SET url = $2, updated_at = now() WHERE id = $1")
                    .bind(agent.id)
                    .bind(&agent_url)
                    .execute(&state.db)
                    .await;
                // Without this the crash-loop guardian never sees this agent
                // again (same reasoning as `seed.rs`'s identical call).
                crate::agents::utils::ensure_deployment_tracked(
                    &state.db,
                    agent.id,
                    Some(agent.owner_id),
                    &qualified_image,
                )
                .await;
                info!(agent_id = %agent.id, "agent reconciliation: redeployed successfully");
            }
            Err(e) => {
                // Leave `agents.status` as `running` — matches how
                // `BackupOrchestrator` leaves state for an operator/retry
                // rather than guessing at a terminal status here.
                warn!(agent_id = %agent.id, error = %e, "agent reconciliation: redeploy failed");
            }
        }
    }
}

/// Pure decision given a `runtime.status()` result: any error (not found,
/// backend unreachable) or a non-`Running` state means the workload isn't
/// actually there. Identical logic to `seed.rs`'s inline `needs_deploy`
/// check, extracted here so it's unit-testable without a real runtime.
fn agent_needs_redeploy(status: Result<DeploymentStatus, RuntimeError>) -> bool {
    match status {
        Ok(s) => s.state != RuntimeState::Running,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: RuntimeState) -> DeploymentStatus {
        DeploymentStatus {
            container_id: ContainerId::from_uuid(uuid::Uuid::nil()),
            state,
            replicas_live: if state == RuntimeState::Running { 1 } else { 0 },
            endpoint: None,
            message: None,
            restart_count: 0,
        }
    }

    #[test]
    fn a_running_workload_does_not_need_redeploy() {
        assert!(!agent_needs_redeploy(Ok(status(RuntimeState::Running))));
    }

    #[test]
    fn a_missing_workload_needs_redeploy() {
        assert!(agent_needs_redeploy(Err(RuntimeError::ContainerNotFound(
            ContainerId::from_uuid(uuid::Uuid::nil())
        ))));
    }

    #[test]
    fn a_non_running_live_workload_needs_redeploy() {
        assert!(agent_needs_redeploy(Ok(status(RuntimeState::Pending))));
    }
}
