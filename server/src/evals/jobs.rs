//! The eval job queue (`eval_jobs`) and its worker.
//!
//! Any server with evals enabled claims jobs (`FOR UPDATE SKIP LOCKED`), at most
//! `EVAL_WORKER_CONCURRENCY` at once. A running job's heartbeat is refreshed every
//! [`HEARTBEAT`]; a job whose heartbeat is older than [`STALE_AFTER`] (its server died) is put
//! back and claimed again, up to its `max_tries`, after which it is given up. A step that fails
//! is retried after [`RETRY_AFTER`] until it is out of tries.
//!
//! The heartbeat runs as its own task, so a step busy on the CPU cannot starve it. Every write a
//! worker makes to its job names the claim it holds (`tries`, raised by each claim): once the job
//! is taken over, the old copy's writes match nothing, and its heartbeat stops the step.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sqlx::PgExecutor;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::state::AppState;

const HEARTBEAT: Duration = Duration::from_secs(30);
const STALE_AFTER_SECS: i64 = 180;
const RETRY_AFTER_SECS: i64 = 30;
const IDLE_POLL: Duration = Duration::from_secs(5);
const RECOVER_EVERY: Duration = Duration::from_secs(60);

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Harvest,
    TaskRewrite,
    TaskBuild,
    TaskScreen,
    RunPreflight,
    RunAttempt,
    RunFinalize,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Harvest => "harvest",
            JobKind::TaskRewrite => "task_rewrite",
            JobKind::TaskBuild => "task_build",
            JobKind::TaskScreen => "task_screen",
            JobKind::RunPreflight => "run_preflight",
            JobKind::RunAttempt => "run_attempt",
            JobKind::RunFinalize => "run_finalize",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "harvest" => JobKind::Harvest,
            "task_rewrite" => JobKind::TaskRewrite,
            "task_build" => JobKind::TaskBuild,
            "task_screen" => JobKind::TaskScreen,
            "run_preflight" => JobKind::RunPreflight,
            "run_attempt" => JobKind::RunAttempt,
            "run_finalize" => JobKind::RunFinalize,
            _ => return None,
        })
    }
}

/// A job to enqueue; only the ids its kind uses are set.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: JobKind,
    pub user_id: Uuid,
    pub dataset_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
    pub model_id: Option<Uuid>,
}

impl NewJob {
    pub fn new(kind: JobKind, user_id: Uuid) -> Self {
        Self {
            kind,
            user_id,
            dataset_id: None,
            task_id: None,
            run_id: None,
            attempt_id: None,
            model_id: None,
        }
    }
}

/// A claimed job.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    #[sqlx(rename = "kind")]
    kind_text: String,
    pub user_id: Uuid,
    pub dataset_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub attempt_id: Option<Uuid>,
    pub model_id: Option<Uuid>,
    pub tries: i32,
    pub max_tries: i32,
}

impl Job {
    pub fn kind(&self) -> Option<JobKind> {
        JobKind::parse(&self.kind_text)
    }
}

/// Adds a job; `run_finalize` is added at most once per run.
pub async fn enqueue(db: impl PgExecutor<'_>, job: &NewJob) -> sqlx::Result<()> {
    sqlx::query(
        r#"INSERT INTO eval_jobs (kind, user_id, dataset_id, task_id, run_id, attempt_id, model_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(job.kind.as_str())
    .bind(job.user_id)
    .bind(job.dataset_id)
    .bind(job.task_id)
    .bind(job.run_id)
    .bind(job.attempt_id)
    .bind(job.model_id)
    .execute(db)
    .await?;
    Ok(())
}

async fn claim(db: &sqlx::PgPool) -> sqlx::Result<Option<Job>> {
    sqlx::query_as(
        r#"WITH next AS (
               SELECT id FROM eval_jobs
               WHERE status = 'pending' AND run_after <= now()
               ORDER BY run_after, id
               FOR UPDATE SKIP LOCKED
               LIMIT 1
           )
           UPDATE eval_jobs j
              SET status = 'running', tries = j.tries + 1, heartbeat_at = now()
             FROM next
            WHERE j.id = next.id
        RETURNING j.id, j.kind, j.user_id, j.dataset_id, j.task_id, j.run_id, j.attempt_id,
                  j.model_id, j.tries, j.max_tries"#,
    )
    .fetch_optional(db)
    .await
}

/// Refreshes `job`'s heartbeat; false once its claim is no longer this worker's. A database
/// error reads as still held: the next beat tries again.
async fn heartbeat(db: &sqlx::PgPool, job: &Job) -> bool {
    sqlx::query(
        "UPDATE eval_jobs SET heartbeat_at = now() \
         WHERE id = $1 AND tries = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(job.tries)
    .execute(db)
    .await
    .map_or(true, |r| r.rows_affected() == 1)
}

/// Beats every [`HEARTBEAT`] until `job` is taken over, then says so through `lost`.
async fn keep_alive(db: sqlx::PgPool, job: Job, lost: tokio::sync::oneshot::Sender<()>) {
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;
    loop {
        beat.tick().await;
        if !heartbeat(&db, &job).await {
            let _ = lost.send(());
            return;
        }
    }
}

/// Ends `job` as `status`; false when its claim is no longer this worker's (nothing changed).
async fn mark(
    db: &sqlx::PgPool,
    job: &Job,
    status: &str,
    error: Option<&str>,
) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "UPDATE eval_jobs SET status = $3, error = $4, finished_at = now() \
         WHERE id = $1 AND tries = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(job.tries)
    .bind(status)
    .bind(error)
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

async fn retry_later(db: &sqlx::PgPool, job: &Job, error: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE eval_jobs SET status = 'pending', error = $3, \
         run_after = now() + make_interval(secs => $4) \
         WHERE id = $1 AND tries = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(job.tries)
    .bind(error)
    .bind(RETRY_AFTER_SECS as f64)
    .execute(db)
    .await?;
    Ok(())
}

/// Puts jobs whose server stopped heartbeating back in the queue.
async fn recover(db: &sqlx::PgPool) -> sqlx::Result<u64> {
    Ok(sqlx::query(
        "UPDATE eval_jobs SET status = 'pending', run_after = now() \
         WHERE status = 'running' AND heartbeat_at < now() - make_interval(secs => $1)",
    )
    .bind(STALE_AFTER_SECS as f64)
    .execute(db)
    .await?
    .rows_affected())
}

/// Starts the eval worker and its recovery sweep (once per process).
pub fn spawn_worker(state: AppState) {
    tokio::spawn(recover_forever(state.db.clone(), state.eval_jobs.clone()));
    tokio::spawn(run(state));
}

/// The recovery sweep, on its own timer: inside the claim loop it would wait for a free worker
/// slot, so while every slot runs a long job, jobs of a server that died would sit `running`.
async fn recover_forever(db: sqlx::PgPool, jobs: Arc<tokio::sync::Notify>) {
    let mut every = tokio::time::interval(RECOVER_EVERY);
    loop {
        every.tick().await;
        match recover(&db).await {
            Ok(0) => {}
            Ok(n) => {
                tracing::warn!(jobs = n, "evals: took over jobs whose server stopped");
                jobs.notify_one();
            }
            Err(e) => tracing::error!(error = %e, "evals: job recovery failed"),
        }
    }
}

async fn run(state: AppState) {
    let permits = Arc::new(Semaphore::new(state.config.evals.worker_concurrency));
    loop {
        let Ok(permit) = permits.clone().acquire_owned().await else {
            return;
        };
        match claim(&state.db).await {
            Ok(Some(job)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    process(&state, job).await;
                    drop(permit);
                });
            }
            Ok(None) => {
                drop(permit);
                let _ = tokio::time::timeout(IDLE_POLL, state.eval_jobs.notified()).await;
            }
            Err(e) => {
                drop(permit);
                tracing::error!(error = %e, "evals: claiming a job failed");
                tokio::time::sleep(IDLE_POLL).await;
            }
        }
    }
}

async fn process(state: &AppState, job: Job) {
    if job.tries > job.max_tries {
        give_up(state, &job, "the server running it stopped too many times").await;
        return;
    }
    let (lost_tx, lost) = tokio::sync::oneshot::channel();
    let beat = tokio::spawn(keep_alive(state.db.clone(), job.clone(), lost_tx));
    let result = tokio::select! {
        result = super::steps::run(state, &job) => Some(result),
        _ = lost => None,
    };
    beat.abort();
    let Some(result) = result else {
        tracing::warn!(job = job.id, kind = %job.kind_text, "evals: a job was taken over; this copy stopped");
        return;
    };
    match result {
        Ok(()) => match mark(&state.db, &job, "done", None).await {
            Ok(true) => super::steps::after(state, &job).await,
            Ok(false) => {}
            Err(e) => {
                tracing::error!(job = job.id, error = %e, "evals: could not mark a job done");
                super::steps::after(state, &job).await;
            }
        },
        Err(e) => {
            let error = format!("{e:#}");
            tracing::warn!(job = job.id, kind = %job.kind_text, %error, "evals: a job failed");
            if job.tries < job.max_tries {
                if let Err(e) = retry_later(&state.db, &job, &error).await {
                    tracing::error!(job = job.id, error = %e, "evals: could not requeue a job");
                }
            } else {
                give_up(state, &job, &error).await;
            }
        }
    }
}

async fn give_up(state: &AppState, job: &Job, error: &str) {
    match mark(&state.db, job, "failed", Some(error)).await {
        Ok(false) => return,
        Ok(true) => {}
        Err(e) => tracing::error!(job = job.id, error = %e, "evals: could not mark a job failed"),
    }
    super::steps::gave_up(state, job, error).await;
    super::steps::after(state, job).await;
}

/// Wakes this server's worker; other servers find the job on their next poll.
pub fn notify(state: &AppState) {
    state.eval_jobs.notify_one();
}

/// The jobs of a dataset or run still to do.
pub async fn open_count(
    db: &sqlx::PgPool,
    dataset_id: Option<Uuid>,
    run_id: Option<Uuid>,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM eval_jobs WHERE status IN ('pending', 'running') \
         AND ($1::uuid IS NULL OR dataset_id = $1) AND ($2::uuid IS NULL OR run_id = $2)",
    )
    .bind(dataset_id)
    .bind(run_id)
    .fetch_one(db)
    .await?)
}
