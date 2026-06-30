use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::AppState;

use super::upload::{BuildJobPayload, execute_upload_and_deploy};

#[derive(Debug, sqlx::FromRow)]
struct BuildJob {
    id: Uuid,
    agent_id: Uuid,
    payload: serde_json::Value,
}

/// Main build worker loop. Spawned once at server startup.
///
/// Receives notifications on `notify` when a new job is queued; also polls
/// every 5 seconds as a fallback in case a notification is lost.
///
/// After each successful claim, drains the queue immediately before sleeping
/// to avoid a 5-second lag when multiple jobs arrive in a burst.
pub async fn run(state: AppState, mut notify: mpsc::Receiver<()>) {
    recover_stuck_jobs(&state.db).await;

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
        }
        // Drain: keep claiming jobs until the queue is empty.
        loop {
            match try_claim_and_run(&state).await {
                Ok(true) => {} // there may be more
                Ok(false) => {
                    tracing::debug!("build worker: queue empty");
                    break;
                }
                Err(e) => {
                    tracing::error!(%e, "build worker: claim/run error");
                    break;
                }
            }
        }
    }
}

/// On startup, reset any jobs that were left `in_progress` for more than 30 minutes
/// (likely from a crashed server instance).
async fn recover_stuck_jobs(db: &PgPool) {
    match sqlx::query(
        "UPDATE build_jobs SET status = 'pending', picked_at = NULL
         WHERE status = 'in_progress' AND picked_at < now() - INTERVAL '30 minutes'",
    )
    .execute(db)
    .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            tracing::warn!(count = result.rows_affected(), "build worker: reset stuck in_progress jobs on startup");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(%e, "build worker: startup recovery query failed");
        }
    }
}

/// Claim one pending job and execute it. Returns `Ok(true)` if a job was found
/// and run (caller should loop), `Ok(false)` if the queue is empty.
async fn try_claim_and_run(state: &AppState) -> anyhow::Result<bool> {
    let mut tx = state.db.begin().await?;

    let job = sqlx::query_as::<_, BuildJob>(
        "SELECT id, agent_id, payload
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

    let payload: BuildJobPayload = match serde_json::from_value(job.payload) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(job_id = %job.id, %e, "build worker: invalid payload, marking failed");
            mark_job(&state.db, job.id, "failed", Some(&e.to_string())).await;
            return Ok(true);
        }
    };

    tracing::info!(
        job_id = %job.id,
        agent_id = %job.agent_id,
        attempt = "from DB",
        name = %payload.name,
        "build worker: job claimed"
    );

    let start = std::time::Instant::now();
    execute_upload_and_deploy(
        state.runtime.clone(),
        state.db.clone(),
        payload.build_id,
        payload.agent_id,
        payload.owner_id,
        payload.upload_id,
        payload.name.clone(),
        std::path::PathBuf::from(&payload.zip_path),
        payload.image_tag,
        payload.ports,
        payload.env,
    )
    .await;

    // `execute_upload_and_deploy` writes agent_builds / upload_status itself.
    // We just need to finalize the build_job row.
    // Infer success/failure from the agent_builds status that was just written.
    let build_status: Option<String> = sqlx::query_scalar(
        "SELECT status::text FROM agent_builds WHERE id = $1",
    )
    .bind(payload.build_id)
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
