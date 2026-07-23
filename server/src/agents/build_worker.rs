use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::AppState;

const MAX_ATTEMPTS: i32 = 3;
/// How old a job must be before it is considered stuck (not legitimately slow).
/// Must exceed the runtime's build_timeout (Docker + K8s both default to 30 min).
/// 2× headroom avoids false positives on large images.
const STUCK_JOB_MINS: i64 = 60;

use super::update::{AgentVersionRow, execute_agent_rollback, execute_agent_update};
use super::upload::{BuildJobPayload, execute_clone_and_deploy, execute_github_clone_and_deploy, execute_upload_and_deploy};
use crate::build::routes::execute_build;

#[derive(Debug, sqlx::FromRow)]
struct BuildJob {
    id: Uuid,
    agent_id: Uuid,
    payload: serde_json::Value,
    attempt: i32,
}

/// Main build worker loop. Spawned once at server startup.
///
/// Receives notifications on `notify` when a new job is queued; also polls
/// every 5 seconds as a fallback in case a notification is lost.
///
/// A separate periodic sweep (every 10 minutes) re-runs `recover_stuck_jobs` to
/// catch jobs left `in_progress` by a crashed replica — without this, a multi-replica
/// cluster where no replica restarts would leave stuck jobs stranded indefinitely.
///
/// After each successful claim, drains the queue immediately before sleeping
/// to avoid a 5-second lag when multiple jobs arrive in a burst.
pub async fn run(state: AppState, mut notify: mpsc::Receiver<()>) {
    recover_stuck_jobs(&state.db).await;

    // First tick fires after the interval, not immediately — startup already ran recovery.
    let recovery_start = tokio::time::Instant::now() + Duration::from_secs(10 * 60);
    let mut recovery_tick = tokio::time::interval_at(recovery_start, Duration::from_secs(10 * 60));
    recovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tracing::info!("build worker: started");
    loop {
        tokio::select! {
            msg = notify.recv() => {
                if msg.is_none() {
                    // Sender was dropped — server is shutting down.
                    tracing::info!("build worker: notification channel closed, exiting");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = recovery_tick.tick() => {
                recover_stuck_jobs(&state.db).await;
                // Fall through to the drain loop: recovered jobs are now pending.
            }
        }
        // Drain: keep claiming jobs until the queue is empty.
        // Claim runs in the outer task (minimal, no panic risk).
        // Execute runs in a spawned task for panic isolation.
        // Separating the two means we always have the job_id available in the panic arm,
        // enabling immediate reset instead of waiting up to STUCK_JOB_MINS.
        loop {
            // Phase 1: claim (no panic risk — just DB reads/writes)
            let job = match claim_next_job(&state).await {
                Ok(Some(j)) => j,
                Ok(None) => {
                    tracing::debug!("build worker: queue empty");
                    break;
                }
                Err(e) => {
                    tracing::error!(%e, "build worker: claim error");
                    break;
                }
            };

            let job_id = job.id;
            let old_attempt = job.attempt; // pre-increment; DB now holds old_attempt + 1

            // Inline attempt cap: fail immediately if this claim pushed attempt over the limit.
            if old_attempt >= MAX_ATTEMPTS {
                mark_job(&state.db, job_id, "failed", Some("max attempts exceeded")).await;
                // Must also terminalize the agent here, not just the job row: once this
                // row is 'failed' it falls outside recover_stuck_jobs's exhausted-job
                // query (which only re-scans rows still 'in_progress'), so without this
                // call the agent would stay 'deploying' forever with no path back to a
                // terminal state (RUN-4).
                fail_agent_terminal(&state.db, job.agent_id).await;
                tracing::warn!(job_id = %job_id, attempt = old_attempt + 1, "build worker: job exceeded max attempts");
                continue; // try next job
            }

            // Phase 2: execute in a spawned task (panic-isolated)
            let state_clone = state.clone();
            match tokio::task::spawn(async move { execute_claimed_job(state_clone, job).await })
                .await
            {
                Ok(()) => {} // job finished (success or failure recorded in DB by execute_claimed_job)
                Err(ref e) if e.is_panic() => {
                    tracing::error!(job_id = %job_id, "build worker: job panicked — resetting immediately");
                    reset_panicked_job(&state.db, job_id, old_attempt).await;
                }
                Err(_) => break, // task cancelled (server shutdown)
            }
        }
    }
}

/// Handle jobs left `in_progress` for longer than `STUCK_JOB_MINS`.
///
/// Called at startup and periodically (every 10 min) so stuck jobs from a crashed
/// replica are recovered without requiring a server restart.
/// The `make_interval` form keeps the threshold in one place rather than
/// embedding it as a string literal in two separate SQL statements.
async fn recover_stuck_jobs(db: &PgPool) {
    // Permanently fail exhausted jobs (>= MAX_ATTEMPTS attempts already made).
    // RETURNING agent_id so we can also drive those agents to a terminal state
    // (RUN-4) — otherwise agent_builds stays 'building' / agents 'deploying' and
    // the deploy SSE waits forever.
    match sqlx::query_as::<_, (Uuid,)>(
        "UPDATE build_jobs SET status = 'failed', error_msg = 'max attempts exceeded', completed_at = now()
         WHERE status = 'in_progress' AND picked_at < now() - make_interval(mins => $2::int) AND attempt >= $1
         RETURNING agent_id",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STUCK_JOB_MINS)
    .fetch_all(db)
    .await
    {
        Ok(rows) => {
            for (agent_id,) in rows {
                fail_agent_terminal(db, agent_id).await;
            }
        }
        Err(e) => tracing::error!(%e, "build worker: exhausted-job recovery query failed"),
    }

    // Reset remaining stuck jobs so they get another try.
    match sqlx::query(
        "UPDATE build_jobs SET status = 'pending', picked_at = NULL
         WHERE status = 'in_progress' AND picked_at < now() - make_interval(mins => $2::int) AND attempt < $1",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STUCK_JOB_MINS)
    .execute(db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::warn!(count = r.rows_affected(), "build worker: reset stuck in_progress jobs");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(%e, "build worker: stuck-job recovery query failed");
        }
    }
}

/// Claim one pending job from the queue.
///
/// Sets status to `in_progress` and increments attempt within a transaction.
/// Returns `Ok(None)` if the queue is empty. The returned `job.attempt` is the
/// pre-increment value; the DB now holds `attempt + 1`.
async fn claim_next_job(state: &AppState) -> anyhow::Result<Option<BuildJob>> {
    let mut tx = state.db.begin().await?;

    let job = sqlx::query_as::<_, BuildJob>(
        "SELECT id, agent_id, payload, attempt
         FROM build_jobs
         WHERE status = 'pending'
         ORDER BY created_at
         FOR UPDATE SKIP LOCKED
         LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(job) = job else {
        tx.rollback().await?;
        return Ok(None);
    };

    sqlx::query(
        "UPDATE build_jobs SET status = 'in_progress', picked_at = now(), attempt = attempt + 1 WHERE id = $1",
    )
    .bind(job.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Some(job))
}

/// Execute an already-claimed build job.
///
/// Runs as a `tokio::task::spawn` target so panics are isolated from the worker loop.
/// Marks the job done or failed in the DB before returning — the caller does not
/// need to do any DB cleanup on `Ok(())`.
async fn execute_claimed_job(state: AppState, job: BuildJob) {
    let payload: BuildJobPayload = match serde_json::from_value(job.payload) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(job_id = %job.id, %e, "build worker: invalid payload, marking failed");
            mark_job(&state.db, job.id, "failed", Some("invalid job payload")).await;
            return;
        }
    };

    let build_id = payload.build_id();

    tracing::info!(
        job_id = %job.id,
        agent_id = %job.agent_id,
        name = %payload.label(),
        "build worker: job claimed"
    );

    let start = std::time::Instant::now();

    match payload {
        BuildJobPayload::Upload {
            build_id: _,
            agent_id,
            owner_id,
            upload_id,
            name,
            zip_path,
            image_tag,
            ports,
            env,
        } => {
            let mut platform_env = state.agent_env(agent_id).await;
            platform_env.extend(env);
            execute_upload_and_deploy(
                state.runtime.clone(),
                state.db.clone(),
                state.http_client.clone(),
                build_id,
                agent_id,
                owner_id,
                upload_id,
                name,
                std::path::PathBuf::from(&zip_path),
                image_tag,
                ports,
                platform_env,
                // Inject server LLM secrets at execution — not persisted in the payload (RUN-5).
                state.config.openai_api_key.clone(),
                state.config.openai_base_url.clone(),
                state.config.agent_runtime.clone(),
                state.config.agent_image_registry.clone(),
            )
            .await;
        }

        BuildJobPayload::Update {
            build_id: _,
            agent_id,
            owner_id,
            name,
            zip_path,
            image_tag,
            new_version,
            prev_version,
            prev_image,
            changelog,
        } => {
            let source_data = match zip_path {
                Some(ref path) => match tokio::fs::read(path).await {
                    Ok(data) => Some(data),
                    Err(e) => {
                        tracing::error!(job_id = %job.id, %path, %e, "build worker: cannot read update zip");
                        mark_job(
                            &state.db,
                            job.id,
                            "failed",
                            Some("failed to read update source"),
                        )
                        .await;
                        return;
                    }
                },
                None => None,
            };
            execute_agent_update(
                state.clone(),
                build_id,
                agent_id,
                owner_id,
                name,
                source_data,
                image_tag,
                new_version,
                prev_version,
                prev_image,
                changelog,
            )
            .await;
        }

        BuildJobPayload::Rollback {
            rollback_build_id: _,
            agent_id,
            caller_id,
            agent_name,
            target_version,
            target_image_tag,
            reason,
        } => {
            let target = AgentVersionRow {
                version: target_version,
                image_tag: target_image_tag,
                can_rollback: true,
            };
            execute_agent_rollback(
                state.clone(),
                build_id,
                agent_id,
                caller_id,
                agent_name,
                target,
                reason,
            )
            .await;
        }

        BuildJobPayload::Clone {
            build_id: _,
            agent_id,
            owner_id,
            upload_id,
            name,
            tar_gz_path,
            image_tag,
            ports,
            env,
        } => {
            let mut platform_env = state.agent_env(agent_id).await;
            platform_env.extend(env);
            execute_clone_and_deploy(
                state.runtime.clone(),
                state.db.clone(),
                state.http_client.clone(),
                build_id,
                agent_id,
                owner_id,
                upload_id,
                name,
                std::path::PathBuf::from(&tar_gz_path),
                image_tag,
                ports,
                platform_env,
                state.config.openai_api_key.clone(),
                state.config.openai_base_url.clone(),
                state.config.agent_runtime.clone(),
                state.config.agent_image_registry.clone(),
            )
            .await;
        }

        BuildJobPayload::GithubClone {
            build_id: _,
            agent_id,
            owner_id,
            upload_id,
            name,
            repo_full_name,
            branch,
            image_tag,
            ports,
            env,
        } => {
            execute_github_clone_and_deploy(
                state.clone(),
                build_id,
                agent_id,
                owner_id,
                upload_id,
                name,
                repo_full_name,
                branch,
                image_tag,
                ports,
                env,
            )
            .await;
        }

        BuildJobPayload::StandaloneBuild {
            build_id: _,
            agent_id: _,
            agent_name,
            github_url,
            source_key,
            version_tag,
        } => {
            execute_build(
                state.runtime.clone(),
                state.db.clone(),
                build_id,
                agent_name,
                github_url,
                source_key,
                version_tag,
                state.oci_storage.clone(),
                state.http_client.clone(),
                state.config.git_clone_allowed_hosts.clone(),
                state.config.capability_generator_model.clone(),
            )
            .await;
        }
    }

    // Infer success/failure from the agent_builds status written by the execute function.
    let build_status: Option<String> =
        sqlx::query_scalar("SELECT status::text FROM agent_builds WHERE id = $1")
            .bind(build_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let (status, err) = match build_status.as_deref() {
        Some("success") => {
            tracing::info!(
                job_id = %job.id,
                agent_id = %job.agent_id,
                duration_ms = start.elapsed().as_millis(),
                "build worker: job completed"
            );
            ("done", None)
        }
        _ => {
            tracing::error!(job_id = %job.id, agent_id = %job.agent_id, "build worker: job failed");
            (
                "failed",
                Some("build or deploy step failed — see agent_builds for details".to_string()),
            )
        }
    };

    mark_job(&state.db, job.id, status, err.as_deref()).await;
    // If the agent was deleted mid-build, the cascade will have deleted this build_jobs row —
    // the UPDATE above is a no-op (0 rows affected) and that's fine.
}

/// Immediately reset a panicked job rather than waiting for the `STUCK_JOB_MINS` sweep.
///
/// `old_attempt` is the pre-increment value from the claim. The DB now holds `old_attempt + 1`.
/// If that value is at or above `MAX_ATTEMPTS`, the job is permanently failed;
/// otherwise it is reset to `pending` for immediate retry.
async fn reset_panicked_job(db: &PgPool, job_id: Uuid, old_attempt: i32) {
    if old_attempt >= MAX_ATTEMPTS {
        mark_job(db, job_id, "failed", Some("job panicked during execution")).await;
        // Also terminalize the agent/build so the deploy SSE stops waiting (RUN-4).
        if let Ok(Some((agent_id,))) =
            sqlx::query_as::<_, (Uuid,)>("SELECT agent_id FROM build_jobs WHERE id = $1")
                .bind(job_id)
                .fetch_optional(db)
                .await
        {
            fail_agent_terminal(db, agent_id).await;
        }
    } else if let Err(e) =
        sqlx::query("UPDATE build_jobs SET status = 'pending', picked_at = NULL WHERE id = $1")
            .bind(job_id)
            .execute(db)
            .await
    {
        tracing::error!(job_id = %job_id, %e, "build worker: failed to reset panicked job — will recover via periodic sweep");
    } else {
        tracing::warn!(job_id = %job_id, attempt = old_attempt + 1, "build worker: panicked job reset to pending");
    }
}

/// Drive an agent to a terminal state after its build job is permanently failed
/// (exhaustion or panic), so the deploy-status SSE terminates (RUN-4). The normal
/// per-job failure path already marks `agent_builds` via the execute functions;
/// this covers the paths where execution never completed and left the build stuck
/// `building` / the agent `deploying`. Idempotent + status-guarded.
///
/// For first-time deploys (no prior successful builds) the agents row is deleted
/// rather than set to `status='failed'`, so no orphaned record is left. For existing
/// agents that exceeded max attempts on an update/rollback the row is kept (the caller
/// may still want to redeploy or inspect history).
async fn fail_agent_terminal(db: &PgPool, agent_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE agent_builds SET status = 'failed', updated_at = now() \
         WHERE agent_id = $1 AND status = 'building'",
    )
    .bind(agent_id)
    .execute(db)
    .await;
    super::utils::delete_agent_or_mark_failed(db, agent_id).await;
}

async fn mark_job(db: &PgPool, id: Uuid, status: &str, error: Option<&str>) {
    if let Err(e) = sqlx::query(
        "UPDATE build_jobs SET status = $2, error_msg = $3, completed_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .execute(db)
    .await
    {
        tracing::error!(job_id = %id, %e, "build worker: failed to update job status");
    }
}
