//! Reading and writing the `review_*` tables (migration 0120).

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::api::{Metrics, ModelFinding, ReferenceFinding};
use super::grader::GradeResult;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TaskRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_title: String,
    pub base_sha: String,
    pub head_sha: String,
    pub diff: String,
    pub files: Json<std::collections::BTreeMap<String, String>>,
    pub reference_findings: Json<Vec<ReferenceFinding>>,
    pub graded: bool,
    pub created_at: DateTime<Utc>,
}

const TASK_COLS: &str = "id, user_id, owner, repo, pr_number, pr_title, base_sha, head_sha, \
    diff, files, reference_findings, graded, created_at";

pub struct NewTask<'a> {
    pub user_id: Uuid,
    pub owner: &'a str,
    pub repo: &'a str,
    pub pr_number: i64,
    pub pr_title: &'a str,
    pub base_sha: &'a str,
    pub head_sha: &'a str,
    pub diff: &'a str,
    pub files: &'a std::collections::BTreeMap<String, String>,
}

pub async fn insert_task(db: &PgPool, new: &NewTask<'_>) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO review_tasks \
         (user_id, owner, repo, pr_number, pr_title, base_sha, head_sha, diff, files) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         ON CONFLICT (user_id, owner, repo, pr_number) DO UPDATE SET \
            pr_title = EXCLUDED.pr_title, base_sha = EXCLUDED.base_sha, \
            head_sha = EXCLUDED.head_sha, diff = EXCLUDED.diff, files = EXCLUDED.files \
         RETURNING id",
    )
    .bind(new.user_id)
    .bind(new.owner)
    .bind(new.repo)
    .bind(new.pr_number)
    .bind(new.pr_title)
    .bind(new.base_sha)
    .bind(new.head_sha)
    .bind(new.diff)
    .bind(Json(new.files))
    .fetch_one(db)
    .await?)
}

pub async fn users_task(db: &PgPool, id: Uuid, user_id: Uuid) -> Result<Option<TaskRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TASK_COLS} FROM review_tasks WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?
    .filter(|t: &TaskRow| t.user_id == user_id))
}

pub async fn users_tasks(db: &PgPool, user_id: Uuid, limit: i64) -> Result<Vec<TaskRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {TASK_COLS} FROM review_tasks WHERE user_id = $1 ORDER BY created_at DESC \
         LIMIT $2"
    ))
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn set_reference_findings(
    db: &PgPool,
    id: Uuid,
    findings: &[ReferenceFinding],
) -> Result<()> {
    sqlx::query(
        "UPDATE review_tasks SET reference_findings = $2, graded = (jsonb_array_length($2) > 0) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(Json(findings))
    .execute(db)
    .await?;
    Ok(())
}

/// One of the caller's `llm_configs`, snapshotted into a run's `models`.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ModelConfig {
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
}

pub async fn users_configs(db: &PgPool, user_id: Uuid, ids: &[Uuid]) -> Result<Vec<ModelConfig>> {
    Ok(sqlx::query_as(
        "SELECT id, name, provider, model FROM llm_configs \
         WHERE created_by = $1 AND deleted_at IS NULL AND id = ANY($2) ORDER BY name",
    )
    .bind(user_id)
    .bind(ids)
    .fetch_all(db)
    .await?)
}

pub async fn insert_run(
    db: &PgPool,
    user_id: Uuid,
    task_id: Uuid,
    models: &[ModelConfig],
) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO review_runs (user_id, task_id, models) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(task_id)
    .bind(Json(models))
    .fetch_one(db)
    .await?)
}

pub async fn insert_pending_attempt(
    db: impl PgExecutor<'_>,
    run_id: Uuid,
    task_id: Uuid,
    model_id: Uuid,
    tag: &str,
) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO review_attempts (run_id, task_id, model_id, tag, started_at) \
         VALUES ($1, $2, $3, $4, now()) RETURNING id",
    )
    .bind(run_id)
    .bind(task_id)
    .bind(model_id)
    .bind(tag)
    .fetch_one(db)
    .await?)
}

/// What the router billed one attempt's tag, read back from its own `trace_usage` row(s) — never
/// estimated client-side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttemptUsage {
    pub calls: i64,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub worst_price_source: String,
}

#[derive(sqlx::FromRow)]
struct UsageRow {
    calls: i64,
    cost_usd: Option<f64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    price_sources: Vec<String>,
}

pub async fn attempt_usage(db: &PgPool, user_id: Uuid, tag: &str) -> Result<AttemptUsage> {
    let row: UsageRow = sqlx::query_as(
        "SELECT count(*) AS calls,
                sum(router_cost_usd)::float8 AS cost_usd,
                sum(router_input_tokens)::bigint AS input_tokens,
                sum(router_output_tokens)::bigint AS output_tokens,
                COALESCE(array_agg(DISTINCT COALESCE(metadata->'pricing'->>'source', 'Default')),
                    '{}') AS price_sources
         FROM trace_usage WHERE user_id = $1 AND eval_attempt = $2",
    )
    .bind(user_id)
    .bind(tag)
    .fetch_one(db)
    .await?;
    Ok(AttemptUsage {
        calls: row.calls,
        cost_usd: row.cost_usd.unwrap_or(0.0),
        input_tokens: row.input_tokens.unwrap_or(0),
        output_tokens: row.output_tokens.unwrap_or(0),
        worst_price_source: super::grader::worst_price_source(&row.price_sources),
    })
}

pub struct AttemptOutcome<'a> {
    pub raw_output: &'a str,
    pub findings: Option<&'a [ModelFinding]>,
    pub grade: Option<&'a GradeResult>,
    pub latency_ms: i64,
    pub usage: &'a AttemptUsage,
}

pub async fn finish_attempt(db: &PgPool, id: Uuid, outcome: &AttemptOutcome<'_>) -> Result<()> {
    let findings = outcome.findings.map(Json);
    let metrics = outcome.grade.map(Metrics::from).map(Json);
    sqlx::query(
        "UPDATE review_attempts SET status = 'done', raw_output = $2, findings = $3, \
         metrics = $4, latency_ms = $5, usage = $6, finished_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(outcome.raw_output)
    .bind(findings)
    .bind(metrics)
    .bind(outcome.latency_ms)
    .bind(Json(outcome.usage))
    .execute(db)
    .await?;
    Ok(())
}

pub async fn fail_attempt(db: &PgPool, id: Uuid, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE review_attempts SET status = 'failed', error = $2, finished_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AttemptRow {
    pub id: Uuid,
    pub model_id: Uuid,
    pub status: String,
    pub raw_output: Option<String>,
    pub findings: Option<Json<Vec<ModelFinding>>>,
    pub metrics: Option<Json<Metrics>>,
    pub latency_ms: Option<i64>,
    pub usage: Option<Json<AttemptUsage>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub task_id: Uuid,
    pub models: Json<Vec<ModelConfig>>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// `review_runs.error` is not read here: nothing in this module writes it today (a run that
/// finds no successful attempt is reported via its attempts' own `error`, settled to `status =
/// 'failed'` by `settle_run`), so selecting it would only ever surface `NULL`.
pub async fn users_run(db: &PgPool, id: Uuid, user_id: Uuid) -> Result<Option<RunRow>> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, task_id, models, status, created_at, finished_at \
         FROM review_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .filter(|r: &RunRow| r.user_id == user_id))
}

/// One attempt joined with the model identity snapshotted on its run (`review_runs.models`),
/// across every task — the "Eval Universe" view's flat point list. `review_attempts` itself only
/// stores `model_id`; the human-readable name/provider/model string live only in the run's
/// `models` JSONB snapshot, hence the lateral join rather than a second round-trip per attempt.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UniverseAttemptRow {
    pub attempt_id: Uuid,
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub model_id: Uuid,
    pub model_name: String,
    pub provider: String,
    pub model: Option<String>,
    pub status: String,
    pub metrics: Option<Json<Metrics>>,
    pub latency_ms: Option<i64>,
    pub usage: Option<Json<AttemptUsage>>,
    pub error: Option<String>,
}

pub async fn users_universe_attempts(
    db: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<UniverseAttemptRow>> {
    Ok(sqlx::query_as(
        "SELECT a.id AS attempt_id, a.run_id, a.task_id, a.model_id, a.status, a.metrics, \
                a.latency_ms, a.usage, a.error, \
                m.value->>'name' AS model_name, m.value->>'provider' AS provider, \
                m.value->>'model' AS model \
         FROM review_attempts a \
         JOIN review_runs r ON r.id = a.run_id \
         JOIN LATERAL jsonb_array_elements(r.models) AS m(value) \
            ON (m.value->>'id')::uuid = a.model_id \
         WHERE r.user_id = $1 \
         ORDER BY a.started_at DESC NULLS LAST \
         LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn run_attempts(db: &PgPool, run_id: Uuid) -> Result<Vec<AttemptRow>> {
    Ok(sqlx::query_as(
        "SELECT id, model_id, status, raw_output, findings, metrics, latency_ms, usage, error \
         FROM review_attempts WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_all(db)
    .await?)
}

/// Marks the run `done` once every attempt has left `pending`/`running` and at least one
/// finished; `failed` only when none did. `finished_at` is set exactly once, the moment the run
/// actually leaves `running` — not on every attempt that finishes while others are still going.
pub async fn settle_run(db: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE review_runs SET
            status = sub.new_status,
            finished_at = CASE WHEN sub.new_status <> 'running' THEN now() ELSE finished_at END
         FROM (
            SELECT CASE
                WHEN EXISTS (SELECT 1 FROM review_attempts
                             WHERE run_id = $1 AND status IN ('pending', 'running')) THEN 'running'
                WHEN EXISTS (SELECT 1 FROM review_attempts WHERE run_id = $1 AND status = 'done')
                    THEN 'done'
                ELSE 'failed'
            END AS new_status
         ) sub
         WHERE review_runs.id = $1",
    )
    .bind(run_id)
    .execute(db)
    .await?;
    Ok(())
}
