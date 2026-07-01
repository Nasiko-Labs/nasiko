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
use super::upload::{BuildJobPayload, execute_upload_and_deploy};
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
    let mut recovery_tick =
        tokio::time::interval_at(recovery_start, Duration::from_secs(10 * 60));
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
        // Each job runs in a spawned task so a panicking build job cannot kill
        // the outer loop. Panicked jobs are recovered by the next recovery_tick.
        loop {
            let state_clone = state.clone();
            match tokio::task::spawn(async move { try_claim_and_run(state_clone).await }).await {
                Ok(Ok(true)) => {} // job ran, there may be more
                Ok(Ok(false)) => {
                    tracing::debug!("build worker: queue empty");
                    break;
                }
                Ok(Err(e)) => {
                    tracing::error!(%e, "build worker: claim/run error");
                    break;
                }
                Err(ref e) if e.is_panic() => {
                    tracing::error!("build worker: job task panicked — job left in_progress, will recover at next startup");
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
    if let Err(e) = sqlx::query(
        "UPDATE build_jobs SET status = 'failed', error_msg = 'max attempts exceeded', completed_at = now()
         WHERE status = 'in_progress' AND picked_at < now() - make_interval(mins => $2) AND attempt >= $1",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STUCK_JOB_MINS)
    .execute(db)
    .await
    {
        tracing::error!(%e, "build worker: exhausted-job recovery query failed");
    }

    // Reset remaining stuck jobs so they get another try.
    match sqlx::query(
        "UPDATE build_jobs SET status = 'pending', picked_at = NULL
         WHERE status = 'in_progress' AND picked_at < now() - make_interval(mins => $2) AND attempt < $1",
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

/// Claim one pending job and execute it. Returns `Ok(true)` if a job was found
/// and run (caller should loop), `Ok(false)` if the queue is empty.
async fn try_claim_and_run(state: AppState) -> anyhow::Result<bool> {
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
        return Ok(false);
    };

    sqlx::query(
        "UPDATE build_jobs SET status = 'in_progress', picked_at = now(), attempt = attempt + 1 WHERE id = $1",
    )
    .bind(job.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Inline attempt cap: fail immediately if we've already exhausted all retries.
    // job.attempt is the pre-increment value; after the UPDATE, DB holds attempt+1.
    // We cap when DB value > MAX_ATTEMPTS, i.e., job.attempt >= MAX_ATTEMPTS.
    if job.attempt >= MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE build_jobs SET status = 'failed', error_msg = 'max attempts exceeded', completed_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .execute(&state.db)
        .await
        .ok();
        tracing::warn!(job_id = %job.id, attempt = job.attempt + 1, "build worker: job exceeded max attempts, failing");
        return Ok(true); // consumed a slot; try next
    }

    let payload: BuildJobPayload = match serde_json::from_value(job.payload) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(job_id = %job.id, %e, "build worker: invalid payload, marking failed");
            mark_job(&state.db, job.id, "failed", Some(&e.to_string())).await;
            return Ok(true);
        }
    };

    let build_id = payload.build_id();
    let label = payload.label().to_owned();

    tracing::info!(
        job_id = %job.id,
        agent_id = %job.agent_id,
        name = %label,
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
            execute_upload_and_deploy(
                state.runtime.clone(),
                state.db.clone(),
                build_id,
                agent_id,
                owner_id,
                upload_id,
                name,
                std::path::PathBuf::from(&zip_path),
                image_tag,
                ports,
                env,
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
                        mark_job(&state.db, job.id, "failed", Some(&format!("read zip: {e}"))).await;
                        return Ok(true);
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
            execute_agent_rollback(state.clone(), build_id, agent_id, caller_id, agent_name, target, reason).await;
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
            )
            .await;
        }
    }

    // Infer success/failure from the agent_builds status written by the execute function.
    let build_status: Option<String> = sqlx::query_scalar(
        "SELECT status::text FROM agent_builds WHERE id = $1",
    )
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
            tracing::error!(
                job_id = %job.id,
                agent_id = %job.agent_id,
                "build worker: job failed"
            );
            ("failed", Some("build or deploy step failed — see agent_builds for details".to_string()))
        }
    };

    mark_job(&state.db, job.id, status, err.as_deref()).await;

    // If the agent was deleted mid-build, the cascade will have deleted this build_jobs row —
    // the UPDATE above is a no-op (0 rows affected) and that's fine.

    Ok(true)
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
