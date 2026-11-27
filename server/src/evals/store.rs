//! Reading and writing the eval tables (migration 0117).

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use nasiko_evals::prompt::ScreenVerdict;
use nasiko_evals::report::AttemptRecord;
use nasiko_evals::spec::EnvSpec;
use nasiko_evals::{TaskInstance, TaskSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dataset {
    pub id: Uuid,
    pub user_id: Uuid,
    pub harness: String,
    pub status: String,
    pub session_ids: Vec<String>,
    pub rewriter_config_id: Option<Uuid>,
    pub reviewer_config_ids: Vec<Uuid>,
    pub gate_runs: i32,
    pub outcomes: Json<BTreeMap<String, String>>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

const DATASET_COLS: &str = "id, user_id, harness, status, session_ids, rewriter_config_id, \
     reviewer_config_ids, gate_runs, outcomes, error, created_at";

pub async fn dataset(db: &PgPool, id: Uuid) -> Result<Option<Dataset>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DATASET_COLS} FROM eval_datasets WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// The dataset when it is `user_id`'s; another user's reads as missing.
pub async fn users_dataset(db: &PgPool, id: Uuid, user_id: Uuid) -> Result<Option<Dataset>> {
    Ok(dataset(db, id).await?.filter(|d| d.user_id == user_id))
}

pub async fn users_datasets(db: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<Dataset>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {DATASET_COLS} FROM eval_datasets WHERE user_id = $1 \
         ORDER BY created_at DESC LIMIT $2"
    ))
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub struct NewDataset<'a> {
    pub user_id: Uuid,
    pub harness: &'a str,
    pub session_ids: &'a [String],
    pub rewriter_config_id: Option<Uuid>,
    pub reviewer_config_ids: &'a [Uuid],
    pub gate_runs: i32,
}

pub async fn insert_dataset(db: impl PgExecutor<'_>, new: &NewDataset<'_>) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO eval_datasets \
         (user_id, harness, session_ids, rewriter_config_id, reviewer_config_ids, gate_runs) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(new.user_id)
    .bind(new.harness)
    .bind(new.session_ids)
    .bind(new.rewriter_config_id)
    .bind(new.reviewer_config_ids)
    .bind(new.gate_runs)
    .fetch_one(db)
    .await?)
}

pub async fn set_dataset_status(
    db: &PgPool,
    id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE eval_datasets SET status = $2, error = COALESCE($3, error), updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn set_outcomes(
    db: &PgPool,
    id: Uuid,
    outcomes: &BTreeMap<String, String>,
) -> Result<()> {
    sqlx::query("UPDATE eval_datasets SET outcomes = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(Json(outcomes))
        .execute(db)
        .await?;
    Ok(())
}

/// A preparing dataset with nothing left to do is ready.
pub async fn settle_dataset(db: &PgPool, id: Uuid) -> Result<()> {
    if super::jobs::open_count(db, Some(id), None).await? == 0 {
        sqlx::query(
            "UPDATE eval_datasets SET status = 'ready', updated_at = now() \
             WHERE id = $1 AND status = 'preparing'",
        )
        .bind(id)
        .execute(db)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskRow {
    pub id: Uuid,
    pub dataset_id: Uuid,
    pub repo: String,
    pub source_session_id: String,
    pub status: String,
    pub stage: String,
    pub instance: Json<TaskInstance>,
    pub source: Json<TaskSource>,
    pub link: Json<Value>,
    pub gate: Option<Json<Value>>,
    pub reviews: Json<Vec<ScreenVerdict>>,
}

const TASK_COLS: &str = "id, dataset_id, repo, source_session_id, status, stage, instance, source, \
     link, gate, reviews";

pub async fn task(db: &PgPool, id: Uuid) -> Result<Option<TaskRow>> {
    Ok(
        sqlx::query_as(&format!("SELECT {TASK_COLS} FROM eval_tasks WHERE id = $1"))
            .bind(id)
            .fetch_optional(db)
            .await?,
    )
}

pub async fn tasks_of(db: &PgPool, dataset_id: Uuid) -> Result<Vec<TaskRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TASK_COLS} FROM eval_tasks WHERE dataset_id = $1 ORDER BY created_at, id"
    ))
    .bind(dataset_id)
    .fetch_all(db)
    .await?)
}

pub struct NewTask<'a> {
    pub dataset_id: Uuid,
    pub repo: &'a str,
    pub source_session_id: &'a str,
    pub stage: &'a str,
    pub instance: &'a TaskInstance,
    pub source: &'a TaskSource,
    pub link: &'a Value,
}

pub async fn insert_task(db: impl PgExecutor<'_>, new: &NewTask<'_>) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO eval_tasks (dataset_id, repo, source_session_id, status, stage, instance, \
         source, link) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(new.dataset_id)
    .bind(new.repo)
    .bind(new.source_session_id)
    .bind(status_name(new.instance))
    .bind(new.stage)
    .bind(Json(new.instance))
    .bind(Json(new.source))
    .bind(Json(new.link))
    .fetch_one(db)
    .await?)
}

/// Saves a task's instance (its status follows the instance's) and next stage.
pub async fn save_task(
    db: impl PgExecutor<'_>,
    id: Uuid,
    instance: &TaskInstance,
    stage: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE eval_tasks SET instance = $2, status = $3, stage = $4, updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(Json(instance))
    .bind(status_name(instance))
    .bind(stage)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn save_gate(db: &PgPool, id: Uuid, gate: &Value) -> Result<()> {
    sqlx::query("UPDATE eval_tasks SET gate = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(Json(gate))
        .execute(db)
        .await?;
    Ok(())
}

pub async fn save_reviews(db: &PgPool, id: Uuid, reviews: &[ScreenVerdict]) -> Result<()> {
    sqlx::query("UPDATE eval_tasks SET reviews = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(Json(reviews))
        .execute(db)
        .await?;
    Ok(())
}

fn status_name(instance: &TaskInstance) -> String {
    serde_json::to_value(instance.x_nasiko.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "candidate".to_string())
}

pub async fn spec(db: &PgPool, user_id: Uuid, repo: &str) -> Result<Option<EnvSpec>> {
    let row: Option<Json<EnvSpec>> =
        sqlx::query_scalar("SELECT spec FROM eval_specs WHERE user_id = $1 AND repo = $2")
            .bind(user_id)
            .bind(repo)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|j| j.0))
}

pub async fn save_spec(db: &PgPool, user_id: Uuid, repo: &str, spec: &EnvSpec) -> Result<()> {
    sqlx::query(
        "INSERT INTO eval_specs (user_id, repo, spec) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, repo) DO UPDATE SET spec = EXCLUDED.spec, updated_at = now()",
    )
    .bind(user_id)
    .bind(repo)
    .bind(Json(spec))
    .execute(db)
    .await?;
    Ok(())
}

/// One model a run tests: one of the caller's LLM configs, as it was when the run started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RunModel {
    /// The `llm_configs.id`.
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    /// It also rewrote or screened the dataset's tasks, so its scores are optimistic.
    pub builder: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub dataset_id: Uuid,
    pub models: Json<Vec<RunModel>>,
    pub attempts: i32,
    pub status: String,
    pub report: Option<Json<Value>>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

const RUN_COLS: &str = "id, user_id, dataset_id, models, attempts, status, report, error, \
     created_at, started_at, finished_at";

pub async fn run(db: &PgPool, id: Uuid) -> Result<Option<RunRow>> {
    Ok(
        sqlx::query_as(&format!("SELECT {RUN_COLS} FROM eval_runs WHERE id = $1"))
            .bind(id)
            .fetch_optional(db)
            .await?,
    )
}

pub async fn users_run(db: &PgPool, id: Uuid, user_id: Uuid) -> Result<Option<RunRow>> {
    Ok(run(db, id).await?.filter(|r| r.user_id == user_id))
}

pub async fn users_runs(
    db: &PgPool,
    user_id: Uuid,
    dataset_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RunRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {RUN_COLS} FROM eval_runs WHERE user_id = $1 \
         AND ($2::uuid IS NULL OR dataset_id = $2) ORDER BY created_at DESC LIMIT $3"
    ))
    .bind(user_id)
    .bind(dataset_id)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn set_run_status(
    db: &PgPool,
    id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE eval_runs SET status = $2, error = COALESCE($3, error), \
         started_at = CASE WHEN $2 = 'running' THEN COALESCE(started_at, now()) ELSE started_at END, \
         finished_at = CASE WHEN $2 IN ('done', 'failed', 'cancelled') THEN now() ELSE finished_at END \
         WHERE id = $1 AND status NOT IN ('done', 'failed', 'cancelled')",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn save_report(db: &PgPool, id: Uuid, report: &Value) -> Result<()> {
    sqlx::query(
        "UPDATE eval_runs SET report = $2, status = 'done', finished_at = now() \
         WHERE id = $1 AND status <> 'cancelled'",
    )
    .bind(id)
    .bind(Json(report))
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttemptRow {
    pub id: Uuid,
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub model_id: Uuid,
    pub attempt_no: i32,
    pub tag: String,
    pub status: String,
    pub instance: Json<TaskInstance>,
    pub record: Option<Json<AttemptRecord>>,
    pub patch: Option<String>,
    pub artifacts: Json<BTreeMap<String, String>>,
    pub error: Option<String>,
}

const ATTEMPT_COLS: &str = "id, run_id, task_id, model_id, attempt_no, tag, status, instance, \
     record, patch, artifacts, error";

pub async fn attempt(db: &PgPool, id: Uuid) -> Result<Option<AttemptRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ATTEMPT_COLS} FROM eval_attempts WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

pub async fn attempts_of(db: &PgPool, run_id: Uuid) -> Result<Vec<AttemptRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ATTEMPT_COLS} FROM eval_attempts WHERE run_id = $1 \
         ORDER BY task_id, model_id, attempt_no"
    ))
    .bind(run_id)
    .fetch_all(db)
    .await?)
}

pub async fn insert_attempt(
    db: impl PgExecutor<'_>,
    run_id: Uuid,
    task_id: Uuid,
    model_id: Uuid,
    attempt_no: i32,
    instance: &TaskInstance,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO eval_attempts (id, run_id, task_id, model_id, attempt_no, tag, instance) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(run_id)
    .bind(task_id)
    .bind(model_id)
    .bind(attempt_no)
    .bind(attempt_tag(run_id, id))
    .bind(Json(instance))
    .execute(db)
    .await?;
    Ok(id)
}

/// The router tag of an attempt: `<run>:<attempt>`, so `GET /api/evals/usage?run=<run>` (and
/// the run's own report) finds every attempt by the run's prefix.
pub fn attempt_tag(run_id: Uuid, attempt_id: Uuid) -> String {
    format!("{run_id}:{attempt_id}")
}

pub async fn set_attempt_running(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE eval_attempts SET status = 'running', started_at = COALESCE(started_at, now()) \
         WHERE id = $1 AND status IN ('pending', 'running')",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn finish_attempt(
    db: &PgPool,
    id: Uuid,
    record: &AttemptRecord,
    patch: &str,
    artifacts: &BTreeMap<String, String>,
) -> Result<()> {
    sqlx::query(
        "UPDATE eval_attempts SET status = 'done', record = $2, patch = $3, artifacts = $4, \
         finished_at = now() WHERE id = $1 AND status <> 'cancelled'",
    )
    .bind(id)
    .bind(Json(record))
    .bind(patch)
    .bind(Json(artifacts))
    .execute(db)
    .await?;
    Ok(())
}

pub async fn save_attempt_record(db: &PgPool, id: Uuid, record: &AttemptRecord) -> Result<()> {
    sqlx::query("UPDATE eval_attempts SET record = $2 WHERE id = $1")
        .bind(id)
        .bind(Json(record))
        .execute(db)
        .await?;
    Ok(())
}

/// Fails the attempts that have not ended; `model_id` narrows it to one model's.
pub async fn fail_open_attempts(
    db: &PgPool,
    run_id: Uuid,
    model_id: Option<Uuid>,
    error: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE eval_attempts SET status = 'failed', error = $3, finished_at = now() \
         WHERE run_id = $1 AND ($2::uuid IS NULL OR model_id = $2) \
           AND status IN ('pending', 'running')",
    )
    .bind(run_id)
    .bind(model_id)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn fail_attempt(db: &PgPool, id: Uuid, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE eval_attempts SET status = 'failed', error = $2, finished_at = now() \
         WHERE id = $1 AND status IN ('pending', 'running')",
    )
    .bind(id)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn open_attempts(db: &PgPool, run_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM eval_attempts WHERE run_id = $1 AND status IN ('pending', 'running')",
    )
    .bind(run_id)
    .fetch_one(db)
    .await?)
}
