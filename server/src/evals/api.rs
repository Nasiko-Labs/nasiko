//! The `/api/evals/*` handlers for the Evals section and `nasiko eval`: the caller's harnesses
//! and sessions, datasets built from sessions, tasks, runs and their reports. Every object is
//! the caller's own; another user's id reads as `404`.
//!
//! Building a dataset and running one take minutes to hours, so both answer `202` with the new
//! object, and the caller polls it until its status is final.

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use nasiko_evals::TaskStatus;
use nasiko_evals::instance::Difficulty;
use nasiko_evals::prompt::ScreenVerdict;
use nasiko_evals::spec::EnvSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::ApiError;
use super::forge;
use super::jobs::{JobKind, NewJob, enqueue, notify};
use super::sessions::{self, Harness, SessionSummary};
use super::store::{self, RunModel, TaskRow};
use crate::auth::Claims;
use crate::state::AppState;

/// Most sessions one dataset is built from.
const MAX_SESSIONS: usize = 50;
const DEFAULT_SESSIONS_PAGE: i64 = 50;
const MAX_LIST: i64 = 100;

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
    ApiError::internal("eval request failed", e)
}

// ── Harnesses and sessions ───────────────────────────────────────────────────

/// `GET /api/evals/harnesses` answer.
#[derive(Serialize, utoipa::ToSchema)]
pub struct HarnessList {
    pub harnesses: Vec<Harness>,
    /// Whether the caller's GitHub account is connected (repositories are read through it).
    pub github_connected: bool,
    /// Whether this server keeps coding sessions' chat history, which tasks are built from.
    pub chat_history: bool,
    /// Whether this server builds datasets and runs evals (`EVAL_ENABLED` and a container
    /// backend).
    pub runner_available: bool,
}

#[utoipa::path(get, path = "/api/evals/harnesses", tag = "evals",
    responses((status = 200, description = "The caller's connected coding harnesses", body = HarnessList)))]
pub async fn list_harnesses(State(state): State<AppState>, claims: Claims) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let harnesses = sessions::harnesses(&state.db, user_id)
            .await
            .map_err(internal)?;
        let github_connected = forge::connection(&state, user_id).await.is_ok();
        Ok(data(HarnessList {
            harnesses,
            github_connected,
            chat_history: state.config.coding_agent_ingest.accept_chat_history,
            runner_available: runner_available(&state),
        }))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

fn runner_available(state: &AppState) -> bool {
    use nasiko_runtime::BatchRunner;
    state.config.evals.enabled && state.batch_runner.is_available()
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct SessionsQuery {
    /// Page size, 1-100 (default 50).
    pub limit: Option<i64>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<DateTime<Utc>>,
}

/// `GET /api/evals/harnesses/{id}/sessions` answer.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<DateTime<Utc>>,
}

#[utoipa::path(get, path = "/api/evals/harnesses/{id}/sessions", tag = "evals",
    params(("id" = String, Path, description = "Harness id"), SessionsQuery),
    responses(
        (status = 200, description = "The harness's newest sessions, each marked eligible or not", body = SessionList),
        (status = 404, description = "`harness_not_connected`"),
    ))]
pub async fn list_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Path(harness): Path<String>,
    Query(query): Query<SessionsQuery>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        if !sessions::has_harness(&state.db, user_id, &harness)
            .await
            .map_err(internal)?
        {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "harness_not_connected",
                "this coding harness is not connected",
            ));
        }
        let limit = query
            .limit
            .unwrap_or(DEFAULT_SESSIONS_PAGE)
            .clamp(1, MAX_LIST);
        let mut list = sessions::sessions(&state.db, user_id, &harness, query.cursor, limit + 1)
            .await
            .map_err(internal)?;
        let next_cursor = if list.len() as i64 > limit {
            list.truncate(limit as usize);
            list.last().map(|s| s.updated_at)
        } else {
            None
        };
        let readable = readable_repos(&state, user_id, &list).await;
        for session in &mut list {
            sessions::judge(&harness, session, |repo| {
                readable.as_ref().map(|r| r.contains(repo))
            });
        }
        Ok(data(SessionList {
            sessions: list,
            next_cursor,
        }))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// The GitHub repositories among the sessions' that the caller's account can read; `None` when
/// GitHub is not connected.
async fn readable_repos(
    state: &AppState,
    user_id: Uuid,
    list: &[SessionSummary],
) -> Option<BTreeSet<String>> {
    let (svc, token) = forge::connection(state, user_id).await.ok()?;
    let repos: BTreeSet<&str> = list
        .iter()
        .filter(|s| s.repo_host.as_deref() == Some("github.com"))
        .filter_map(|s| s.repo.as_deref())
        .collect();
    let mut readable = BTreeSet::new();
    for repo in repos {
        if let Ok(Some(info)) = svc.repo(&token, repo).await {
            readable.insert(repo.to_string());
            readable.insert(info.full_name);
        }
    }
    Some(readable)
}

// ── Datasets ─────────────────────────────────────────────────────────────────

/// `POST /api/evals/datasets` body.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateDataset {
    pub harness: String,
    /// Up to 50 of the harness's session ids.
    pub session_ids: Vec<String>,
    /// The LLM config that rewrites requests as issues; default: the config of the model the
    /// sessions mostly used, else the caller's default config.
    pub rewriter_id: Option<Uuid>,
    /// The LLM configs that screen tasks; default: the rewriter. Empty: the caller reviews.
    pub reviewer_ids: Option<Vec<Uuid>>,
    /// Runs of each gate set-up, 1-5 (default 3).
    pub gate_runs: Option<i32>,
}

#[utoipa::path(post, path = "/api/evals/datasets", tag = "evals", request_body = CreateDataset,
    responses(
        (status = 202, description = "The dataset, harvesting", body = DatasetView),
        (status = 400, description = "`invalid_sessions`, `invalid_model` or `invalid_gate_runs`"),
        (status = 404, description = "`harness_not_connected`"),
        (status = 409, description = "`github_not_connected` or `chat_history_off`"),
        (status = 503, description = "`eval_runner_unavailable`"),
    ))]
pub async fn create_dataset(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateDataset>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        require_runner(&state)?;
        if !state.config.coding_agent_ingest.accept_chat_history {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "chat_history_off",
                "this server keeps no coding-session chat history, which tasks are built from",
            ));
        }
        if forge::connection(&state, user_id).await.is_err() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "github_not_connected",
                "connect GitHub so the sessions' repositories can be read",
            ));
        }
        if !sessions::has_harness(&state.db, user_id, &body.harness)
            .await
            .map_err(internal)?
        {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "harness_not_connected",
                "this coding harness is not connected",
            ));
        }
        let ids: Vec<String> = body
            .session_ids
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() || ids.len() > MAX_SESSIONS {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_sessions",
                "pick 1 to 50 sessions",
            ));
        }
        let gate_runs = body.gate_runs.unwrap_or(3);
        if !(1..=5).contains(&gate_runs) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_gate_runs",
                "gate_runs must be 1-5",
            ));
        }
        let configs = user_configs(&state, user_id).await?;
        let rewriter = match body.rewriter_id {
            Some(id) => Some(owned_config(&configs, id)?),
            None => default_builder(&state, user_id, &ids, &configs).await?,
        };
        let reviewers: Vec<Uuid> = match &body.reviewer_ids {
            Some(list) => list
                .iter()
                .map(|id| owned_config(&configs, *id))
                .collect::<ApiResult<_>>()?,
            None => rewriter.into_iter().collect(),
        };
        let mut tx = state.db.begin().await.map_err(internal)?;
        let id = store::insert_dataset(
            &mut *tx,
            &store::NewDataset {
                user_id,
                harness: &body.harness,
                session_ids: &ids,
                rewriter_config_id: rewriter,
                reviewer_config_ids: &reviewers,
                gate_runs,
            },
        )
        .await
        .map_err(internal)?;
        let mut job = NewJob::new(JobKind::Harvest, user_id);
        job.dataset_id = Some(id);
        enqueue(&mut *tx, &job).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        notify(&state);
        Ok(accepted(dataset_view(&state, user_id, id).await?))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

fn require_runner(state: &AppState) -> ApiResult<()> {
    if runner_available(state) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "eval_runner_unavailable",
            "this server does not run evals (EVAL_ENABLED is off or no container backend)",
        ))
    }
}

/// One of the caller's LLM configs, offered as a model to test.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct ModelView {
    /// The `llm_configs.id`.
    pub id: Uuid,
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    /// An original session used this model: ticked by default, as the fair comparison.
    #[sqlx(skip)]
    pub used: bool,
}

async fn user_configs(state: &AppState, user_id: Uuid) -> ApiResult<Vec<ModelView>> {
    sqlx::query_as(
        "SELECT id, name, provider, model FROM llm_configs \
         WHERE created_by = $1 AND deleted_at IS NULL ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)
}

fn owned_config(configs: &[ModelView], id: Uuid) -> ApiResult<Uuid> {
    configs
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.id)
        .ok_or(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_model",
            "a model must be one of your LLM configs",
        ))
}

/// The config of the model the picked sessions mostly used, else the caller's default config.
async fn default_builder(
    state: &AppState,
    user_id: Uuid,
    session_ids: &[String],
    configs: &[ModelView],
) -> ApiResult<Option<Uuid>> {
    let top: Option<String> = sqlx::query_scalar(
        "SELECT model FROM trace_usage WHERE session_id = ANY($1) AND user_id = $2 \
         AND model IS NOT NULL GROUP BY model ORDER BY sum(output_tokens) DESC LIMIT 1",
    )
    .bind(session_ids)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;
    if let Some(found) = top.and_then(|m| configs.iter().find(|c| c.model.as_deref() == Some(&m))) {
        return Ok(Some(found.id));
    }
    sqlx::query_scalar(
        "SELECT id FROM llm_configs WHERE created_by = $1 AND is_default AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)
}

/// A dataset as the Evals section shows it.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DatasetView {
    pub id: Uuid,
    pub harness: String,
    /// `harvesting`, `preparing`, `ready` or `failed`.
    pub status: String,
    pub repos: Vec<String>,
    pub progress: Progress,
    /// What happened to each picked session (`task`, `no_match`, `no_repo`, ...).
    pub outcomes: BTreeMap<String, String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub tasks: Vec<TaskView>,
    pub models: Vec<ModelView>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct Progress {
    pub done: i64,
    pub total: i64,
}

/// One task in a dataset.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TaskView {
    pub id: Uuid,
    pub session_id: String,
    pub repo: String,
    /// The task's request (the issue P1 wrote, else the person's first message), first line.
    pub title: String,
    /// `candidate`, `reproducible`, `verified` (runnable) or `rejected`.
    pub status: String,
    /// The next step: `rewrite`, `build`, `screen`, `review` (a person) or `done`.
    pub stage: String,
    #[schema(value_type = Option<String>)]
    pub difficulty: Option<Difficulty>,
    pub fail_to_pass: usize,
    pub pass_to_pass: usize,
    pub reject_reason: Option<String>,
    pub original: OriginalView,
}

/// What the original session did: the baseline every model is compared with.
#[derive(Serialize, utoipa::ToSchema)]
pub struct OriginalView {
    pub model: String,
    /// Priced by the router's engine; `None` when a model has no real price.
    pub cost_usd: Option<f64>,
    /// From the message that led to the work to the commit.
    pub latency_ms: Option<i64>,
}

async fn dataset_view(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<DatasetView> {
    let dataset = store::users_dataset(&state.db, id, user_id)
        .await
        .map_err(internal)?
        .ok_or(not_found("no such dataset"))?;
    let rows = store::tasks_of(&state.db, id).await.map_err(internal)?;
    let mut tasks = Vec::with_capacity(rows.len());
    for row in &rows {
        tasks.push(task_view(state, row).await);
    }
    let originals: BTreeSet<&str> = rows
        .iter()
        .map(|r| r.instance.0.x_nasiko.orig.model.as_str())
        .collect();
    let mut models = user_configs(state, user_id).await?;
    for m in &mut models {
        m.used = m
            .model
            .as_deref()
            .is_some_and(|model| originals.contains(model));
    }
    let repos: BTreeSet<String> = rows.iter().map(|r| r.repo.clone()).collect();
    let done = rows
        .iter()
        .filter(|r| r.stage == "done" || r.stage == "review")
        .count() as i64;
    Ok(DatasetView {
        id: dataset.id,
        harness: dataset.harness,
        status: dataset.status,
        repos: repos.into_iter().collect(),
        progress: Progress {
            done,
            total: rows.len() as i64,
        },
        outcomes: dataset.outcomes.0,
        error: dataset.error,
        created_at: dataset.created_at,
        tasks,
        models,
    })
}

async fn task_view(state: &AppState, row: &TaskRow) -> TaskView {
    let task = &row.instance.0;
    let source = &row.source.0;
    let latency_ms = source
        .task_messages
        .first()
        .map(|m| (task.created_at - m.at).num_milliseconds())
        .filter(|ms| *ms >= 0);
    TaskView {
        id: row.id,
        session_id: row.source_session_id.clone(),
        repo: row.repo.clone(),
        title: task
            .problem_statement
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect(),
        status: row.status.clone(),
        stage: row.stage.clone(),
        difficulty: task.difficulty,
        fail_to_pass: task.fail_to_pass.len(),
        pass_to_pass: task.pass_to_pass.len(),
        reject_reason: task.x_nasiko.reject_reason.clone(),
        original: OriginalView {
            model: task.x_nasiko.orig.model.clone(),
            cost_usd: super::attempt::original_cost_of(state, source).await,
            latency_ms,
        },
    }
}

#[utoipa::path(get, path = "/api/evals/datasets/{id}", tag = "evals",
    params(("id" = Uuid, Path, description = "Dataset id")),
    responses((status = 200, description = "The dataset, its tasks and the models to test", body = DatasetView),
              (status = 404, description = "`not_found`")))]
pub async fn get_dataset(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        Ok(data(dataset_view(&state, user_id, id).await?))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// A dataset in the caller's list.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DatasetSummary {
    pub id: Uuid,
    pub harness: String,
    pub status: String,
    pub sessions: usize,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(get, path = "/api/evals/datasets", tag = "evals",
    responses((status = 200, description = "The caller's datasets, newest first", body = [DatasetSummary])))]
pub async fn list_datasets(State(state): State<AppState>, claims: Claims) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let list: Vec<DatasetSummary> = store::users_datasets(&state.db, user_id, MAX_LIST)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|d| DatasetSummary {
                id: d.id,
                harness: d.harness,
                status: d.status,
                sessions: d.session_ids.len(),
                created_at: d.created_at,
            })
            .collect();
        Ok(data(list))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(delete, path = "/api/evals/datasets/{id}", tag = "evals",
    params(("id" = Uuid, Path, description = "Dataset id")),
    responses((status = 204, description = "Deleted, with its tasks and runs"),
              (status = 404, description = "`not_found`"),
              (status = 409, description = "`busy`: it is still being built or run")))]
pub async fn delete_dataset(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        store::users_dataset(&state.db, id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such dataset"))?;
        let busy: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM eval_jobs j LEFT JOIN eval_runs r ON r.id = j.run_id \
             WHERE (j.dataset_id = $1 OR r.dataset_id = $1) AND j.status IN ('pending', 'running'))",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        .map_err(internal)?;
        if busy {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "busy",
                "the dataset is still being built or run",
            ));
        }
        sqlx::query("DELETE FROM eval_datasets WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

// ── Tasks and specs ──────────────────────────────────────────────────────────

async fn users_task(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<TaskRow> {
    let row = store::task(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or(not_found("no such task"))?;
    store::users_dataset(&state.db, row.dataset_id, user_id)
        .await
        .map_err(internal)?
        .ok_or(not_found("no such task"))?;
    Ok(row)
}

#[utoipa::path(get, path = "/api/evals/tasks/{id}", tag = "evals",
    params(("id" = Uuid, Path, description = "Task id")),
    responses((status = 200, description = "The task: SWE-bench row, link candidates, gate, reviews"),
              (status = 404, description = "`not_found`")))]
pub async fn get_task(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let row = users_task(&state, user_id, id).await?;
        Ok(data(json!({
            "id": row.id,
            "dataset_id": row.dataset_id,
            "status": row.status,
            "stage": row.stage,
            "instance": row.instance.0,
            "link": row.link.0,
            "gate": row.gate.map(|g| g.0),
            "reviews": row.reviews.0,
        })))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// `POST /api/evals/tasks/{id}/review` body: a person's verdict, on prompt P3's scale.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ReviewBody {
    /// 0 (clear) to 3 (unclear).
    pub clarity: u8,
    /// 0 (any correct fix passes) to 3 (only this exact fix passes).
    pub test_validity: u8,
    #[serde(default)]
    pub other_problems: Vec<String>,
    /// `<15 min fix`, `15 min - 1 hour`, `1-4 hours` or `>4 hours`.
    #[schema(value_type = String)]
    pub effort: Difficulty,
}

#[utoipa::path(post, path = "/api/evals/tasks/{id}/review", tag = "evals", request_body = ReviewBody,
    params(("id" = Uuid, Path, description = "Task id")),
    responses((status = 200, description = "The task, decided again"),
              (status = 400, description = "`invalid_review`"),
              (status = 404, description = "`not_found`"),
              (status = 409, description = "`not_reviewable`: only reproducible tasks are reviewed")))]
pub async fn review_task(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<ReviewBody>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let row = users_task(&state, user_id, id).await?;
        if row.instance.0.x_nasiko.status != TaskStatus::Reproducible {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "not_reviewable",
                "only a reproducible task is reviewed",
            ));
        }
        if body.clarity > 3 || body.test_validity > 3 {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_review",
                "scores are 0-3",
            ));
        }
        let verdict = ScreenVerdict {
            reviewer: String::new(),
            clarity: body.clarity,
            clarity_reason: String::new(),
            test_validity: body.test_validity,
            test_validity_reason: String::new(),
            other_problems: body.other_problems,
            effort: body.effort,
            effort_reason: String::new(),
        };
        super::review::record_human(&state, &row, verdict, &claims.username)
            .await
            .map_err(internal)?;
        let row = users_task(&state, user_id, id).await?;
        Ok(data(task_view(&state, &row).await))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(get, path = "/api/evals/specs/{owner}/{repo}", tag = "evals",
    params(("owner" = String, Path), ("repo" = String, Path)),
    responses((status = 200, description = "The repository's build and test settings"),
              (status = 404, description = "`not_found`")))]
pub async fn get_spec(
    State(state): State<AppState>,
    claims: Claims,
    Path((owner, repo)): Path<(String, String)>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let spec = store::spec(&state.db, user_id, &format!("{owner}/{repo}"))
            .await
            .map_err(internal)?
            .ok_or(not_found("no settings for this repository yet"))?;
        Ok(data(spec))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(put, path = "/api/evals/specs/{owner}/{repo}", tag = "evals",
    params(("owner" = String, Path), ("repo" = String, Path)),
    responses((status = 200, description = "Saved"), (status = 400, description = "`invalid_spec`")))]
pub async fn put_spec(
    State(state): State<AppState>,
    claims: Claims,
    Path((owner, repo)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let full = format!("{owner}/{repo}");
        let mut spec: EnvSpec = serde_json::from_value(body).map_err(|_| {
            ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_spec",
                "not build settings (see GET on this path for the shape)",
            )
        })?;
        if spec.test_cmd.trim().is_empty()
            || spec.build_cmd.trim().is_empty()
            || spec.toolchain_image.trim().is_empty()
            || spec.timeout_secs == 0
        {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_spec",
                "toolchain_image, build_cmd, test_cmd and timeout_secs are required",
            ));
        }
        spec.repo = full.clone();
        spec.local_path = None;
        store::save_spec(&state.db, user_id, &full, &spec)
            .await
            .map_err(internal)?;
        Ok(data(spec))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

// ── Runs ─────────────────────────────────────────────────────────────────────

/// `POST /api/evals/datasets/{id}/runs` body.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateRun {
    /// Verified tasks of the dataset.
    pub task_ids: Vec<Uuid>,
    /// The caller's LLM configs to test (`models[].id` of the dataset).
    pub model_ids: Vec<Uuid>,
    /// Attempts per task and model, 1-5 (default 1).
    pub attempts: Option<i32>,
}

#[utoipa::path(post, path = "/api/evals/datasets/{id}/runs", tag = "evals", request_body = CreateRun,
    params(("id" = Uuid, Path, description = "Dataset id")),
    responses((status = 202, description = "The run, queued", body = RunView),
              (status = 400, description = "`invalid_tasks`, `invalid_model` or `invalid_attempts`"),
              (status = 404, description = "`not_found`"),
              (status = 503, description = "`eval_runner_unavailable`")))]
pub async fn create_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(dataset_id): Path<Uuid>,
    Json(body): Json<CreateRun>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        require_runner(&state)?;
        if state.config.evals.router_url.is_none() {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "eval_runner_unavailable",
                "EVAL_ROUTER_URL is not set, so attempts cannot reach the model router",
            ));
        }
        let dataset = store::users_dataset(&state.db, dataset_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such dataset"))?;
        let attempts = body.attempts.unwrap_or(1);
        if !(1..=5).contains(&attempts) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_attempts",
                "attempts must be 1-5",
            ));
        }
        let tasks: Vec<TaskRow> = store::tasks_of(&state.db, dataset_id)
            .await
            .map_err(internal)?
            .into_iter()
            .filter(|t| body.task_ids.contains(&t.id))
            .collect();
        let picked: BTreeSet<Uuid> = body.task_ids.iter().copied().collect();
        if picked.is_empty()
            || tasks.len() != picked.len()
            || tasks
                .iter()
                .any(|t| t.instance.0.x_nasiko.status != TaskStatus::Verified)
        {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_tasks",
                "pick verified tasks of this dataset",
            ));
        }
        let configs = user_configs(&state, user_id).await?;
        let picked_models: BTreeSet<Uuid> = body.model_ids.iter().copied().collect();
        if picked_models.is_empty() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_model",
                "pick at least one model",
            ));
        }
        let builders: BTreeSet<Uuid> = dataset
            .rewriter_config_id
            .into_iter()
            .chain(dataset.reviewer_config_ids.iter().copied())
            .collect();
        let models: Vec<RunModel> = picked_models
            .iter()
            .map(|id| {
                let c = configs.iter().find(|c| c.id == *id).ok_or(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_model",
                    "a model must be one of your LLM configs",
                ))?;
                Ok(RunModel {
                    id: c.id,
                    name: c.name.clone(),
                    provider: c.provider.clone(),
                    model: c.model.clone(),
                    builder: builders.contains(&c.id),
                })
            })
            .collect::<ApiResult<_>>()?;

        let mut tx = state.db.begin().await.map_err(internal)?;
        let run_id: Uuid = sqlx::query_scalar(
            "INSERT INTO eval_runs (user_id, dataset_id, models, attempts) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(user_id)
        .bind(dataset_id)
        .bind(sqlx::types::Json(&models))
        .bind(attempts)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        for task in &tasks {
            for model in &models {
                for n in 1..=attempts {
                    store::insert_attempt(&mut *tx, run_id, task.id, model.id, n, &task.instance.0)
                        .await
                        .map_err(internal)?;
                }
            }
        }
        for model in &models {
            let mut job = NewJob::new(JobKind::RunPreflight, user_id);
            job.run_id = Some(run_id);
            job.model_id = Some(model.id);
            enqueue(&mut *tx, &job).await.map_err(internal)?;
        }
        tx.commit().await.map_err(internal)?;
        notify(&state);
        Ok(accepted(run_view(&state, user_id, run_id).await?))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

/// A run: its status, one result per attempt (the comparison table's cells) and, once done,
/// its report.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RunView {
    pub id: Uuid,
    pub dataset_id: Uuid,
    /// `queued`, `running`, `finalizing`, `done`, `failed` or `cancelled`.
    pub status: String,
    pub models: Vec<RunModel>,
    pub attempts: i32,
    pub progress: Progress,
    pub results: Vec<ResultView>,
    /// Per model (`ModelSummary`: resolved rate with its 95% range, cost, saving, ...).
    #[schema(value_type = Object)]
    pub report: Option<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// One attempt: a cell of the comparison table.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ResultView {
    pub attempt_id: Uuid,
    pub task_id: Uuid,
    pub model_id: Uuid,
    pub attempt: i32,
    /// `pending`, `running`, `done`, `failed` or `cancelled`.
    pub status: String,
    /// 0-100: the share of the task's failing tests the model fixed, 0 if it broke others.
    pub score: Option<u32>,
    /// SWE-bench's rule: every FAIL_TO_PASS and PASS_TO_PASS test passes.
    pub resolved: Option<bool>,
    /// The router's cost for the attempt; `None` until the run is finalized or when unpriced.
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<u64>,
    /// Why it was not resolved (`no_patch`, `tests_failed`, `regression`, ...).
    pub failure: Option<String>,
    /// The model also rewrote or screened the task: its score is optimistic.
    pub builder: bool,
    pub error: Option<String>,
}

async fn run_view(state: &AppState, user_id: Uuid, id: Uuid) -> ApiResult<RunView> {
    let run = store::users_run(&state.db, id, user_id)
        .await
        .map_err(internal)?
        .ok_or(not_found("no such run"))?;
    let attempts = store::attempts_of(&state.db, id).await.map_err(internal)?;
    let builders: BTreeSet<Uuid> = run
        .models
        .0
        .iter()
        .filter(|m| m.builder)
        .map(|m| m.id)
        .collect();
    let done = attempts
        .iter()
        .filter(|a| !matches!(a.status.as_str(), "pending" | "running"))
        .count() as i64;
    let results = attempts
        .iter()
        .map(|a| {
            let record = a.record.as_ref().map(|r| &r.0);
            ResultView {
                attempt_id: a.id,
                task_id: a.task_id,
                model_id: a.model_id,
                attempt: a.attempt_no,
                status: a.status.clone(),
                score: record.map(|r| super::attempt::score(&r.grade)),
                resolved: record.map(|r| r.grade.resolved),
                cost_usd: record.and_then(|r| r.cost_usd),
                latency_ms: record.map(|r| r.wall_secs * 1000),
                failure: record
                    .and_then(|r| r.grade.failure)
                    .and_then(|f| serde_json::to_value(f).ok())
                    .and_then(|v| v.as_str().map(str::to_string)),
                builder: builders.contains(&a.model_id),
                error: a.error.clone(),
            }
        })
        .collect();
    Ok(RunView {
        id: run.id,
        dataset_id: run.dataset_id,
        status: run.status,
        attempts: run.attempts,
        models: run.models.0,
        progress: Progress {
            done,
            total: attempts.len() as i64,
        },
        results,
        report: run.report.map(|r| r.0),
        error: run.error,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
    })
}

#[utoipa::path(get, path = "/api/evals/runs/{id}", tag = "evals",
    params(("id" = Uuid, Path, description = "Run id")),
    responses((status = 200, description = "The run, its results and report", body = RunView),
              (status = 404, description = "`not_found`")))]
pub async fn get_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        Ok(data(run_view(&state, user_id, id).await?))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct RunsQuery {
    /// Only this dataset's runs.
    pub dataset_id: Option<Uuid>,
}

/// A run in the caller's list (saved evals).
#[derive(Serialize, utoipa::ToSchema)]
pub struct RunSummary {
    pub id: Uuid,
    pub dataset_id: Uuid,
    pub status: String,
    pub models: Vec<RunModel>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[utoipa::path(get, path = "/api/evals/runs", tag = "evals", params(RunsQuery),
    responses((status = 200, description = "The caller's runs, newest first", body = [RunSummary])))]
pub async fn list_runs(
    State(state): State<AppState>,
    claims: Claims,
    Query(query): Query<RunsQuery>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let list: Vec<RunSummary> =
            store::users_runs(&state.db, user_id, query.dataset_id, MAX_LIST)
                .await
                .map_err(internal)?
                .into_iter()
                .map(|r| RunSummary {
                    id: r.id,
                    dataset_id: r.dataset_id,
                    status: r.status,
                    models: r.models.0,
                    created_at: r.created_at,
                    finished_at: r.finished_at,
                })
                .collect();
        Ok(data(list))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(get, path = "/api/evals/runs/{id}/attempts/{attempt_id}", tag = "evals",
    params(("id" = Uuid, Path), ("attempt_id" = Uuid, Path)),
    responses((status = 200, description = "The attempt: graded patch, grade, logs"),
              (status = 404, description = "`not_found`")))]
pub async fn get_attempt(
    State(state): State<AppState>,
    claims: Claims,
    Path((run_id, attempt_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        store::users_run(&state.db, run_id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such run"))?;
        let attempt = store::attempt(&state.db, attempt_id)
            .await
            .map_err(internal)?
            .filter(|a| a.run_id == run_id)
            .ok_or(not_found("no such attempt"))?;
        let mut logs = BTreeMap::new();
        for (name, key) in &attempt.artifacts.0 {
            if let Ok(bytes) = state.oci_storage.get_blob(key).await {
                let text = String::from_utf8_lossy(&bytes);
                logs.insert(
                    name.clone(),
                    nasiko_runtime::batch::tail(&text, 256 * 1024).to_string(),
                );
            }
        }
        Ok(data(json!({
            "id": attempt.id,
            "task_id": attempt.task_id,
            "model_id": attempt.model_id,
            "attempt": attempt.attempt_no,
            "status": attempt.status,
            "record": attempt.record.map(|r| r.0),
            "patch": attempt.patch,
            "error": attempt.error,
            "logs": logs,
        })))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(post, path = "/api/evals/runs/{id}/cancel", tag = "evals",
    params(("id" = Uuid, Path)),
    responses((status = 200, description = "Cancelled: no new attempt starts", body = RunView),
              (status = 404, description = "`not_found`")))]
pub async fn cancel_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        store::users_run(&state.db, id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such run"))?;
        let mut tx = state.db.begin().await.map_err(internal)?;
        sqlx::query(
            "UPDATE eval_attempts SET status = 'cancelled', finished_at = now() \
             WHERE run_id = $1 AND status = 'pending'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "UPDATE eval_jobs SET status = 'failed', error = 'cancelled', finished_at = now() \
             WHERE run_id = $1 AND status = 'pending'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "UPDATE eval_runs SET status = 'cancelled', finished_at = now() \
             WHERE id = $1 AND status NOT IN ('done', 'failed', 'cancelled')",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(data(run_view(&state, user_id, id).await?))
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[utoipa::path(delete, path = "/api/evals/runs/{id}", tag = "evals",
    params(("id" = Uuid, Path)),
    responses((status = 204, description = "Deleted"),
              (status = 404, description = "`not_found`"),
              (status = 409, description = "`busy`: cancel it first")))]
pub async fn delete_run(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> Response {
    let result: ApiResult<Response> = async {
        let user_id = user(&claims)?;
        let run = store::users_run(&state.db, id, user_id)
            .await
            .map_err(internal)?
            .ok_or(not_found("no such run"))?;
        if super::jobs::open_count(&state.db, None, Some(id))
            .await
            .map_err(internal)?
            > 0
            && run.status != "cancelled"
        {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "busy",
                "the run is still going: cancel it first",
            ));
        }
        sqlx::query("DELETE FROM eval_runs WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
