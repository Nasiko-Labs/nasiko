//! The `/api/evals/code-review/*` handlers: import one pull request, add validated reference
//! findings, review it with chosen LLM configs, and read back the graded (or ungraded-preview)
//! results. Every object is the caller's own; another user's id reads as `404`.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::ApiError;
use super::grader::{self, GradeResult};
use super::prompt;
use super::store::{self, AttemptUsage, ModelConfig, TaskRow};
use crate::auth::Claims;
use crate::evals::forge::{Forge, ForgeRefusal};
use crate::state::AppState;

const MAX_LIST: i64 = 100;
/// This slice reviews one task at a time; the UI and the report must say so plainly rather than
/// imply a statistically meaningful ranking (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`).
const SAMPLE_SIZE_WARNING: &str = "This run graded 1 task. Treat any difference between models as directional only, not a \
     statistically significant ranking.";

type ApiResult<T> = Result<T, ApiError>;

fn user(claims: &Claims) -> ApiResult<Uuid> {
    claims
        .user_uuid()
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "unauthorized", "not signed in"))
}

fn data(value: impl Serialize) -> Response {
    Json(json!({ "data": value })).into_response()
}

fn accepted(value: impl Serialize) -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "data": value }))).into_response()
}

fn not_found(what: &'static str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not_found", what)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal("code review eval request failed", e)
}

fn forge_error(refusal: ForgeRefusal) -> ApiError {
    match refusal {
        ForgeRefusal::GithubNotConnected => ApiError(
            StatusCode::BAD_REQUEST,
            "github_not_connected",
            "connect a GitHub account before importing a pull request",
        ),
        ForgeRefusal::RepoNotAccessible => ApiError(
            StatusCode::NOT_FOUND,
            "repo_not_accessible",
            "this GitHub account cannot see that repository",
        ),
    }
}

/// A GitHub owner or repo name: letters, digits, `-`, `_`, `.`, 1-100 bytes.
fn valid_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

// ── Findings and severity/category ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Bug,
    Security,
    Quality,
}

/// A human-validated piece of ground truth, added once per task before a graded run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReferenceFinding {
    pub file: String,
    pub line: i64,
    pub severity: Severity,
    pub category: Category,
    pub description: String,
}

/// One finding a model's reply reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ModelFinding {
    pub file: String,
    pub line: i64,
    pub severity: Severity,
    pub category: Category,
    pub description: String,
    pub confidence: f64,
}

/// A graded attempt's score; absent on an ungraded task's preview attempt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Metrics {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub precision: f64,
    pub recall: f64,
    pub severity_accuracy: Option<f64>,
}

// ── Tasks ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateTask {
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TaskView {
    pub id: Uuid,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_title: String,
    pub base_sha: String,
    pub head_sha: String,
    pub diff: String,
    pub files: BTreeMap<String, String>,
    pub reference_findings: Vec<ReferenceFinding>,
    pub graded: bool,
    pub created_at: DateTime<Utc>,
}

impl From<TaskRow> for TaskView {
    fn from(row: TaskRow) -> Self {
        TaskView {
            id: row.id,
            owner: row.owner,
            repo: row.repo,
            pr_number: row.pr_number,
            pr_title: row.pr_title,
            base_sha: row.base_sha,
            head_sha: row.head_sha,
            diff: row.diff,
            files: row.files.0,
            reference_findings: row.reference_findings.0,
            graded: row.graded,
            created_at: row.created_at,
        }
    }
}

pub async fn create_task(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateTask>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        if !valid_path_segment(&body.owner) || !valid_path_segment(&body.repo) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_repo",
                "owner and repo must be valid GitHub path segments",
            ));
        }
        if body.pr_number <= 0 {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_pr_number",
                "pr_number must be positive",
            ));
        }
        let repo_full = format!("{}/{}", body.owner, body.repo);
        let forge = Forge::open(&state, user_id, &repo_full)
            .await
            .map_err(internal)?
            .map_err(forge_error)?;
        let pr = forge
            .pull_request(body.pr_number as u64)
            .await
            .map_err(internal)?;
        let diff = forge
            .diff(&pr.base.sha, &pr.head.sha)
            .await
            .map_err(internal)?;
        let touched = touched_files(&diff);
        let files = forge
            .files_at(&pr.head.sha, &touched)
            .await
            .map_err(internal)?;
        let id = store::insert_task(
            &state.db,
            &store::NewTask {
                user_id,
                owner: &body.owner,
                repo: &body.repo,
                pr_number: body.pr_number,
                pr_title: &pr.title,
                base_sha: &pr.base.sha,
                head_sha: &pr.head.sha,
                diff: &diff,
                files: &files,
            },
        )
        .await
        .map_err(internal)?;
        let task = store::users_task(&state.db, id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("the imported task could not be read back"))?;
        Ok(data(TaskView::from(task)))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// Every `+++ b/<path>` target in a unified diff, deduplicated. A deleted file's `+++` line reads
/// `+++ /dev/null` (no `b/` prefix), so it never matches and is naturally skipped — there is
/// nothing left to show as "touched file context" for a file the PR removed.
fn touched_files(diff: &str) -> Vec<String> {
    let set: BTreeSet<String> = diff
        .lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .map(str::to_string)
        .collect();
    set.into_iter().collect()
}

pub async fn list_tasks(State(state): State<AppState>, claims: Claims) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let tasks = store::users_tasks(&state.db, user_id, MAX_LIST)
            .await
            .map_err(internal)?
            .into_iter()
            .map(TaskView::from)
            .collect::<Vec<_>>();
        Ok(data(tasks))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

pub async fn get_task(
    State(state): State<AppState>,
    claims: Claims,
    Path(task_id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let task = store::users_task(&state.db, task_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such task"))?;
        Ok(data(TaskView::from(task)))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddReferenceFindings {
    pub findings: Vec<ReferenceFinding>,
}

pub async fn add_reference_findings(
    State(state): State<AppState>,
    claims: Claims,
    Path(task_id): Path<Uuid>,
    Json(body): Json<AddReferenceFindings>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        store::users_task(&state.db, task_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such task"))?;
        store::set_reference_findings(&state.db, task_id, &body.findings)
            .await
            .map_err(internal)?;
        let task = store::users_task(&state.db, task_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("the task vanished while it was being graded"))?;
        Ok(data(TaskView::from(task)))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

// ── Runs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRun {
    pub model_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunCreated {
    pub id: Uuid,
    pub status: String,
}

pub async fn create_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(task_id): Path<Uuid>,
    Json(body): Json<CreateRun>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let task = store::users_task(&state.db, task_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such task"))?;
        let picked: BTreeSet<Uuid> = body.model_ids.iter().copied().collect();
        if picked.is_empty() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_model",
                "pick at least one model",
            ));
        }
        let configs = store::users_configs(&state.db, user_id, &body.model_ids)
            .await
            .map_err(internal)?;
        if configs.len() != picked.len() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_model",
                "a model must be one of your own LLM configs",
            ));
        }
        let run_id = store::insert_run(&state.db, user_id, task_id, &configs)
            .await
            .map_err(internal)?;
        for config in configs {
            let tag = format!("cr-{run_id}:{}", config.id);
            let attempt_id =
                store::insert_pending_attempt(&state.db, run_id, task_id, config.id, &tag)
                    .await
                    .map_err(internal)?;
            spawn_attempt(
                state.clone(),
                user_id,
                run_id,
                attempt_id,
                tag,
                config,
                task.clone(),
            );
        }
        Ok(accepted(RunCreated {
            id: run_id,
            status: "running".to_string(),
        }))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// Runs one attempt's single router call, grades it when the task is graded, and settles the run
/// once every attempt has left `pending`/`running`. Spawned rather than queued durably: a review
/// is one short completion, not an hours-long harness session, and a crash mid-run is an accepted
/// limitation at this scope (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`).
fn spawn_attempt(
    state: AppState,
    user_id: Uuid,
    run_id: Uuid,
    attempt_id: Uuid,
    tag: String,
    config: ModelConfig,
    task: TaskRow,
) {
    tokio::spawn(async move {
        let prompt = prompt::build(&task.diff, &task.files.0);
        let started = std::time::Instant::now();
        let reply = crate::evals::llm::complete(&state, user_id, config.id, &tag, &prompt).await;
        let latency_ms = started.elapsed().as_millis() as i64;
        let raw_output = match reply {
            Ok(text) => text,
            Err(e) => {
                log_err(store::fail_attempt(&state.db, attempt_id, &e.to_string()).await);
                log_err(store::settle_run(&state.db, run_id).await);
                return;
            }
        };
        let findings = match prompt::parse_findings(&raw_output) {
            Ok(findings) => findings,
            Err(e) => {
                let message = format!("the model's reply did not parse: {e}");
                log_err(store::fail_attempt(&state.db, attempt_id, &message).await);
                log_err(store::settle_run(&state.db, run_id).await);
                return;
            }
        };
        let grade: Option<GradeResult> = task
            .graded
            .then(|| grader::grade(&task.reference_findings.0, &findings));
        let usage = match store::attempt_usage(&state.db, user_id, &tag).await {
            Ok(usage) => usage,
            Err(e) => {
                tracing::error!(%e, "could not read the router's usage for a review attempt");
                AttemptUsage::default()
            }
        };
        let outcome = store::AttemptOutcome {
            raw_output: &raw_output,
            findings: Some(&findings),
            grade: grade.as_ref(),
            latency_ms,
            usage: &usage,
        };
        log_err(store::finish_attempt(&state.db, attempt_id, &outcome).await);
        log_err(store::settle_run(&state.db, run_id).await);
    });
}

fn log_err(result: anyhow::Result<()>) {
    if let Err(e) = result {
        tracing::error!(%e, "code review eval attempt bookkeeping failed");
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AttemptResult {
    /// The attempt's own id — not a model config id, so a client can tell two attempts on the
    /// same model apart if this task is ever re-run (e.g. a future retry or re-review).
    pub attempt_id: Option<Uuid>,
    pub model_id: Uuid,
    pub model_name: String,
    pub provider: String,
    pub model: Option<String>,
    pub status: String,
    /// The model's reply verbatim, kept for audit even when it failed to parse into `findings`.
    pub raw_output: Option<String>,
    pub findings: Vec<ModelFinding>,
    pub metrics: Option<Metrics>,
    pub latency_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub worst_price_source: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RunView {
    pub id: Uuid,
    pub task_id: Uuid,
    pub status: String,
    /// Whether the task has validated reference findings; when false every result's `metrics`
    /// is `None` and the caller must present this run as an ungraded preview, not a benchmark.
    pub graded: bool,
    pub sample_size_warning: String,
    pub results: Vec<AttemptResult>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub async fn get_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(run_id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let run = store::users_run(&state.db, run_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such run"))?;
        let task = store::users_task(&state.db, run.task_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such task"))?;
        let attempts = store::run_attempts(&state.db, run_id)
            .await
            .map_err(internal)?;
        let results = run
            .models
            .0
            .iter()
            .map(|config| {
                let attempt = attempts.iter().find(|a| a.model_id == config.id);
                AttemptResult {
                    attempt_id: attempt.map(|a| a.id),
                    model_id: config.id,
                    model_name: config.name.clone(),
                    provider: config.provider.clone(),
                    model: config.model.clone(),
                    status: attempt
                        .map(|a| a.status.clone())
                        .unwrap_or_else(|| "pending".to_string()),
                    raw_output: attempt.and_then(|a| a.raw_output.clone()),
                    findings: attempt
                        .and_then(|a| a.findings.as_ref())
                        .map(|f| f.0.clone())
                        .unwrap_or_default(),
                    metrics: attempt.and_then(|a| a.metrics.as_ref()).map(|m| m.0),
                    latency_ms: attempt.and_then(|a| a.latency_ms),
                    cost_usd: attempt.and_then(|a| a.usage.as_ref()).map(|u| u.0.cost_usd),
                    input_tokens: attempt
                        .and_then(|a| a.usage.as_ref())
                        .map(|u| u.0.input_tokens),
                    output_tokens: attempt
                        .and_then(|a| a.usage.as_ref())
                        .map(|u| u.0.output_tokens),
                    worst_price_source: attempt
                        .and_then(|a| a.usage.as_ref())
                        .map(|u| u.0.worst_price_source.clone()),
                    error: attempt.and_then(|a| a.error.clone()),
                }
            })
            .collect();
        Ok(data(RunView {
            id: run.id,
            task_id: run.task_id,
            status: run.status,
            graded: task.graded,
            sample_size_warning: SAMPLE_SIZE_WARNING.to_string(),
            results,
            created_at: run.created_at,
            finished_at: run.finished_at,
        }))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

// ── Universe (cross-task aggregate view) ────────────────────────────────────

/// A bound on how many attempts the universe view ever returns. Flat, uncursored — correct at
/// today's data volume; a genuinely large history needs real pagination, not raised here.
const MAX_UNIVERSE_ATTEMPTS: i64 = 500;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TaskSummary {
    pub id: Uuid,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_title: String,
    pub graded: bool,
    pub created_at: DateTime<Utc>,
}

impl From<TaskRow> for TaskSummary {
    fn from(t: TaskRow) -> Self {
        Self {
            id: t.id,
            owner: t.owner,
            repo: t.repo,
            pr_number: t.pr_number,
            pr_title: t.pr_title,
            graded: t.graded,
            created_at: t.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UniverseAttempt {
    pub attempt_id: Uuid,
    pub run_id: Uuid,
    pub task_id: Uuid,
    pub model_id: Uuid,
    pub model_name: String,
    pub provider: String,
    pub model: Option<String>,
    pub status: String,
    pub metrics: Option<Metrics>,
    pub latency_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub worst_price_source: Option<String>,
    pub error: Option<String>,
}

fn to_universe_attempt(row: store::UniverseAttemptRow) -> UniverseAttempt {
    UniverseAttempt {
        attempt_id: row.attempt_id,
        run_id: row.run_id,
        task_id: row.task_id,
        model_id: row.model_id,
        model_name: row.model_name,
        provider: row.provider,
        model: row.model,
        status: row.status,
        metrics: row.metrics.map(|m| m.0),
        latency_ms: row.latency_ms,
        cost_usd: row.usage.as_ref().map(|u| u.0.cost_usd),
        input_tokens: row.usage.as_ref().map(|u| u.0.input_tokens),
        output_tokens: row.usage.as_ref().map(|u| u.0.output_tokens),
        worst_price_source: row.usage.as_ref().map(|u| u.0.worst_price_source.clone()),
        error: row.error,
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UniverseView {
    pub tasks: Vec<TaskSummary>,
    pub attempts: Vec<UniverseAttempt>,
}

/// Every task and attempt the caller has run, flattened for a cross-task view — not one task's
/// detail. User-scoped like every other route here; no cross-user visibility.
pub async fn get_universe(State(state): State<AppState>, claims: Claims) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let tasks = store::users_tasks(&state.db, user_id, MAX_LIST)
            .await
            .map_err(internal)?
            .into_iter()
            .map(TaskSummary::from)
            .collect();
        let attempts = store::users_universe_attempts(&state.db, user_id, MAX_UNIVERSE_ATTEMPTS)
            .await
            .map_err(internal)?
            .into_iter()
            .map(to_universe_attempt)
            .collect();
        Ok(data(UniverseView { tasks, attempts }))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touched_files_reads_added_side_targets_and_skips_deletions() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@\n\
                     diff --git a/old.rs b/old.rs\n--- a/old.rs\n+++ /dev/null\n@@\n";
        assert_eq!(touched_files(diff), vec!["a.rs".to_string()]);
    }

    #[test]
    fn path_segments_reject_slashes_and_empty_strings() {
        assert!(valid_path_segment("nasiko"));
        assert!(valid_path_segment("nasiko-labs.oss"));
        assert!(!valid_path_segment(""));
        assert!(!valid_path_segment("a/b"));
        assert!(!valid_path_segment(&"a".repeat(101)));
    }

    #[test]
    fn universe_attempt_carries_the_joined_model_identity_and_flattens_usage() {
        use sqlx::types::Json;

        let row = store::UniverseAttemptRow {
            attempt_id: Uuid::nil(),
            run_id: Uuid::nil(),
            task_id: Uuid::nil(),
            model_id: Uuid::nil(),
            model_name: "Flint (Free)".to_string(),
            provider: "openrouter".to_string(),
            model: Some("recursive/flint-v1.0:free".to_string()),
            status: "done".to_string(),
            metrics: Some(Json(Metrics {
                true_positives: 1,
                false_positives: 0,
                false_negatives: 0,
                precision: 1.0,
                recall: 1.0,
                severity_accuracy: Some(1.0),
            })),
            latency_ms: Some(117_327),
            usage: Some(Json(AttemptUsage {
                calls: 1,
                cost_usd: 0.0,
                input_tokens: 12_105,
                output_tokens: 8_187,
                worst_price_source: "Default".to_string(),
            })),
            error: None,
        };

        let attempt = to_universe_attempt(row);

        assert_eq!(attempt.model_name, "Flint (Free)");
        assert_eq!(attempt.provider, "openrouter");
        assert_eq!(attempt.input_tokens, Some(12_105));
        assert_eq!(attempt.output_tokens, Some(8_187));
        assert_eq!(attempt.cost_usd, Some(0.0));
        assert_eq!(attempt.metrics.unwrap().precision, 1.0);
    }

    #[test]
    fn universe_attempt_with_no_usage_or_metrics_reports_none_not_zero() {
        let row = store::UniverseAttemptRow {
            attempt_id: Uuid::nil(),
            run_id: Uuid::nil(),
            task_id: Uuid::nil(),
            model_id: Uuid::nil(),
            model_name: "GPT-OSS 120B (Low Cost)".to_string(),
            provider: "openrouter".to_string(),
            model: Some("openai/gpt-oss-120b".to_string()),
            status: "pending".to_string(),
            metrics: None,
            latency_ms: None,
            usage: None,
            error: None,
        };

        let attempt = to_universe_attempt(row);

        assert!(attempt.metrics.is_none());
        assert!(attempt.cost_usd.is_none());
        assert!(attempt.input_tokens.is_none());
    }
}
