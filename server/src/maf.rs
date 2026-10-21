use crate::auth::Claims;
use crate::state::AppState;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use nasiko_orchestrator::RouteRequest;
use nasiko_orchestrator::maf::{
    decomposer::DecomposerClient,
    llm::LlmClient,
    planner::{self, AgentInfo as PlannerAgentInfo},
    types::{MafDefinition, MafStep, StepResult},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// MAF routes, split into two rate-limit classes because their costs differ by
/// orders of magnitude — see the limiter definitions in `lib.rs`.
///
/// Both limiters are per-caller (`limit_by_user`), so one tenant cannot starve
/// another. Layering them here rather than on the whole `protected` router
/// keeps MAF's budget separate from unrelated endpoints.
pub fn router(
    run_limiter: crate::rate_limit::RateLimiter,
    read_limiter: crate::rate_limit::RateLimiter,
) -> Router<AppState> {
    // Expensive: each of these fans out to multiple LLM calls (and, for a run,
    // N agent HTTP calls on top). Left unlimited, one client could enqueue
    // workflow runs in a loop and bill the deployment for all of it.
    let expensive = Router::new()
        .route(
            "/maf/workflow/from-instruction",
            post(create_maf_from_instruction),
        )
        .route("/maf/generate", post(generate_maf))
        .route("/maf/workflow/{id}/run", post(run_workflow))
        .layer(axum::middleware::from_fn_with_state(
            run_limiter,
            crate::rate_limit::limit_by_user,
        ));

    // Cheap single-row reads plus CRUD. The budget here is deliberately loose:
    // `/maf/execution/{id}` and `/maf/execution/{id}/usage` are both polled by
    // the UI while a workflow runs, so a tight window would break normal use
    // rather than abuse.
    let standard = Router::new()
        .route("/maf/workflows", get(list_mafs).post(create_maf))
        // Static segment "result" wins over {id} in matchit so this route is unambiguous
        .route("/maf/workflow/result/{exec_id}", get(get_result))
        // Same static-beats-{id} rule: "draft"/"drafts" never shadow a UUID.
        // Saving a draft is a plain row write — no decomposer, no routing — so
        // it sits in the cheap tier and can be called on every keystroke pause.
        .route("/maf/workflow/draft", post(save_draft))
        .route("/maf/workflow/drafts", get(list_drafts))
        // Promotion is a single-row status update: the steps and their agents
        // were resolved once, when `from-instruction` created the draft, and
        // promotion never re-derives them. That makes it as cheap as any other
        // CRUD write, so it shares their budget rather than the LLM tier's.
        .route("/maf/workflow/{id}/promote", post(promote_draft))
        .route(
            "/maf/workflow/{id}",
            get(get_maf).put(update_maf).delete(delete_maf),
        )
        .route("/maf/workflow/{id}/executions", get(list_executions))
        .route("/maf/executions", get(list_all_executions))
        .route("/maf/execution/{id}", get(get_execution))
        .route("/maf/execution/{id}/usage", get(get_execution_usage))
        .layer(axum::middleware::from_fn_with_state(
            read_limiter,
            crate::rate_limit::limit_by_user,
        ));

    expensive.merge(standard)
}

// ─── Shared helpers ────────────────────────────────────────────────────────
//
// Every MAF response — success or error — is wrapped in the same envelope:
// {"data": <payload or null>, "status_code": <mirrors the real HTTP status>, "message": <human-readable>}
// so frontend code can parse one shape regardless of outcome.

fn parse_user_id(claims: &Claims) -> Option<Uuid> {
    claims.sub.parse().ok()
}

fn ok_json<T: Serialize>(status: StatusCode, data: T, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({
            "data": data,
            "status_code": status.as_u16(),
            "message": message
        })),
    )
        .into_response()
}

fn err_json(status: StatusCode, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({
            "data": serde_json::Value::Null,
            "status_code": status.as_u16(),
            "message": message
        })),
    )
        .into_response()
}

fn unauthorized() -> axum::response::Response {
    err_json(StatusCode::UNAUTHORIZED, "invalid or missing user identity")
}

fn internal_err(e: impl std::fmt::Display) -> axum::response::Response {
    err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

fn not_found(resource: &str) -> axum::response::Response {
    err_json(StatusCode::NOT_FOUND, &format!("{resource} not found"))
}

fn forbidden(msg: &str) -> axum::response::Response {
    err_json(StatusCode::FORBIDDEN, msg)
}

fn bad_request(msg: &str) -> axum::response::Response {
    err_json(StatusCode::BAD_REQUEST, msg)
}

// ─── DB row types (JSONB cast to text in SQL) ──────────────────────────────

#[derive(Debug, sqlx::FromRow)]
struct MafRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    description: Option<String>,
    maf_json: String, // fetched via maf_json::text
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// How many times this workflow has been run — COUNT(*) over maf_executions,
    /// computed on read rather than stored, so it can never drift out of sync.
    execution_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ExecRow {
    id: Uuid,
    /// User-facing incremental id — globally sequential, cosmetic only.
    /// `id` (UUID) remains the real identifier used internally (A2A
    /// contextId, Redis job key); never used as a lookup key.
    execution_number: i64,
    maf_id: Option<Uuid>,
    user_id: Uuid,
    status: String,
    attempt_count: i32,
    max_attempts: i32,
    tokens_used: i64,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    output: Option<String>,
    step_results: Option<String>, // fetched via step_results::text
    error: Option<String>,
    created_at: DateTime<Utc>,
}

/// Same shape as ExecRow, plus the parent workflow's current name/status —
/// fetched via LEFT JOIN so it's populated whether the workflow is active,
/// soft-deleted, or (defensively) its maf_id has gone missing entirely.
#[derive(Debug, sqlx::FromRow)]
struct ExecWithWorkflowRow {
    id: Uuid,
    execution_number: i64,
    maf_id: Option<Uuid>,
    user_id: Uuid,
    status: String,
    attempt_count: i32,
    max_attempts: i32,
    tokens_used: i64,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    output: Option<String>,
    step_results: Option<String>,
    error: Option<String>,
    created_at: DateTime<Utc>,
    workflow_name: Option<String>,
    /// "active" | "deleted", or None if the workflow row itself is gone.
    workflow_status: Option<String>,
}

// ─── Response types ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MafResponse {
    id: Uuid,
    user_id: Uuid,
    name: String,
    description: Option<String>,
    maf_json: serde_json::Value,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    execution_count: i64,
}

#[derive(Serialize)]
struct ExecResponse {
    id: Uuid,
    execution_number: i64,
    maf_id: Option<Uuid>,
    user_id: Uuid,
    status: String,
    attempt_count: i32,
    max_attempts: i32,
    tokens_used: i64,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    output: Option<String>,
    step_results: Option<serde_json::Value>,
    error: Option<String>,
    created_at: DateTime<Utc>,
}

/// Additive on top of whichever execution shape the endpoint already returned —
/// `#[serde(flatten)]` keeps every existing field byte-identical. `hitl` is how the frontend
/// recovers a paused step's `hitl_requests.id` directly from the execution it's already polling,
/// so it never has to call `GET /api/hitl/pending` to correlate a MAF pause — and, once the run
/// has moved on, how it shows what the human actually answered.
///
/// Generic over the inner exec because all four execution endpoints carry it: the two
/// single-execution ones over `ExecResponse`, and the two list ones over `ExecResponse` /
/// `ExecWithWorkflowResponse`. The lists earn it despite returning many rows at once: a decided
/// row is the only record of a human's answer, and without it a finished run showed the workflow
/// acting on an answer nobody could see. They pay one batched query for the whole page
/// (`HitlStore::list_for_maf_executions`), not one per row.
#[derive(Serialize)]
struct ExecWithHitlResponse<T> {
    #[serde(flatten)]
    exec: T,
    /// Every HITL tied to this execution, pending or already resolved — oldest first, same shape
    /// `GET /api/hitl/{id}` returns. At most one entry is ever `status: "pending"` at a time
    /// (MAF steps run strictly sequentially); the rest are historical audit records.
    hitl: Vec<serde_json::Value>,
}

/// Shared by `get_result`/`get_execution` — fetches this execution's HITL rows scoped by the
/// SAME `user_id` the caller already validated against `maf_executions.user_id` (both call sites
/// check `row.user_id == user_id` before reaching here), so a HITL row can never leak across
/// owners even if `hitl_requests.owner_user_id` and `maf_executions.user_id` were ever to drift.
/// A lookup failure surfaces as a real 500 (matching `chat/routes.rs::list_messages`'s own HITL
/// lookup) rather than silently degrading to an empty array — an execution genuinely
/// `awaiting_human` must never be misreported as having nothing pending.
async fn hitl_rows_for_execution(
    hitl_store: &std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    execution_id: Uuid,
    owner_user_id: Uuid,
) -> Result<Vec<serde_json::Value>, nasiko_hitl::HitlError> {
    let rows = hitl_store
        .list_for_maf_execution(execution_id, owner_user_id)
        .await?;
    // Each row goes through `resolve_display_row` before `to_response` — a no-op for the
    // ordinary case, but substitutes the real row's id/kind/question when this row is a
    // `maf`-origin mirror of a real `mcp_tool` block (a step's underlying agent call mapping an
    // MCP tool-approval gate onto its own pause, the same dual-origin situation direct-chat's
    // `chat/routes.rs::list_messages` already accounts for). The real `mcp_tool` row itself is
    // never returned by `list_for_maf_execution` at all (it has no `maf_execution_id`), so
    // without this the frontend would only ever see the mirror's own generic placeholder.
    let mut hitl = Vec::with_capacity(rows.len());
    for row in &rows {
        let display =
            nasiko_hitl::resolve_display_row(hitl_store.as_ref(), row, owner_user_id).await;
        hitl.push(crate::router::hitl::to_response(&display));
    }
    Ok(hitl)
}

/// The same rows as `hitl_rows_for_execution`, for a whole page of executions — one batched query
/// instead of one per row, so the two list endpoints can carry a run's HITL history without their
/// cost growing with the page size.
///
/// Returns a map keyed by execution id; an execution that never paused is simply absent, and the
/// caller renders it as the empty list it already was. Like the single-execution helper, a lookup
/// failure is a real 500 rather than a silent empty map — a list that quietly forgets every
/// human answer is worse than one that says it could not be read.
async fn hitl_rows_for_executions(
    hitl_store: &std::sync::Arc<dyn nasiko_hitl::HitlStore>,
    execution_ids: &[Uuid],
    owner_user_id: Uuid,
) -> Result<std::collections::HashMap<Uuid, Vec<serde_json::Value>>, nasiko_hitl::HitlError> {
    let by_exec = hitl_store
        .list_for_maf_executions(execution_ids, owner_user_id)
        .await?;

    let mut out = std::collections::HashMap::with_capacity(by_exec.len());
    for (exec_id, rows) in by_exec {
        // Same `resolve_display_row` pass the single-execution helper documents — a `maf`-origin
        // mirror of a real `mcp_tool` block must show the real row's question, not its own
        // placeholder.
        let mut hitl = Vec::with_capacity(rows.len());
        for row in &rows {
            let display =
                nasiko_hitl::resolve_display_row(hitl_store.as_ref(), row, owner_user_id).await;
            hitl.push(crate::router::hitl::to_response(&display));
        }
        out.insert(exec_id, hitl);
    }
    Ok(out)
}

fn maf_row_to_response(row: MafRow) -> MafResponse {
    let maf_json = serde_json::from_str(&row.maf_json).unwrap_or(serde_json::Value::Null);
    MafResponse {
        id: row.id,
        user_id: row.user_id,
        name: row.name,
        description: row.description,
        maf_json,
        status: row.status,
        created_at: row.created_at,
        updated_at: row.updated_at,
        execution_count: row.execution_count,
    }
}

fn exec_row_to_response(row: ExecRow) -> ExecResponse {
    let step_results = row
        .step_results
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    ExecResponse {
        id: row.id,
        execution_number: row.execution_number,
        maf_id: row.maf_id,
        user_id: row.user_id,
        status: row.status,
        attempt_count: row.attempt_count,
        max_attempts: row.max_attempts,
        tokens_used: row.tokens_used,
        started_at: row.started_at,
        completed_at: row.completed_at,
        duration_ms: row.duration_ms,
        output: row.output,
        step_results,
        error: row.error,
        created_at: row.created_at,
    }
}

#[derive(Serialize)]
struct ExecWithWorkflowResponse {
    id: Uuid,
    execution_number: i64,
    maf_id: Option<Uuid>,
    user_id: Uuid,
    status: String,
    attempt_count: i32,
    max_attempts: i32,
    tokens_used: i64,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    output: Option<String>,
    step_results: Option<serde_json::Value>,
    error: Option<String>,
    created_at: DateTime<Utc>,
    workflow_name: Option<String>,
    workflow_status: Option<String>,
}

fn exec_with_workflow_row_to_response(row: ExecWithWorkflowRow) -> ExecWithWorkflowResponse {
    let step_results = row
        .step_results
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    ExecWithWorkflowResponse {
        id: row.id,
        execution_number: row.execution_number,
        maf_id: row.maf_id,
        user_id: row.user_id,
        status: row.status,
        attempt_count: row.attempt_count,
        max_attempts: row.max_attempts,
        tokens_used: row.tokens_used,
        started_at: row.started_at,
        completed_at: row.completed_at,
        duration_ms: row.duration_ms,
        output: row.output,
        step_results,
        error: row.error,
        created_at: row.created_at,
        workflow_name: row.workflow_name,
        workflow_status: row.workflow_status,
    }
}

// ─── Request types ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateStepRequest {
    task_description: String,
    agent_id: Option<Uuid>,
}

#[derive(Deserialize)]
struct CreateMafRequest {
    /// Optional name — if omitted, derived from the first task description.
    name: Option<String>,
    description: Option<String>,
    steps: Vec<CreateStepRequest>,
}

// No `step_index`: the order of the array is the order of the steps, and
// `update_maf` renumbers from it. It used to be a required field that was then
// thrown away, so a client sending a well-formed list without it got a 422
// naming a field that changes nothing. Serde ignores it if it is still sent.
#[derive(Deserialize)]
struct UpdateStepRequest {
    #[serde(default)]
    agent_id: Option<Uuid>,
    task_description: String,
}

// Serde helper: distinguishes absent (keep existing) from explicit null (clear the field).
// absent → field missing → outer Option is None → keep existing
// null   → field present but null → outer Option is Some(None) → set to NULL
// value  → field present with value → outer Option is Some(Some(v)) → set to v
mod nullable {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Some(Option::<T>::deserialize(d)?))
    }
}

#[derive(Deserialize)]
struct UpdateMafRequest {
    name: Option<String>,
    #[serde(default, deserialize_with = "nullable::deserialize")]
    description: Option<Option<String>>,
    steps: Option<Vec<UpdateStepRequest>>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 {
    50
}

/// Sort order for the workflows list.
///
/// A closed enum rather than a free string: each variant maps to one fixed
/// `ORDER BY` fragment, so a caller can never reach the query planner with
/// text of their own.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowSort {
    /// Newest first — the list's default, shown as "All" in the UI.
    #[default]
    Recent,
    SuccessRate,
    TokenUsage,
    ExecutionCount,
    Health,
}

impl WorkflowSort {
    /// This option's `ORDER BY` fragment.
    ///
    /// Every option falls back to `created_at` so the order is total: without a
    /// tiebreak, rows with equal metrics (very common — a fleet of workflows at
    /// 100%) could come back in a different order on each page of the same
    /// listing, and the client would show duplicates and drop rows.
    ///
    /// Workflows that have never run sort last wherever the metric is NULL: a
    /// row with no history is never what someone sorting by a metric is looking
    /// for.
    fn order_by(self) -> &'static str {
        match self {
            Self::Recent => "m.created_at DESC",
            Self::SuccessRate => "success_rate DESC NULLS LAST, m.created_at DESC",
            Self::TokenUsage => "total_tokens DESC, m.created_at DESC",
            Self::ExecutionCount => "execution_count DESC, m.created_at DESC",
            // Ascending, i.e. worst first. Health is bucketed from the same
            // number `SuccessRate` sorts on, so descending would make the two
            // options identical; surfacing the workflows that need attention is
            // the only reading under which a separate "Health" option earns its
            // place next to "Success rate".
            Self::Health => "success_rate ASC NULLS LAST, m.created_at DESC",
        }
    }
}

/// Sort order for the drafts list.
///
/// Deliberately a shorter menu than [`WorkflowSort`]: a draft has no run
/// history to rank by, so success rate and execution count would sort every row
/// identically. Token usage is kept because a draft that was promoted, run and
/// sent back to draft state does carry spend.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DraftSort {
    /// Newest draft first — the list's default, shown as "All" in the UI.
    #[default]
    All,
    LastUpdated,
    TokenUsage,
}

impl DraftSort {
    fn order_by(self) -> &'static str {
        match self {
            // "All" orders by when the draft was started; "Last updated" by
            // when it was last edited. They differ for any draft that has been
            // reopened and changed since it was first saved.
            Self::All => "m.created_at DESC",
            Self::LastUpdated => "m.updated_at DESC, m.created_at DESC",
            Self::TokenUsage => "total_tokens DESC, m.updated_at DESC",
        }
    }
}

/// Query parameters for the drafts list — same shape as
/// [`WorkflowListQuery`], with the drafts-specific sort menu.
#[derive(Deserialize)]
struct DraftListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    sort: DraftSort,
}

/// A workflow's health, bucketed from its all-time success rate.
///
/// Derived on read rather than stored: it is a pure function of figures the
/// list query already computes, and a stored copy would be one more thing that
/// can drift away from the executions it summarises.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Health {
    Healthy,
    Degraded,
    Unhealthy,
    /// Never run, so there is no rate to judge. Distinct from `Unhealthy` so
    /// the UI can say "no runs yet" instead of branding a new workflow as
    /// failing — a third of the workflows in a working deployment have never
    /// been run.
    Unknown,
}

impl Health {
    fn from_success_rate(rate: Option<f64>) -> Self {
        match rate {
            None => Self::Unknown,
            Some(rate) if rate >= 90.0 => Self::Healthy,
            Some(rate) if rate >= 50.0 => Self::Degraded,
            Some(_) => Self::Unhealthy,
        }
    }
}

/// Query parameters for the workflows list.
///
/// Its own type rather than an extension of [`ListQuery`]: search and sort are
/// meaningless on the two execution listings that share `ListQuery`, and
/// `#[serde(flatten)]` cannot be used to compose the two because the
/// form-encoded decoder behind `Query` does not support it.
#[derive(Deserialize)]
struct WorkflowListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    /// Case-insensitive substring match over name and description.
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    sort: WorkflowSort,
}

/// One row of the workflows list: the stored workflow plus the aggregates the
/// list renders.
///
/// Separate from [`MafRow`] because the single-workflow endpoints compute no
/// aggregates, and widening their shared row would force every one of their
/// queries to produce figures nothing reads.
#[derive(Debug, sqlx::FromRow)]
struct WorkflowListRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    description: Option<String>,
    maf_json: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    execution_count: i64,
    success_rate: Option<f64>,
    total_tokens: i64,
    last_run_at: Option<DateTime<Utc>>,
    last_run_status: Option<String>,
}

#[derive(Serialize)]
struct WorkflowListResponse {
    id: Uuid,
    user_id: Uuid,
    name: String,
    description: Option<String>,
    maf_json: serde_json::Value,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    execution_count: i64,
    /// All-time successful runs as a percentage to one decimal place, or `null`
    /// when the workflow has never run. `null` is not `0` — see [`Health`].
    success_rate: Option<f64>,
    health: Health,
    /// All-time orchestration spend across every execution of this workflow.
    total_tokens: i64,
    /// When the most recent execution *started*, or `null` if none ever has.
    last_run_at: Option<DateTime<Utc>>,
    /// That same execution's status, so the list can show the last outcome
    /// independently of aggregate health — a workflow at 96% whose latest run
    /// failed is healthy overall and worth flagging right now.
    last_run_status: Option<String>,
    step_count: usize,
    /// The distinct agents this workflow uses, in first-use order.
    agent_names: Vec<String>,
}

/// The distinct agents a workflow's steps use, in first-use order.
///
/// De-duplicated on agent id: a workflow may call the same agent in several
/// steps, and the list shows each agent once.
fn distinct_agent_names(definition: &MafDefinition) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    definition
        .steps
        .iter()
        .filter(|step| seen.insert(step.agent_id))
        .map(|step| step.agent_name.clone())
        .collect()
}

/// Which rows one page of the workflow list selects.
#[derive(Clone, Copy)]
enum ListScope {
    /// Live workflows — what the deployed list shows.
    Active,
    /// Everything that began life as a draft, still a draft or not.
    ///
    /// A promoted draft deliberately stays in this list rather than vanishing
    /// from it: the point of the drafts view is to follow an idea from the
    /// sentence you typed through to the runs and spend it went on to produce,
    /// and a row that disappears the moment it is deployed can't show that.
    /// Rows are told apart by their `status` — `"draft"` versus `"active"`.
    Drafted,
}

impl ListScope {
    /// This scope's `WHERE` fragment. A closed enum, so no caller text ever
    /// reaches the statement.
    fn predicate(self) -> &'static str {
        match self {
            Self::Active => "m.status = 'active'",
            // Soft-deleted rows stay out: discarding a draft means discarding
            // it, whether or not it had been deployed by then.
            Self::Drafted => "m.drafted_at IS NOT NULL AND m.status <> 'deleted'",
        }
    }
}

/// What to select, and how to order, for one page of the workflow list.
struct WorkflowPage<'a> {
    user_id: Uuid,
    scope: ListScope,
    search: Option<&'a str>,
    /// A fixed `ORDER BY` fragment from a sort enum — never caller text.
    order_by: &'a str,
    limit: i64,
    offset: i64,
}

/// One page of workflows with the aggregates the list cards render.
///
/// Shared by the active list and the drafts list. Both draw the same card, so
/// both need the same figures, and the only differences are which `status` they
/// select and how they order — which makes a second copy of this statement pure
/// downside: a column added for one list would silently go missing from the
/// other.
///
/// Every per-row figure comes out of this single statement. The obvious
/// alternative — a correlated subquery per metric, or a round trip per card —
/// turns one listing into O(rows x metrics) queries, which is the shape that
/// makes a dashboard slow once a user has more than a handful of workflows.
///
/// `last_run_status` is the exception: it is the *latest* execution's status
/// rather than an aggregate, so it cannot come from the GROUP BY. It stays a
/// subquery, ordered the same way `MAX(started_at)` picks its row so the two
/// always describe the same execution, and it reads straight down
/// `idx_maf_executions_maf_started (maf_id, started_at)`.
///
/// Executions queued but never started carry a NULL `started_at` and are
/// excluded: "last run" means the last one that actually ran.
async fn fetch_workflow_page(
    db: &sqlx::PgPool,
    page: WorkflowPage<'_>,
) -> Result<Vec<WorkflowListRow>, sqlx::Error> {
    let sql = format!(
        r#"SELECT m.id, m.user_id, m.name, m.description, m.maf_json::text AS maf_json,
                  m.status, m.created_at, m.updated_at,
                  COUNT(e.id) AS execution_count,
                  ROUND(100.0 * COUNT(*) FILTER (WHERE e.status = 'success')
                        / NULLIF(COUNT(e.id), 0), 1)::float8 AS success_rate,
                  COALESCE(SUM(e.tokens_used), 0)::bigint AS total_tokens,
                  MAX(e.started_at) AS last_run_at,
                  (SELECT latest.status
                     FROM maf_executions latest
                    WHERE latest.maf_id = m.id AND latest.started_at IS NOT NULL
                    ORDER BY latest.started_at DESC
                    LIMIT 1) AS last_run_status
           FROM mafs m
           LEFT JOIN maf_executions e ON e.maf_id = m.id
           WHERE m.user_id = $1 AND {}
             AND ($4::text IS NULL
                  OR m.name ILIKE '%' || $4 || '%'
                  OR COALESCE(m.description, '') ILIKE '%' || $4 || '%')
           GROUP BY m.id
           ORDER BY {}
           LIMIT $2 OFFSET $3"#,
        page.scope.predicate(),
        page.order_by
    );

    sqlx::query_as::<_, WorkflowListRow>(&sql)
        .bind(page.user_id)
        .bind(page.limit)
        .bind(page.offset)
        .bind(page.search)
        .fetch_all(db)
        .await
}

/// An all-whitespace `?search=` is someone who has cleared the box, not a
/// search for spaces — treat it as absent so it doesn't match nothing.
fn normalize_search(search: Option<&str>) -> Option<&str> {
    search.map(str::trim).filter(|s| !s.is_empty())
}

fn list_row_to_response(row: WorkflowListRow) -> WorkflowListResponse {
    // A row whose definition no longer parses still belongs in the list — it is
    // exactly the row a user needs to see in order to fix or delete it — so a
    // parse failure degrades to an empty shape rather than dropping the card.
    let definition = serde_json::from_str::<MafDefinition>(&row.maf_json).ok();
    let step_count = definition.as_ref().map_or(0, |d| d.steps.len());
    let agent_names = definition
        .as_ref()
        .map(distinct_agent_names)
        .unwrap_or_default();

    WorkflowListResponse {
        id: row.id,
        user_id: row.user_id,
        name: row.name,
        description: row.description,
        maf_json: serde_json::from_str(&row.maf_json).unwrap_or(serde_json::Value::Null),
        status: row.status,
        created_at: row.created_at,
        updated_at: row.updated_at,
        execution_count: row.execution_count,
        success_rate: row.success_rate,
        health: Health::from_success_rate(row.success_rate),
        total_tokens: row.total_tokens,
        last_run_at: row.last_run_at,
        last_run_status: row.last_run_status,
        step_count,
        agent_names,
    }
}

// ─── 1. GET /maf/workflows ─────────────────────────────────────────────────

async fn list_mafs(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<WorkflowListQuery>,
) -> impl IntoResponse {
    let Some(user_id) = parse_user_id(&claims) else {
        return unauthorized();
    };

    let rows = fetch_workflow_page(
        &state.db,
        WorkflowPage {
            user_id,
            scope: ListScope::Active,
            search: normalize_search(q.search.as_deref()),
            order_by: q.sort.order_by(),
            limit: q.limit,
            offset: q.offset,
        },
    )
    .await;

    match rows {
        Ok(data) => {
            let items: Vec<WorkflowListResponse> =
                data.into_iter().map(list_row_to_response).collect();
            ok_json(
                StatusCode::OK,
                crate::Paginated::new(items),
                "Workflows retrieved successfully",
            )
        }
        Err(e) => internal_err(e),
    }
}

/// Resolves a workflow step's auto-assigned agent (no explicit `agent_id`) — shared by
/// `create_maf` and `update_maf` so the rule is encoded once, not twice: routes via the engine,
/// requires a non-empty endpoint, and re-checks access on the result. That re-check is
/// defense-in-depth against `agent_registry::get_agents_for_user`'s candidate query being (or
/// becoming) too permissive — not the primary authorization mechanism, which is that query
/// itself — since unlike the explicit-`agent_id` case, nothing else in this path validates the
/// routing engine's pick before it's used. `None` on any failure (routing error, no endpoint, or
/// access denied) so every caller falls through to the catalog fallback uniformly: an unusable
/// or inaccessible routed candidate is not something the caller asked for by id, so there's
/// nothing to explain to them — just try the next mechanism.
/// `Ok(None)` is an ordinary miss — no route, no endpoint, or a result this caller cannot
/// reach — and the caller falls through to the catalog fallback.
///
/// `Err(reason)` is an explicit refusal by the operator's routing policy, which must NEVER
/// fall through: that fallback exists for "the router could not decide", and using it here
/// would assign the step to an agent the policy just judged unable to do it, reinstating the
/// exact behaviour the policy is there to remove. The two cannot share `None`, which is why
/// this returns a `Result` rather than an `Option`.
async fn route_with_access_check(
    state: &AppState,
    claims: &Claims,
    user_id: Uuid,
    task_description: &str,
    policy: Option<&dyn nasiko_orchestrator::RoutingPolicy>,
) -> Result<Option<(Uuid, String, String)>, String> {
    let route_req = RouteRequest {
        query: task_description.to_string(),
        session_id: Uuid::new_v4().to_string(),
        user_id,
        file_parts: vec![],
    };
    let routed = match state
        .routing_engine
        .route(route_req, &state.db, policy)
        .await
    {
        Ok(result) => match result.agent.url {
            Some(endpoint) if !endpoint.is_empty() => {
                Some((result.agent.id, result.agent.name, endpoint))
            }
            _ => None,
        },
        // Relayed verbatim rather than rephrased: this crate does not know what the policy
        // checked for, so it cannot say what to do about it.
        Err(nasiko_orchestrator::RouterError::PolicyRefused { reason }) => return Err(reason),
        Err(_) => None,
    };
    Ok(match routed {
        Some((id, name, endpoint)) if crate::acl::can_access_agent(state, claims, id).await => {
            Some((id, name, endpoint))
        }
        _ => None,
    })
}

// ─── 2. POST /maf/workflows ────────────────────────────────────────────────

async fn create_maf(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<CreateMafRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(id) => id,
        None => return unauthorized(),
    };

    // A caller that supplies its own steps has already decided what the
    // workflow is, so there is nothing left to review — this creates a live
    // workflow, not a draft.
    create_maf_from_steps(
        &state,
        &claims,
        user_id,
        req.name,
        req.description,
        req.steps,
        false,
    )
    .await
}

/// Shared by `create_maf` (caller gives steps directly) and
/// `create_maf_from_instruction` (steps come from decomposing one sentence).
/// Resolves any step lacking an `agent_id` via the routing engine, then
/// persists the resulting `MafDefinition` as a new `mafs` row.
/// Resolves each step to an agent and persists the workflow.
///
/// `as_draft` decides the status the new row is born with. `false` inserts a
/// live workflow, which is what a caller supplying its own steps gets. `true`
/// inserts a `draft` — the row carries its fully resolved steps and agents from
/// the moment it exists, and all promotion has left to do is flip the status.
/// That is the whole point of resolving here: the plan the user reviews on the
/// draft is the plan that runs, rather than one re-derived later from the same
/// sentence and free to come out different.
async fn create_maf_from_steps(
    state: &AppState,
    claims: &Claims,
    user_id: Uuid,
    name: Option<String>,
    description: Option<String>,
    steps: Vec<CreateStepRequest>,
    as_draft: bool,
) -> axum::response::Response {
    if steps.is_empty() {
        return bad_request("steps must not be empty");
    }

    // Resolve any steps that lack an agent_id via the routing engine. Resolved
    // once here rather than once per step inside route(): the operator's policy
    // is the same for every step of this request.
    let policy = state.orchestrator_policy.routing_policy(&state.db).await;
    let mut resolved_steps: Vec<MafStep> = Vec::with_capacity(steps.len());
    for (idx, step) in steps.into_iter().enumerate() {
        if step.task_description.trim().is_empty() {
            return bad_request(&format!("step {idx}: task_description is required"));
        }
        // The task description is user-authored prose, so it stays out of
        // `info!` — these lines ship to Loki, where anyone with dashboard
        // access can read them. Length is the part that's useful for
        // diagnosing a routing miss; the text itself is available at `debug`.
        tracing::info!(
            step = idx,
            task_description_len = step.task_description.len(),
            has_explicit_agent = step.agent_id.is_some(),
            "maf create: resolving step"
        );
        tracing::debug!(step = idx, task_description = %step.task_description);
        let step_start = std::time::Instant::now();

        let (agent_id, agent_name, agent_endpoint) = if let Some(aid) = step.agent_id {
            // Caller provided an agent — must be reachable by this caller (owner ∪
            // public ∪ user/team/dept grant per edition) before we accept it into a
            // workflow step. Same "not found" response for both missing and
            // inaccessible agents — matches a2a_dispatch.rs's enumeration-safe
            // pattern (a non-grantee can't distinguish "doesn't exist" from
            // "exists but you can't use it").
            if !crate::acl::can_access_agent(state, claims, aid).await {
                return forbidden(&format!("agent {aid} not found"));
            }
            match fetch_agent_info(&state.db, aid).await {
                Ok(Some((name, url))) => (aid, name, url),
                Ok(None) => return forbidden(&format!("agent {aid} not found")),
                Err(e) => return internal_err(e),
            }
        } else {
            // Auto-assign via routing engine. An agent row with an empty `url` (registered but
            // never deployed, or a seed whose URL was never backfilled) used to hard-fail the
            // whole request with a 400, which is what made *every* workflow uncreatable on such
            // a fleet — `route_with_access_check` treats that, a routing failure, and an
            // inaccessible result all the same way: fall through to the catalog fallback below.
            // An explicit policy refusal is the one case that must not: see the helper's docs.
            let routed = match route_with_access_check(
                state,
                claims,
                user_id,
                &step.task_description,
                policy.as_deref(),
            )
            .await
            {
                Ok(routed) => routed,
                Err(reason) => {
                    return bad_request(&format!(
                        "step {idx}: the routing policy refused every agent for this task. {reason}"
                    ));
                }
            };

            match routed {
                Some(agent) => agent,
                None => {
                    // The routing engine only considers status='running' agents.
                    // Fall back to any agent registered by this user that has a valid URL,
                    // picking the one whose name/description best matches the task description.
                    let catalog = match fetch_user_agents(&state.db, user_id).await {
                        Ok(v) => v,
                        Err(e) => return internal_err(e),
                    };
                    let query_lower = step.task_description.to_lowercase();
                    let best = catalog
                        .into_iter()
                        .filter(|a| a.url.as_deref().is_some_and(|u| !u.is_empty()))
                        .max_by_key(|a| {
                            let haystack =
                                format!("{} {}", a.name, a.description.as_deref().unwrap_or(""))
                                    .to_lowercase();
                            query_lower
                                .split_whitespace()
                                .filter(|w| haystack.contains(*w))
                                .count()
                        });
                    match best {
                        Some(a) => (a.id, a.name, a.url.unwrap_or_default()),
                        None => {
                            return bad_request(&format!(
                                "step {idx}: no deployed agent is available to run this step. \
                                 Deploy at least one agent (a registered agent with no running \
                                 container has no endpoint to call) before creating a workflow."
                            ));
                        }
                    }
                }
            }
        };

        tracing::info!(
            step = idx,
            agent_name = %agent_name,
            elapsed_ms = step_start.elapsed().as_millis() as u64,
            "maf create: step resolved"
        );

        resolved_steps.push(MafStep {
            step_id: Uuid::new_v4(),
            step_index: idx as i32,
            agent_id,
            agent_name,
            agent_endpoint,
            task_description: step.task_description,
        });
    }

    // Derive name from first task description if not provided
    let name = name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| derive_workflow_name(&resolved_steps[0].task_description));

    let maf_def = MafDefinition {
        description: None, // generated by the runtime planner on each execution
        steps: resolved_steps,
        output_generation: None, // generated by the runtime planner on each execution
    };
    let maf_json = serde_json::to_value(&maf_def).unwrap_or_default();
    let maf_json_str = maf_json.to_string();
    let description = description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Both forms insert; only the status the row starts in differs. `drafted_at`
    // is set on the draft path because it is what the drafts list selects on,
    // and it is never cleared — a promoted draft stays visible there.
    let sql = if as_draft {
        r#"INSERT INTO mafs (user_id, name, description, maf_json, status, drafted_at)
           VALUES ($1, $2, $3, $4::jsonb, 'draft', now())
           RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                     status, created_at, updated_at, 0::bigint AS execution_count"#
    } else {
        r#"INSERT INTO mafs (user_id, name, description, maf_json)
           VALUES ($1, $2, $3, $4::jsonb)
           RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                     status, created_at, updated_at, 0::bigint AS execution_count"#
    };

    let row = sqlx::query_as::<_, MafRow>(sql)
        .bind(user_id)
        .bind(&name)
        .bind(description)
        .bind(&maf_json_str)
        .fetch_one(&state.db)
        .await;

    match row {
        Ok(r) => {
            tracing::info!(maf_id = %r.id, name = %r.name, status = %r.status, "maf create: workflow persisted");
            let message = if as_draft {
                "Draft created successfully"
            } else {
                "Workflow created successfully"
            };
            ok_json(StatusCode::CREATED, maf_row_to_response(r), message)
        }
        Err(e) => internal_err(e),
    }
}

#[derive(Deserialize)]
struct FromInstructionRequest {
    /// The full compound sentence, e.g. "translate hello to japanese then
    /// email to jordan" — handed to the decomposer as-is.
    instruction: String,
}

// ─── 2b. POST /maf/workflow/from-instruction ───────────────────────────────
//
// Splits one compound instruction into atomic sub-queries via the external
// decomposer service (MODEL_API_URL/MODEL_APIKEY), then resolves each one to an
// agent exactly like `create_maf` — same routing-engine auto-assign per step,
// same persisted `mafs.maf_json` shape. No LLM planner involved.
//
// The row it writes is a **draft**: decomposing a sentence is a guess at what
// the user meant, so the steps land somewhere they can be reviewed before they
// can run. Everything expensive happens here, once. Promotion is then a status
// update and nothing more, and what the user approved is what executes.

async fn create_maf_from_instruction(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<FromInstructionRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(id) => id,
        None => return unauthorized(),
    };

    if req.instruction.trim().is_empty() {
        return bad_request("instruction is required");
    }

    let steps = match decompose_into_steps(&state, &req.instruction).await {
        Ok(steps) => steps,
        Err(response) => return response,
    };

    create_maf_from_steps(
        &state,
        &claims,
        user_id,
        None,
        Some(req.instruction),
        steps,
        true,
    )
    .await
}

/// Splits one compound instruction into per-step requests via the decomposer
/// service, each left unassigned so the routing engine picks its agent.
///
/// The `Err` arm carries the finished response rather than an error type
/// because every failure here is already a decided HTTP outcome — a 503 when
/// the service is unconfigured or unreachable.
async fn decompose_into_steps(
    state: &AppState,
    instruction: &str,
) -> Result<Vec<CreateStepRequest>, axum::response::Response> {
    let Some(decomposer_url) = state.config.decomposer_api_url.clone() else {
        return Err(err_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "MODEL_API_URL is not configured on this server",
        ));
    };
    let decomposer = DecomposerClient::new(
        state.http_client.clone(),
        decomposer_url,
        state.config.decomposer_api_key.clone(),
    );

    // Same reasoning as the per-step log in `create_maf_from_steps`: the raw
    // instruction is user content and does not belong in `info!`.
    tracing::info!(
        instruction_len = instruction.len(),
        "maf create: decomposing instruction"
    );
    tracing::debug!(instruction = %instruction, "maf create: instruction text");
    let decompose_start = std::time::Instant::now();
    let sub_queries = match decomposer.decompose(instruction).await {
        Ok(qs) => qs,
        Err(e) => {
            // A failed dependency is a warning, not routine info. The error
            // carries the decomposer's response body, which can echo the
            // submitted query back — so it stays at `warn` where it is
            // actionable, rather than being emitted on every request.
            tracing::warn!(
                elapsed_ms = decompose_start.elapsed().as_millis() as u64,
                error = %e,
                "maf create: decomposer failed"
            );
            return Err(err_json(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("decomposer: {e}"),
            ));
        }
    };
    tracing::info!(
        elapsed_ms = decompose_start.elapsed().as_millis() as u64,
        sub_query_count = sub_queries.len(),
        "maf create: decomposer returned sub-queries"
    );
    tracing::debug!(sub_queries = ?sub_queries, "maf create: sub-query text");

    Ok(sub_queries
        .into_iter()
        .map(|task_description| CreateStepRequest {
            task_description,
            agent_id: None,
        })
        .collect())
}

/// A display name derived from free text: the first 60 characters.
///
/// Used for both a draft (from the instruction as typed) and a finished
/// workflow (from step 0's task), so the name a draft shows does not jump to
/// something unrecognisable the moment it is promoted.
fn derive_workflow_name(text: &str) -> String {
    text.trim().chars().take(60).collect()
}

// ─── Drafts ────────────────────────────────────────────────────────────────
//
// A draft is a `mafs` row with `status = 'draft'`. The status is a label for
// where the user is with it, not a restriction on what the row can do: a draft
// holds the same steps and the same agents a deployed workflow does, and is
// edited (`update_maf`) and run (`run_workflow`) through exactly the same
// endpoints. Promotion is the user saying "this one is ready", and flips the
// status without touching anything else.
//
// There are two kinds, differing only in whether their steps exist yet.
//
// `from-instruction` writes the useful kind: a decomposed instruction with an
// agent resolved for every step. It is runnable the moment it is written.
//
// `save_draft` below writes the other kind — the sentence a user has typed but
// not yet decomposed, with an empty step list. It makes no decomposer call,
// runs no routing and bills nothing, so it can be called on every keystroke
// pause and an abandoned draft only ever costs a row. It is a text box that
// survives closing the tab. Running or promoting one is refused for the same
// reason in both places: with no steps it would report success having done
// nothing. Sending its instruction to `from-instruction` gives it steps.
//
// Drafts of either kind are invisible to `list_mafs` (it filters
// `status = 'active'`), so a half-written idea never appears among deployed
// workflows. Both appear in `list_drafts`, which selects on `drafted_at`.

#[derive(Deserialize)]
struct SaveDraftRequest {
    /// The instruction as typed so far.
    instruction: String,
    /// The draft to overwrite. Omitted on the first save; the response echoes
    /// the id back so the client keeps updating one row rather than creating a
    /// new draft on every autosave.
    #[serde(default)]
    draft_id: Option<Uuid>,
}

// ─── 2c. POST /maf/workflow/draft ──────────────────────────────────────────

async fn save_draft(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<SaveDraftRequest>,
) -> impl IntoResponse {
    let Some(user_id) = parse_user_id(&claims) else {
        return unauthorized();
    };

    let instruction = req.instruction.trim();
    if instruction.is_empty() {
        return bad_request("instruction is required");
    }
    let name = derive_workflow_name(instruction);

    // Both branches scope the write to the caller, so one user can never
    // overwrite another's draft by guessing an id.
    let row = match req.draft_id {
        Some(draft_id) => {
            sqlx::query_as::<_, MafRow>(
                r#"UPDATE mafs
                      SET name = $2, description = $3, updated_at = now()
                    WHERE id = $4 AND user_id = $1 AND status = 'draft'
                RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                          status, created_at, updated_at, 0::bigint AS execution_count"#,
            )
            .bind(user_id)
            .bind(&name)
            .bind(instruction)
            .bind(draft_id)
            .fetch_optional(&state.db)
            .await
        }
        None => {
            sqlx::query_as::<_, MafRow>(
                r#"INSERT INTO mafs (user_id, name, description, maf_json, status, drafted_at)
                   VALUES ($1, $2, $3, '{"steps": []}'::jsonb, 'draft', now())
                   RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                             status, created_at, updated_at, 0::bigint AS execution_count"#,
            )
            .bind(user_id)
            .bind(&name)
            .bind(instruction)
            .fetch_optional(&state.db)
            .await
        }
    };

    match row {
        Ok(Some(r)) => ok_json(StatusCode::OK, maf_row_to_response(r), "Draft saved"),
        // The update matched nothing: the draft was deleted or already
        // promoted. Saying so lets the client drop its stale id and save again
        // as a new draft rather than silently losing the user's text.
        Ok(None) => not_found("draft"),
        Err(e) => internal_err(e),
    }
}

// ─── 2d. GET /maf/workflow/drafts ──────────────────────────────────────────

async fn list_drafts(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<DraftListQuery>,
) -> impl IntoResponse {
    let Some(user_id) = parse_user_id(&claims) else {
        return unauthorized();
    };

    let rows = fetch_workflow_page(
        &state.db,
        WorkflowPage {
            user_id,
            scope: ListScope::Drafted,
            search: normalize_search(q.search.as_deref()),
            order_by: q.sort.order_by(),
            limit: q.limit,
            offset: q.offset,
        },
    )
    .await;

    match rows {
        Ok(data) => {
            let items: Vec<WorkflowListResponse> =
                data.into_iter().map(list_row_to_response).collect();
            ok_json(
                StatusCode::OK,
                crate::Paginated::new(items),
                "Drafts retrieved successfully",
            )
        }
        Err(e) => internal_err(e),
    }
}

// ─── 2e. POST /maf/workflow/{id}/promote ───────────────────────────────────
//
// Deploys a draft by flipping its status, and nothing else. It does not make
// the workflow runnable — a draft with steps already was — it records that the
// user is done reviewing, which is what moves the row out of the drafts view
// and into `list_mafs`.
//
// The steps and their agents were resolved when `from-instruction` created the
// draft and are left exactly as they were reviewed: promoting does not
// decompose the instruction again, does not re-run routing, and cannot hand
// back a different plan than the one that was approved.
//
// The row keeps its id, so any link to the draft stays valid, and a promotion
// retried after a dropped response is a no-op rather than a duplicate.

async fn promote_draft(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let Some(user_id) = parse_user_id(&claims) else {
        return unauthorized();
    };

    // Read first so each way this can fail gets its own answer. The UPDATE
    // below folds "gone", "someone else's" and "not a draft" into one empty
    // result, and a caller that sees only a 404 can't tell which happened.
    let draft = match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id == user_id => r,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found("draft"),
        Err(e) => return internal_err(e),
    };

    if draft.status != "draft" {
        return bad_request(&format!(
            "workflow is already '{}' — only a draft can be promoted",
            draft.status
        ));
    }

    // A draft saved by `POST /maf/workflow/draft` is just the sentence a user
    // typed — it has no steps, and promotion no longer supplies any. Left
    // through, it would become an active workflow whose runs iterate over an
    // empty step list: no error anywhere, just executions that quietly produce
    // nothing. Refusing here names the call that gives the draft its steps.
    let has_steps = serde_json::from_str::<MafDefinition>(&draft.maf_json)
        .map(|def| !def.steps.is_empty())
        .unwrap_or(false);
    if !has_steps {
        return bad_request(
            "this draft has no steps to run — create it with \
             POST /api/maf/workflow/from-instruction, which decomposes the \
             instruction and assigns an agent to each step",
        );
    }

    let row = sqlx::query_as::<_, MafRow>(
        r#"UPDATE mafs
              SET status = 'active', updated_at = now()
            WHERE id = $1 AND user_id = $2 AND status = 'draft'
        RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                  status, created_at, updated_at,
                  (SELECT COUNT(*) FROM maf_executions e WHERE e.maf_id = mafs.id)
                      AS execution_count"#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => {
            tracing::info!(maf_id = %r.id, "maf promote: draft is now active");
            ok_json(
                StatusCode::OK,
                maf_row_to_response(r),
                "Draft promoted successfully",
            )
        }
        // The row changed between the read above and this write — a concurrent
        // promotion or delete. Neither is a server fault.
        Ok(None) => not_found("draft"),
        Err(e) => internal_err(e),
    }
}

// ─── 3. GET /maf/workflow/{id} ─────────────────────────────────────────────

async fn get_maf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match fetch_maf(&state.db, id).await {
        Ok(Some(row)) if row.user_id == user_id => ok_json(
            StatusCode::OK,
            maf_row_to_response(row),
            "Workflow retrieved successfully",
        ),
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found("workflow"),
        Err(e) => internal_err(e),
    }
}

// ─── 4. PUT /maf/workflow/{id} ─────────────────────────────────────────────
//
// Edits a draft as readily as a deployed workflow. A draft is the row a user is
// still working on, so refusing to edit it would leave the one state that most
// needs editing as the only one that cannot be — and the way to fix a step the
// decomposer got wrong would be to deploy the workflow first. Only a deleted
// row is off limits.

async fn update_maf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
    Json(req): Json<UpdateMafRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let existing = match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id == user_id => r,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found("workflow"),
        Err(e) => return internal_err(e),
    };

    // Build new maf_json if steps are being replaced
    let new_maf_json_str = if let Some(steps) = req.steps {
        if steps.is_empty() {
            return bad_request("steps must not be empty");
        }

        // Resolved once here rather than once per step inside route() — see
        // create_maf's own note.
        let policy = state.orchestrator_policy.routing_policy(&state.db).await;
        let mut resolved: Vec<MafStep> = Vec::with_capacity(steps.len());
        for (idx, step) in steps.iter().enumerate() {
            if step.task_description.trim().is_empty() {
                return bad_request(&format!("step {idx}: task_description is required"));
            }

            let (agent_id, name, endpoint) = if let Some(aid) = step.agent_id {
                // Same access check as create_maf — a caller-supplied agent_id must
                // be reachable by this caller before it's accepted into a step.
                if !crate::acl::can_access_agent(&state, &claims, aid).await {
                    return forbidden(&format!("agent {aid} not found"));
                }
                match fetch_agent_info(&state.db, aid).await {
                    Ok(Some((n, u))) => (aid, n, u),
                    Ok(None) => return forbidden(&format!("agent {aid} not found")),
                    Err(e) => return internal_err(e),
                }
            } else {
                // Auto-assign via routing engine — same helper as create_maf, so this stays in
                // sync with it (this branch used to hard-400 on an empty endpoint instead of
                // falling through to the catalog fallback like create_maf does, and never
                // re-checked access on the routed result at all; both are now the same code).
                // A policy refusal is propagated, not fallen back on — see the helper's docs.
                let routed = match route_with_access_check(
                    &state,
                    &claims,
                    user_id,
                    &step.task_description,
                    policy.as_deref(),
                )
                .await
                {
                    Ok(routed) => routed,
                    Err(reason) => {
                        return bad_request(&format!(
                            "step {idx}: the routing policy refused every agent for this task. \
                             {reason}"
                        ));
                    }
                };
                match routed {
                    Some(agent) => agent,
                    None => {
                        let catalog = match fetch_user_agents(&state.db, user_id).await {
                            Ok(v) => v,
                            Err(e) => return internal_err(e),
                        };
                        let query_lower = step.task_description.to_lowercase();
                        let best = catalog
                            .into_iter()
                            .filter(|a| a.url.as_deref().is_some_and(|u| !u.is_empty()))
                            .max_by_key(|a| {
                                let haystack = format!(
                                    "{} {}",
                                    a.name,
                                    a.description.as_deref().unwrap_or("")
                                )
                                .to_lowercase();
                                query_lower
                                    .split_whitespace()
                                    .filter(|w| haystack.contains(*w))
                                    .count()
                            });
                        match best {
                            Some(a) => (a.id, a.name, a.url.unwrap_or_default()),
                            None => {
                                return bad_request(&format!(
                                    "step {idx}: no agents available. Register at least one agent in the Agents page."
                                ));
                            }
                        }
                    }
                }
            };

            resolved.push(MafStep {
                step_id: Uuid::new_v4(),
                step_index: idx as i32,
                agent_id,
                agent_name: name,
                agent_endpoint: endpoint,
                task_description: step.task_description.clone(),
            });
        }
        // Preserve description and output_generation from the existing maf_json when replacing steps
        let existing_def: MafDefinition =
            serde_json::from_str(&existing.maf_json).unwrap_or(MafDefinition {
                description: None,
                steps: vec![],
                output_generation: None,
            });
        let def = MafDefinition {
            description: existing_def.description,
            steps: resolved,
            output_generation: existing_def.output_generation,
        };
        serde_json::to_value(&def).unwrap_or_default().to_string()
    } else {
        existing.maf_json.clone()
    };

    let new_name = req.name.as_deref().map(str::trim).unwrap_or(&existing.name);
    // Some(None) = explicit null in JSON → clear; Some(Some(v)) = new value; None = absent → keep
    let new_description: Option<&str> = match &req.description {
        Some(inner) => inner.as_deref(),
        None => existing.description.as_deref(),
    };

    let row = sqlx::query_as::<_, MafRow>(
        r#"UPDATE mafs
           SET name = $1, description = $2, maf_json = $3::jsonb, updated_at = now()
           WHERE id = $4 AND status <> 'deleted'
           RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                     status, created_at, updated_at,
                     (SELECT COUNT(*) FROM maf_executions e WHERE e.maf_id = mafs.id) AS execution_count"#,
    )
    .bind(new_name)
    .bind(new_description)
    .bind(&new_maf_json_str)
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => ok_json(
            StatusCode::OK,
            maf_row_to_response(r),
            "Workflow updated successfully",
        ),
        Ok(None) => not_found("workflow"),
        Err(e) => internal_err(e),
    }
}

// ─── 5. DELETE /maf/workflow/{id} ─────────────────────────────────────────

async fn delete_maf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Ownership check before soft-delete — superuser may delete any workflow (same
    // owner-or-superuser convention as agent management elsewhere).
    match fetch_maf(&state.db, id).await {
        Ok(Some(row)) if row.user_id != user_id && !claims.is_superuser => {
            return forbidden("not owned by caller");
        }
        Ok(None) => return not_found("workflow"),
        Err(e) => return internal_err(e),
        Ok(Some(_)) => {}
    }

    // `<> 'deleted'` rather than `= 'active'` so discarding a draft works too,
    // while a second delete of an already-deleted row stays a no-op.
    match sqlx::query(
        "UPDATE mafs SET status = 'deleted', updated_at = now() WHERE id = $1 AND status <> 'deleted'",
    )
    .bind(id)
    .execute(&state.db)
    .await
    {
        // 204 No Content can't legally carry a body, so a successful delete
        // now returns 200 with the same envelope as every other response.
        Ok(r) if r.rows_affected() > 0 => {
            ok_json(StatusCode::OK, serde_json::Value::Null, "Workflow deleted successfully")
        }
        Ok(_) => not_found("workflow"),
        Err(e) => internal_err(e),
    }
}

#[derive(Deserialize, Default)]
struct RunWorkflowRequest {
    /// Data for this run only — spliced into step 0's task description
    /// (see `nasiko_orchestrator::maf::executor::run_maf`) before planning,
    /// so the same saved workflow shape can be re-run with different input
    /// each time instead of baking content in at creation.
    #[serde(default)]
    content: Option<String>,
}

// ─── 6. POST /maf/workflow/{id}/run ───────────────────────────────────────
//
// Status is not a gate here. A draft and a deployed workflow are the same row
// holding the same steps and the same agents; `status` records which one the
// user has blessed, not whether it is capable of running. Refusing to run a
// draft would mean refusing to run a workflow that is ready — and would make
// trying one out impossible without first committing to it, which is backwards.
//
// What a run does require is steps to run. That is checked below, on the
// definition itself rather than on the status, so it catches every row that
// cannot produce work regardless of how it got that way.

async fn run_workflow(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Body is optional — existing callers post none at all, so an empty body
    // means "no run-time content", not a parse error.
    let content = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<RunWorkflowRequest>(&body) {
            Ok(r) => r.content,
            Err(e) => return bad_request(&format!("invalid request body: {e}")),
        }
    };

    let maf = match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id == user_id => r,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found("workflow"),
        Err(e) => return internal_err(e),
    };

    // Re-check agent access at run time, not just at create/update time.
    //
    // `create_maf`/`update_maf` already gate every step's agent, but those
    // checks are only true as of the moment the workflow was saved. A grant
    // can be revoked, an agent's `is_public` flag flipped off, or the agent
    // soft-deleted at any point afterwards — and the saved workflow would keep
    // invoking it, because the run path never looked again. That turns a
    // stored workflow into a durable capability that outlives the permission
    // it was built on.
    //
    // Checked here rather than in the worker so the caller gets a synchronous
    // 403 instead of an execution row that fails asynchronously. Agent ids are
    // de-duplicated: a workflow may use the same agent in several steps, and
    // each check is a DB round trip.
    match serde_json::from_str::<MafDefinition>(&maf.maf_json) {
        Ok(def) => {
            // Enqueuing a stepless workflow would create an execution that
            // iterates nothing and reports success several seconds later, out
            // of sight of this caller. The rows this catches are the ones
            // `POST /maf/workflow/draft` writes — a typed sentence that has not
            // been decomposed yet — so the message names what turns one into a
            // workflow with steps.
            if def.steps.is_empty() {
                return bad_request(
                    "this workflow has no steps to run — send its instruction to \
                     POST /api/maf/workflow/from-instruction, which decomposes it \
                     and assigns an agent to each step",
                );
            }

            let mut checked: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
            for step in &def.steps {
                if !checked.insert(step.agent_id) {
                    continue;
                }
                if !crate::acl::can_access_agent(&state, &claims, step.agent_id).await {
                    return forbidden(&format!(
                        "step {}: agent '{}' is no longer accessible to you",
                        step.step_index, step.agent_name
                    ));
                }
            }
        }
        // A workflow row whose JSON no longer parses can't be run at all, and
        // failing closed here is what keeps the ACL check from being
        // bypassable by storing malformed JSON.
        Err(e) => return bad_request(&format!("workflow definition is invalid: {e}")),
    }

    let max_attempts: i32 = std::env::var("MAF_MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    // Create execution record. `maf_json` durably captures the exact snapshot this run executes
    // against — needed so a HITL resume can carry the SAME snapshot forward without re-fetching
    // the mutable `mafs.maf_json`, which may have changed since. The in-flight Redis message below
    // carries the identical string for the worker's normal, non-resume path — unchanged.
    let (exec_id, exec_number): (Uuid, i64) = match sqlx::query_as(
        r#"INSERT INTO maf_executions (maf_id, user_id, status, max_attempts, maf_json)
           VALUES ($1, $2, 'pending', $3, $4::jsonb)
           RETURNING id, execution_number"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(max_attempts)
    .bind(&maf.maf_json)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => return internal_err(e),
    };

    // Enqueue to Redis stream
    let mut redis_conn = match state.redis.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => return internal_err(format!("redis connection failed: {e}")),
    };

    let mut xadd = redis::cmd("XADD");
    xadd.arg(nasiko_orchestrator::maf::STREAM_KEY)
        .arg("*")
        .arg("execution_id")
        .arg(exec_id.to_string())
        .arg("maf_json")
        .arg(&maf.maf_json)
        .arg("user_id")
        .arg(user_id.to_string());
    // Omit the field entirely when there's no run-time content, rather than
    // writing an empty string — keeps worker.rs's parse_job()/Job.content
    // distinguishing "no content given" from "content given but empty".
    if let Some(content) = &content {
        xadd.arg("content").arg(content);
    }
    let enqueue: redis::RedisResult<String> = xadd.query_async(&mut redis_conn).await;

    if let Err(e) = enqueue {
        // Roll back the execution row so the caller knows it wasn't queued
        let _ = sqlx::query("DELETE FROM maf_executions WHERE id = $1")
            .bind(exec_id)
            .execute(&state.db)
            .await;
        return internal_err(format!("failed to enqueue job: {e}"));
    }

    // Fresh count including the execution just created, so the caller can
    // update its UI immediately without a separate re-fetch.
    let execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM maf_executions WHERE maf_id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);

    ok_json(
        StatusCode::ACCEPTED,
        serde_json::json!({
            "execution_id": exec_id,
            "execution_number": exec_number,
            "execution_count": execution_count,
        }),
        "Execution started successfully",
    )
}

// ─── 7. GET /maf/workflow/result/{exec_id} ────────────────────────────────

async fn get_result(
    State(state): State<AppState>,
    Path(exec_id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match fetch_exec(&state.db, exec_id).await {
        Ok(Some(row)) if row.user_id == user_id => {
            let exec = exec_row_to_response(row);
            match hitl_rows_for_execution(&state.hitl_store, exec_id, user_id).await {
                Ok(hitl) => ok_json(
                    StatusCode::OK,
                    ExecWithHitlResponse { exec, hitl },
                    "Execution result retrieved successfully",
                ),
                Err(e) => internal_err(e),
            }
        }
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found("execution"),
        Err(e) => internal_err(e),
    }
}

// ─── 8. GET /maf/workflow/{id}/executions ────────────────────────────────

async fn list_executions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Verify ownership of the MAF first
    match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id != user_id => return forbidden("not owned by caller"),
        Ok(None) => return not_found("workflow"),
        Err(e) => return internal_err(e),
        Ok(Some(_)) => {}
    }

    let rows = sqlx::query_as::<_, ExecRow>(
        r#"SELECT id, execution_number, maf_id, user_id, status, attempt_count, max_attempts, tokens_used,
                  started_at, completed_at, duration_ms, output,
                  step_results::text AS step_results, error, created_at
           FROM maf_executions
           WHERE maf_id = $1 AND user_id = $2
           ORDER BY created_at DESC
           LIMIT $3 OFFSET $4"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(q.limit.min(50))
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    let data = match rows {
        Ok(data) => data,
        Err(e) => return internal_err(e),
    };

    let exec_ids: Vec<Uuid> = data.iter().map(|r| r.id).collect();
    let mut by_exec = match hitl_rows_for_executions(&state.hitl_store, &exec_ids, user_id).await {
        Ok(map) => map,
        Err(e) => return internal_err(e),
    };

    let items: Vec<ExecWithHitlResponse<ExecResponse>> = data
        .into_iter()
        .map(|row| {
            let hitl = by_exec.remove(&row.id).unwrap_or_default();
            ExecWithHitlResponse {
                exec: exec_row_to_response(row),
                hitl,
            }
        })
        .collect();
    ok_json(
        StatusCode::OK,
        crate::Paginated::new(items),
        "Executions retrieved successfully",
    )
}

// ─── 8b. GET /maf/executions ──────────────────────────────────────────────
// Every execution the caller has ever run, across every workflow — unlike
// list_executions (scoped to one workflow, and 404s once that workflow is
// deleted since it gates through fetch_maf's active-only check), this queries
// maf_executions directly by user_id, so a deleted workflow's runs still show
// up here. workflow_status tells the caller which ones are for workflows that
// no longer exist in the active list.

async fn list_all_executions(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let rows = sqlx::query_as::<_, ExecWithWorkflowRow>(
        r#"SELECT e.id, e.execution_number, e.maf_id, e.user_id, e.status, e.attempt_count,
                  e.max_attempts, e.tokens_used, e.started_at, e.completed_at, e.duration_ms,
                  e.output, e.step_results::text AS step_results, e.error, e.created_at,
                  m.name AS workflow_name, m.status AS workflow_status
           FROM maf_executions e
           LEFT JOIN mafs m ON m.id = e.maf_id
           WHERE e.user_id = $1
           ORDER BY e.created_at DESC
           LIMIT $2 OFFSET $3"#,
    )
    .bind(user_id)
    .bind(q.limit.min(50))
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    let data = match rows {
        Ok(data) => data,
        Err(e) => return internal_err(e),
    };

    let exec_ids: Vec<Uuid> = data.iter().map(|r| r.id).collect();
    let mut by_exec = match hitl_rows_for_executions(&state.hitl_store, &exec_ids, user_id).await {
        Ok(map) => map,
        Err(e) => return internal_err(e),
    };

    let items: Vec<ExecWithHitlResponse<ExecWithWorkflowResponse>> = data
        .into_iter()
        .map(|row| {
            let hitl = by_exec.remove(&row.id).unwrap_or_default();
            ExecWithHitlResponse {
                exec: exec_with_workflow_row_to_response(row),
                hitl,
            }
        })
        .collect();
    ok_json(
        StatusCode::OK,
        crate::Paginated::new(items),
        "Executions retrieved successfully",
    )
}

// ─── 9. GET /maf/execution/{id} ──────────────────────────────────────────

async fn get_execution(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    match fetch_exec(&state.db, id).await {
        Ok(Some(row)) if row.user_id == user_id => {
            let exec = exec_row_to_response(row);
            match hitl_rows_for_execution(&state.hitl_store, id, user_id).await {
                Ok(hitl) => ok_json(
                    StatusCode::OK,
                    ExecWithHitlResponse { exec, hitl },
                    "Execution retrieved successfully",
                ),
                Err(e) => internal_err(e),
            }
        }
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found("execution"),
        Err(e) => internal_err(e),
    }
}

// ─── 10. GET /maf/execution/{id}/usage ────────────────────────────────────
//
// Agent-side token and cost figures for one execution.
//
// These are deliberately NOT gathered while the workflow runs — see
// `nasiko_orchestrator::maf::executor::run_maf`. Agents flush their
// `gen_ai.usage` spans on a batch timer (~5s), so reading them inline meant
// every step sat idle for up to 10s producing a number that nothing in the
// run consumes. Instead each step records the trace id it ran under, the
// trace-usage materializer folds those spans into `trace_usage`, and this
// endpoint joins the two back together on demand.
//
// The consequence a caller must handle: usage lands *after* the execution
// does. `complete` reports whether there is anything left to wait for, so the
// UI can poll this endpoint on its own schedule and fill the numbers in when
// they arrive.

/// One step's agent usage, summed over every agent that reported spans under
/// that step's trace.
///
/// A MAF step is a single agent call, but that agent may itself fan out to
/// sub-agents on the same trace, and `trace_usage` stores one row per
/// `(trace_id, agent_name)`. Summing is what makes the figure the *step's*
/// true cost rather than just the entry agent's.
#[derive(sqlx::FromRow)]
struct TraceUsageRollup {
    trace_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd: f64,
    /// Only set when every agent on the trace reported the same model —
    /// otherwise there is no single honest answer, so it stays null rather
    /// than arbitrarily picking one.
    model: Option<String>,
}

#[derive(Serialize)]
struct StepUsage {
    step_index: i32,
    agent_name: String,
    /// Null for a step that never got as far as its agent call.
    trace_id: Option<String>,
    /// False when this step's spans have not been materialized yet. Every
    /// figure below is zero in that case — a zero on an unresolved step means
    /// "not known yet", NOT "cost nothing". Callers must not sum across
    /// unresolved steps and present the result as a total.
    resolved: bool,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    model: Option<String>,
    cost_usd: f64,
    /// MAF's own planning / placeholder-fill / extraction tokens for this
    /// step. Unlike the agent figures these *are* metered inline and stored on
    /// the execution, so they are correct the moment the step finishes.
    maf_tokens: i64,
    latency_ms: i64,
}

#[derive(Serialize)]
struct UsageTotals {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    /// input + output across resolved steps only.
    agent_tokens: i64,
    /// MAF's own reasoning tokens across all steps, plus planning and final
    /// synthesis — i.e. `maf_executions.tokens_used`.
    maf_tokens: i64,
    cost_usd: f64,
}

#[derive(Serialize)]
struct ExecutionUsageResponse {
    execution_id: Uuid,
    /// The execution's own status, so a caller polling only this endpoint can
    /// tell a still-running workflow from a finished one.
    status: String,
    /// Nothing further to wait for: either every step resolved, or the
    /// execution finished long enough ago that anything still missing is not
    /// coming (an agent that made no LLM calls at all never produces a
    /// `trace_usage` row, so this must be bounded by time, not just by count).
    complete: bool,
    /// False when the trace-usage materializer isn't running on this
    /// deployment — either no observability backend is configured
    /// (`TEMPO_URL` unset) or the sync is switched off
    /// (`TRACE_USAGE_SYNC_SECS=0`). Agent usage never arrives in that case and
    /// every step stays unresolved forever, so this distinguishes "this
    /// deployment doesn't collect it" from "not ready yet" — without it a
    /// polling client could not tell the two apart.
    usage_available: bool,
    unresolved_steps: usize,
    steps: Vec<StepUsage>,
    totals: UsageTotals,
}

async fn get_execution_usage(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let row = match fetch_exec(&state.db, id).await {
        Ok(Some(row)) if row.user_id == user_id => row,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found("execution"),
        Err(e) => return internal_err(e),
    };

    let step_results: Vec<StepResult> = row
        .step_results
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // One batched lookup for every step's trace, rather than a query per step.
    let trace_ids: Vec<String> = step_results
        .iter()
        .filter_map(|s| s.trace_id.clone())
        .collect();

    let rollups: Vec<TraceUsageRollup> = if trace_ids.is_empty() {
        Vec::new()
    } else {
        match sqlx::query_as::<_, TraceUsageRollup>(
            r#"SELECT trace_id,
                      COALESCE(SUM(input_tokens), 0)::BIGINT          AS input_tokens,
                      COALESCE(SUM(output_tokens), 0)::BIGINT         AS output_tokens,
                      COALESCE(SUM(cache_read_tokens), 0)::BIGINT     AS cache_read_tokens,
                      COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                      COALESCE(SUM(cost_usd), 0)::DOUBLE PRECISION    AS cost_usd,
                      CASE WHEN COUNT(DISTINCT model) = 1 THEN MIN(model) END AS model
               FROM trace_usage
               WHERE trace_id = ANY($1)
               GROUP BY trace_id"#,
        )
        .bind(&trace_ids)
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows,
            Err(e) => return internal_err(e),
        }
    };

    let by_trace: std::collections::HashMap<&str, &TraceUsageRollup> =
        rollups.iter().map(|r| (r.trace_id.as_str(), r)).collect();

    let mut totals = UsageTotals {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        agent_tokens: 0,
        maf_tokens: row.tokens_used,
        cost_usd: 0.0,
    };

    let steps: Vec<StepUsage> = step_results
        .iter()
        .map(|s| {
            let usage = s.trace_id.as_deref().and_then(|t| by_trace.get(t).copied());
            match usage {
                Some(u) => {
                    totals.input_tokens += u.input_tokens;
                    totals.output_tokens += u.output_tokens;
                    totals.cache_read_tokens += u.cache_read_tokens;
                    totals.cache_creation_tokens += u.cache_creation_tokens;
                    totals.cost_usd += u.cost_usd;
                    StepUsage {
                        step_index: s.step_index,
                        agent_name: s.agent_name.clone(),
                        trace_id: s.trace_id.clone(),
                        resolved: true,
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read_tokens: u.cache_read_tokens,
                        cache_creation_tokens: u.cache_creation_tokens,
                        model: u.model.clone(),
                        cost_usd: u.cost_usd,
                        maf_tokens: s.tokens_used,
                        latency_ms: s.latency_ms,
                    }
                }
                None => StepUsage {
                    step_index: s.step_index,
                    agent_name: s.agent_name.clone(),
                    trace_id: s.trace_id.clone(),
                    resolved: false,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    model: None,
                    cost_usd: 0.0,
                    maf_tokens: s.tokens_used,
                    latency_ms: s.latency_ms,
                },
            }
        })
        .collect();

    totals.agent_tokens = totals.input_tokens + totals.output_tokens;

    let unresolved_steps = steps.iter().filter(|s| !s.resolved).count();
    // Must mirror the condition the materializer is actually spawned under
    // (`state.rs`) — gating on the interval alone would report usage as
    // "coming" on a deployment with no observability backend at all.
    let usage_available =
        state.config.observability_enabled && state.config.trace_usage_sync_secs > 0;
    let terminal = matches!(row.status.as_str(), "success" | "failed");

    // Two full materializer passes after the run ended is the point past
    // which anything still missing isn't arriving — most often because the
    // step's agent made no LLM calls, which produces no `trace_usage` row at
    // all and would otherwise keep a polling client going forever.
    let grace = chrono::Duration::seconds((state.config.trace_usage_sync_secs as i64) * 2);
    let settled = row
        .completed_at
        .is_some_and(|finished| Utc::now() - finished > grace);

    let complete = !usage_available || (terminal && (unresolved_steps == 0 || settled));

    ok_json(
        StatusCode::OK,
        ExecutionUsageResponse {
            execution_id: row.id,
            status: row.status,
            complete,
            usage_available,
            unresolved_steps,
            steps,
            totals,
        },
        "Execution usage retrieved successfully",
    )
}

// ─── POST /maf/generate ───────────────────────────────────────────────────
// Takes a natural language description, uses LLM to plan the MAF steps
// (agent selection, prompt templates, to_extract labels, output_generation guidelines),
// and returns a ready-to-POST draft that the caller can review then create.

#[derive(Deserialize)]
struct GenerateMafRequest {
    description: String,
}

#[derive(Serialize)]
struct GeneratedStep {
    agent_id: Uuid,
    agent_name: String,
    task_description: String,
}

#[derive(Serialize)]
struct GenerateMafResponse {
    name: String,
    description: String,
    output_generation: String,
    steps: Vec<GeneratedStep>,
}

async fn generate_maf(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<GenerateMafRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return unauthorized(),
    };

    if req.description.trim().is_empty() {
        return bad_request("description is required");
    }

    // Build LLM client from config — require an API key
    let api_key = match &state.config.openai_api_key {
        Some(k) => k.clone(),
        None => {
            return err_json(
                StatusCode::SERVICE_UNAVAILABLE,
                "OPENAI_API_KEY is not configured on this server",
            );
        }
    };
    let llm = LlmClient::new(
        state.http_client.clone(),
        api_key,
        state.config.openai_base_url.clone(),
        state.config.openai_model.clone(),
    );

    // Fetch all agents visible to this user
    let agent_rows = match fetch_user_agents(&state.db, user_id).await {
        Ok(a) => a,
        Err(e) => return internal_err(e),
    };

    if agent_rows.is_empty() {
        return bad_request(
            "no agents registered — register at least one agent before generating a MAF",
        );
    }

    let planner_agents: Vec<PlannerAgentInfo> = agent_rows
        .iter()
        .map(|a| PlannerAgentInfo {
            id: a.id,
            name: a.name.clone(),
            description: a.description.clone(),
        })
        .collect();

    match planner::plan_maf(&req.description, &planner_agents, &llm).await {
        Ok(plan) => {
            // Enrich steps with agent names for the response
            let steps: Vec<GeneratedStep> = plan
                .steps
                .into_iter()
                .map(|s| {
                    let name = agent_rows
                        .iter()
                        .find(|a| a.id == s.agent_id)
                        .map(|a| a.name.clone())
                        .unwrap_or_default();
                    GeneratedStep {
                        agent_id: s.agent_id,
                        agent_name: name,
                        task_description: s.task_description,
                    }
                })
                .collect();

            ok_json(
                StatusCode::OK,
                GenerateMafResponse {
                    name: plan.name,
                    description: plan.description,
                    output_generation: plan.output_generation,
                    steps,
                },
                "Workflow plan generated successfully",
            )
        }
        Err(e) => err_json(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("planning failed: {e}"),
        ),
    }
}

// ─── DB helpers ────────────────────────────────────────────────────────────

/// One workflow by id, in any live state.
///
/// Admits drafts as well as active workflows — a draft is a real row its owner
/// can fetch, edit, promote and discard; only the run path treats it specially.
/// Soft-deleted rows stay excluded, so a deleted workflow is still a 404
/// everywhere.
async fn fetch_maf(db: &sqlx::PgPool, id: Uuid) -> Result<Option<MafRow>, sqlx::Error> {
    sqlx::query_as::<_, MafRow>(
        r#"SELECT m.id, m.user_id, m.name, m.description, m.maf_json::text AS maf_json,
                  m.status, m.created_at, m.updated_at,
                  (SELECT COUNT(*) FROM maf_executions e WHERE e.maf_id = m.id) AS execution_count
           FROM mafs m WHERE m.id = $1 AND m.status <> 'deleted'"#,
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

async fn fetch_exec(db: &sqlx::PgPool, id: Uuid) -> Result<Option<ExecRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecRow>(
        r#"SELECT id, execution_number, maf_id, user_id, status, attempt_count, max_attempts, tokens_used,
                  started_at, completed_at, duration_ms, output,
                  step_results::text AS step_results, error, created_at
           FROM maf_executions WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

async fn fetch_agent_info(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT name, url FROM agents WHERE id = $1 AND deleted_at IS NULL ORDER BY name",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .map(|opt| opt.map(|(name, url)| (name, url.unwrap_or_default())))
}

#[derive(sqlx::FromRow)]
struct AgentInfo {
    id: Uuid,
    name: String,
    url: Option<String>,
    description: Option<String>,
}

async fn fetch_user_agents(
    db: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Vec<AgentInfo>, sqlx::Error> {
    sqlx::query_as::<_, AgentInfo>(
        "SELECT id, name, url, description FROM agents WHERE owner_id = $1 AND deleted_at IS NULL ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The create screen sends the same step shape to both saves. `step_index`
    /// was required here and ignored, so "Save as draft and test" — which is a
    /// POST /draft followed by this PUT — 422'd on every draft that had steps.
    #[test]
    fn update_request_accepts_steps_without_a_step_index() {
        let body = r#"{"name":"W","steps":[{"task_description":"do a thing"}]}"#;
        let req: UpdateMafRequest = serde_json::from_str(body).expect("steps without step_index");
        assert_eq!(req.steps.expect("steps").len(), 1);

        // A client that still sends the old shape is not broken by dropping it.
        let legacy = r#"{"steps":[{"step_index":0,"task_description":"do a thing"}]}"#;
        let req: UpdateMafRequest = serde_json::from_str(legacy).expect("legacy step_index");
        assert_eq!(req.steps.expect("steps")[0].task_description, "do a thing");
    }

    fn exec_with_workflow(id: Uuid) -> ExecWithWorkflowResponse {
        ExecWithWorkflowResponse {
            id,
            execution_number: 7,
            maf_id: None,
            user_id: Uuid::nil(),
            status: "success".to_string(),
            attempt_count: 1,
            max_attempts: 3,
            tokens_used: 365,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            output: None,
            step_results: None,
            error: None,
            created_at: Utc::now(),
            workflow_name: None,
            workflow_status: None,
        }
    }

    /// A list row is the exec it always was, plus `hitl`. The flatten is what
    /// makes it additive, so a client reading `status`/`step_results` off a list
    /// row keeps working — and the test is here because losing the flatten would
    /// nest every one of those fields under `exec` without failing to compile.
    #[test]
    fn a_list_row_carries_hitl_without_moving_anything_else() {
        let body = serde_json::to_value(ExecWithHitlResponse {
            exec: exec_with_workflow(Uuid::nil()),
            hitl: vec![serde_json::json!({ "id": "h-1", "status": "resolved" })],
        })
        .expect("serialize");

        assert_eq!(
            body["execution_number"], 7,
            "flattened, not nested under `exec`"
        );
        assert_eq!(body["status"], "success");
        assert_eq!(body["hitl"][0]["status"], "resolved");
        assert!(
            body.get("exec").is_none(),
            "`exec` must not appear as a key"
        );
    }

    /// A run that never paused still answers with the field, as an empty list —
    /// the frontend assigns it straight onto the timeline, and `undefined` there
    /// would be a different thing from "nothing was asked".
    #[test]
    fn a_run_that_never_paused_carries_an_empty_hitl_list() {
        let body = serde_json::to_value(ExecWithHitlResponse {
            exec: exec_with_workflow(Uuid::nil()),
            hitl: vec![],
        })
        .expect("serialize");

        assert_eq!(body["hitl"], serde_json::json!([]));
    }
}
