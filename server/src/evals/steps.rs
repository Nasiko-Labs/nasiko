//! What each eval job does, what happens when one is given up, and what follows one.

use anyhow::{Context, Result, bail};
use nasiko_evals::TaskStatus;

use super::jobs::{Job, JobKind, NewJob, enqueue, notify};
use super::{attempt, build, harvest, review, store};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &Job) -> Result<()> {
    match job.kind() {
        Some(JobKind::Harvest) => harvest::run(state, job).await,
        Some(JobKind::TaskRewrite) => review::rewrite(state, job).await,
        Some(JobKind::TaskBuild) => build::run(state, job).await,
        Some(JobKind::TaskScreen) => review::screen(state, job).await,
        Some(JobKind::RunPreflight) => attempt::preflight(state, job).await,
        Some(JobKind::RunAttempt) => attempt::run(state, job).await,
        Some(JobKind::RunFinalize) => attempt::finalize(state, job).await,
        None => bail!("unknown eval job kind"),
    }
}

/// Records that a job will not be done: its dataset, task, attempt or run says why.
pub async fn gave_up(state: &AppState, job: &Job, error: &str) {
    let db = &state.db;
    let result: Result<()> = async {
        match job.kind() {
            Some(JobKind::Harvest) => {
                store::set_dataset_status(db, need(job.dataset_id)?, "failed", Some(error)).await
            }
            Some(JobKind::TaskRewrite | JobKind::TaskBuild | JobKind::TaskScreen) => {
                let task = store::task(db, need(job.task_id)?)
                    .await?
                    .context("task gone")?;
                let mut instance = task.instance.0;
                instance.x_nasiko.status = TaskStatus::Rejected;
                instance.x_nasiko.reject_reason = Some(format!("error: {error}"));
                store::save_task(db, task.id, &instance, "done").await
            }
            Some(JobKind::RunPreflight) => {
                store::fail_open_attempts(db, need(job.run_id)?, job.model_id, error).await
            }
            Some(JobKind::RunAttempt) => {
                store::fail_attempt(db, need(job.attempt_id)?, error).await
            }
            Some(JobKind::RunFinalize) => {
                store::set_run_status(db, need(job.run_id)?, "failed", Some(error)).await
            }
            None => Ok(()),
        }
    }
    .await;
    if let Err(e) = result {
        tracing::error!(job = job.id, error = %e, "evals: recording a given-up job failed");
    }
}

/// What follows a job that ended either way: a dataset with nothing left to do is ready, and a
/// run whose attempts all ended is finalized.
pub async fn after(state: &AppState, job: &Job) {
    let result: Result<()> = async {
        match job.kind() {
            Some(
                JobKind::Harvest | JobKind::TaskRewrite | JobKind::TaskBuild | JobKind::TaskScreen,
            ) => {
                if let Some(id) = job.dataset_id {
                    store::settle_dataset(&state.db, id).await?;
                }
            }
            Some(JobKind::RunPreflight | JobKind::RunAttempt) => {
                let run_id = need(job.run_id)?;
                if store::open_attempts(&state.db, run_id).await? == 0 {
                    let mut finalize = NewJob::new(JobKind::RunFinalize, job.user_id);
                    finalize.run_id = Some(run_id);
                    enqueue(&state.db, &finalize).await?;
                    notify(state);
                }
            }
            _ => {}
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::error!(job = job.id, error = %e, "evals: follow-up of a job failed");
    }
}

/// The id a job of this kind always carries.
pub fn need<T>(id: Option<T>) -> Result<T> {
    id.context("the job is missing an id its kind needs")
}

/// Queues the next step of a task.
pub async fn next_task_job(state: &AppState, job: &Job, kind: JobKind) -> Result<()> {
    let mut next = NewJob::new(kind, job.user_id);
    next.dataset_id = job.dataset_id;
    next.task_id = job.task_id;
    enqueue(&state.db, &next).await?;
    notify(state);
    Ok(())
}
