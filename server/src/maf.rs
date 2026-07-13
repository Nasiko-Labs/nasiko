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
    llm::LlmClient,
    planner::{self, AgentInfo as PlannerAgentInfo},
    types::{MafDefinition, MafStep},
};
use crate::auth::Claims;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/maf/workflows", get(list_mafs).post(create_maf))
        .route("/maf/generate", post(generate_maf))
        // Static segment "result" wins over {id} in matchit so this route is unambiguous
        .route("/maf/workflow/result/{exec_id}", get(get_result))
        .route("/maf/workflow/{id}", get(get_maf).put(update_maf).delete(delete_maf))
        .route("/maf/workflow/{id}/run", post(run_workflow))
        .route("/maf/workflow/{id}/executions", get(list_executions))
        .route("/maf/execution/{id}", get(get_execution))
}

// ─── Shared helpers ────────────────────────────────────────────────────────

fn parse_user_id(claims: &Claims) -> Option<Uuid> {
    claims.sub.parse().ok()
}

fn internal_err(e: impl std::fmt::Display) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}

fn not_found() -> axum::response::Response {
    StatusCode::NOT_FOUND.into_response()
}

fn forbidden(msg: &str) -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": msg}))).into_response()
}

fn bad_request(msg: &str) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg}))).into_response()
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
    steps: Vec<CreateStepRequest>,
}

#[derive(Deserialize)]
struct UpdateStepRequest {
    step_index: i32,
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
fn default_limit() -> i64 { 50 }

// ─── 1. GET /maf/workflows ─────────────────────────────────────────────────

async fn list_mafs(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(id) => id,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let rows = sqlx::query_as::<_, MafRow>(
        r#"SELECT id, user_id, name, description, maf_json::text AS maf_json,
                  status, created_at, updated_at
           FROM mafs
           WHERE user_id = $1 AND status = 'active'
           ORDER BY created_at DESC
           LIMIT $2 OFFSET $3"#,
    )
    .bind(user_id)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => {
            let items: Vec<MafResponse> = data.into_iter().map(maf_row_to_response).collect();
            Json(crate::Paginated::new(items)).into_response()
        }
        Err(e) => internal_err(e),
    }
}

// ─── 2. POST /maf/workflows ────────────────────────────────────────────────

async fn create_maf(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<CreateMafRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(id) => id,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if req.steps.is_empty() {
        return bad_request("steps must not be empty");
    }

    // Resolve any steps that lack an agent_id via the routing engine
    let mut resolved_steps: Vec<MafStep> = Vec::with_capacity(req.steps.len());
    for (idx, step) in req.steps.into_iter().enumerate() {
        if step.task_description.trim().is_empty() {
            return bad_request(&format!("step {idx}: task_description is required"));
        }

        let (agent_id, agent_name, agent_endpoint) = if let Some(aid) = step.agent_id {
            // Caller provided an agent — look up name and endpoint
            match fetch_agent_info(&state.db, aid).await {
                Ok(Some((name, url))) => (aid, name, url),
                Ok(None) => {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({"error": format!("agent {aid} not found")})),
                    )
                        .into_response();
                }
                Err(e) => return internal_err(e),
            }
        } else {
            // Auto-assign via routing engine
            let route_req = RouteRequest {
                query: step.task_description.clone(),
                session_id: Uuid::new_v4().to_string(),
                user_id,
                file_parts: vec![],
            };
            match state.routing_engine.route(route_req, &state.db).await {
                Ok(result) => {
                    let endpoint = result.agent.url.unwrap_or_default();
                    if endpoint.is_empty() {
                        return bad_request(&format!(
                            "step {idx}: auto-assigned agent '{}' has no endpoint",
                            result.agent.name
                        ));
                    }
                    (result.agent.id, result.agent.name, endpoint)
                }
                Err(_) => {
                    // Routing engine only queries status='running' agents.
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
                        None => return bad_request(&format!(
                            "step {idx}: no agents available. Register at least one agent \
                             in the Agents page before creating a workflow."
                        )),
                    }
                }
            }
        };

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
    let name = req.name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| resolved_steps[0].task_description.chars().take(60).collect());

    let maf_def = MafDefinition {
        description: None,       // generated by the runtime planner on each execution
        steps: resolved_steps,
        output_generation: None, // generated by the runtime planner on each execution
    };
    let maf_json = serde_json::to_value(&maf_def).unwrap_or_default();
    let maf_json_str = maf_json.to_string();

    let row = sqlx::query_as::<_, MafRow>(
        r#"INSERT INTO mafs (user_id, name, description, maf_json)
           VALUES ($1, $2, NULL, $3::jsonb)
           RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                     status, created_at, updated_at"#,
    )
    .bind(user_id)
    .bind(&name)
    .bind(&maf_json_str)
    .fetch_one(&state.db)
    .await;

    match row {
        Ok(r) => (StatusCode::CREATED, Json(maf_row_to_response(r))).into_response(),
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
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match fetch_maf(&state.db, id).await {
        Ok(Some(row)) if row.user_id == user_id => Json(maf_row_to_response(row)).into_response(),
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found(),
        Err(e) => internal_err(e),
    }
}

// ─── 4. PUT /maf/workflow/{id} ─────────────────────────────────────────────

async fn update_maf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
    Json(req): Json<UpdateMafRequest>,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let existing = match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id == user_id => r,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found(),
        Err(e) => return internal_err(e),
    };

    // Build new maf_json if steps are being replaced
    let new_maf_json_str = if let Some(steps) = req.steps {
        if steps.is_empty() {
            return bad_request("steps must not be empty");
        }

        let mut resolved: Vec<MafStep> = Vec::with_capacity(steps.len());
        for (idx, step) in steps.iter().enumerate() {
            if step.task_description.trim().is_empty() {
                return bad_request(&format!("step {}: task_description is required", step.step_index));
            }

            let (agent_id, name, endpoint) = if let Some(aid) = step.agent_id {
                match fetch_agent_info(&state.db, aid).await {
                    Ok(Some((n, u))) => (aid, n, u),
                    Ok(None) => return (
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({"error": format!("agent {aid} not found")})),
                    ).into_response(),
                    Err(e) => return internal_err(e),
                }
            } else {
                // Auto-assign via routing engine (same logic as create_maf)
                let route_req = RouteRequest {
                    query: step.task_description.clone(),
                    session_id: Uuid::new_v4().to_string(),
                    user_id,
                    file_parts: vec![],
                };
                match state.routing_engine.route(route_req, &state.db).await {
                    Ok(result) => {
                        let ep = result.agent.url.unwrap_or_default();
                        if ep.is_empty() {
                            return bad_request(&format!(
                                "step {idx}: auto-assigned agent '{}' has no endpoint",
                                result.agent.name
                            ));
                        }
                        (result.agent.id, result.agent.name, ep)
                    }
                    Err(_) => {
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
                                query_lower.split_whitespace().filter(|w| haystack.contains(*w)).count()
                            });
                        match best {
                            Some(a) => (a.id, a.name, a.url.unwrap_or_default()),
                            None => return bad_request(&format!(
                                "step {idx}: no agents available. Register at least one agent in the Agents page."
                            )),
                        }
                    }
                }
            };

            resolved.push(MafStep {
                step_id: Uuid::new_v4(),
                step_index: step.step_index,
                agent_id,
                agent_name: name,
                agent_endpoint: endpoint,
                task_description: step.task_description.clone(),
            });
        }
        // Preserve description and output_generation from the existing maf_json when replacing steps
        let existing_def: MafDefinition =
            serde_json::from_str(&existing.maf_json).unwrap_or(MafDefinition { description: None, steps: vec![], output_generation: None });
        let def = MafDefinition { description: existing_def.description, steps: resolved, output_generation: existing_def.output_generation };
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
           WHERE id = $4 AND status = 'active'
           RETURNING id, user_id, name, description, maf_json::text AS maf_json,
                     status, created_at, updated_at"#,
    )
    .bind(new_name)
    .bind(new_description)
    .bind(&new_maf_json_str)
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => Json(maf_row_to_response(r)).into_response(),
        Ok(None) => not_found(),
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
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Ownership check before soft-delete
    match fetch_maf(&state.db, id).await {
        Ok(Some(row)) if row.user_id != user_id => return forbidden("not owned by caller"),
        Ok(None) => return not_found(),
        Err(e) => return internal_err(e),
        Ok(Some(_)) => {}
    }

    match sqlx::query(
        "UPDATE mafs SET status = 'deleted', updated_at = now() WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => not_found(),
        Err(e) => internal_err(e),
    }
}

// ─── 6. POST /maf/workflow/{id}/run ───────────────────────────────────────

async fn run_workflow(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let maf = match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id == user_id => r,
        Ok(Some(_)) => return forbidden("not owned by caller"),
        Ok(None) => return not_found(),
        Err(e) => return internal_err(e),
    };

    let max_attempts: i32 = std::env::var("MAF_MAX_ATTEMPTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    // Create execution record
    let (exec_id, exec_number): (Uuid, i64) = match sqlx::query_as(
        r#"INSERT INTO maf_executions (maf_id, user_id, status, max_attempts)
           VALUES ($1, $2, 'pending', $3)
           RETURNING id, execution_number"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(max_attempts)
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

    let enqueue: redis::RedisResult<String> = redis::cmd("XADD")
        .arg("nasiko:maf:execute")
        .arg("*")
        .arg("execution_id")
        .arg(exec_id.to_string())
        .arg("maf_json")
        .arg(&maf.maf_json)
        .arg("user_id")
        .arg(user_id.to_string())
        .query_async(&mut redis_conn)
        .await;

    if let Err(e) = enqueue {
        // Roll back the execution row so the caller knows it wasn't queued
        let _ = sqlx::query("DELETE FROM maf_executions WHERE id = $1")
            .bind(exec_id)
            .execute(&state.db)
            .await;
        return internal_err(format!("failed to enqueue job: {e}"));
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"execution_id": exec_id, "execution_number": exec_number})),
    )
        .into_response()
}

// ─── 7. GET /maf/workflow/result/{exec_id} ────────────────────────────────

async fn get_result(
    State(state): State<AppState>,
    Path(exec_id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match fetch_exec(&state.db, exec_id).await {
        Ok(Some(row)) if row.user_id == user_id => Json(exec_row_to_response(row)).into_response(),
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found(),
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
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Verify ownership of the MAF first
    match fetch_maf(&state.db, id).await {
        Ok(Some(r)) if r.user_id != user_id => return forbidden("not owned by caller"),
        Ok(None) => return not_found(),
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

    match rows {
        Ok(data) => {
            let items: Vec<ExecResponse> = data.into_iter().map(exec_row_to_response).collect();
            Json(crate::Paginated::new(items)).into_response()
        }
        Err(e) => internal_err(e),
    }
}

// ─── 9. GET /maf/execution/{id} ──────────────────────────────────────────

async fn get_execution(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match parse_user_id(&claims) {
        Some(u) => u,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match fetch_exec(&state.db, id).await {
        Ok(Some(row)) if row.user_id == user_id => Json(exec_row_to_response(row)).into_response(),
        Ok(Some(_)) => forbidden("not owned by caller"),
        Ok(None) => not_found(),
        Err(e) => internal_err(e),
    }
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
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if req.description.trim().is_empty() {
        return bad_request("description is required");
    }

    // Build LLM client from config — require an API key
    let api_key = match &state.config.openai_api_key {
        Some(k) => k.clone(),
        None => return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "OPENAI_API_KEY is not configured on this server"})),
        ).into_response(),
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
        return bad_request("no agents registered — register at least one agent before generating a MAF");
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

            Json(GenerateMafResponse {
                name: plan.name,
                description: plan.description,
                output_generation: plan.output_generation,
                steps,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("planning failed: {e}")})),
        )
            .into_response(),
    }
}

// ─── DB helpers ────────────────────────────────────────────────────────────

async fn fetch_maf(db: &sqlx::PgPool, id: Uuid) -> Result<Option<MafRow>, sqlx::Error> {
    sqlx::query_as::<_, MafRow>(
        r#"SELECT id, user_id, name, description, maf_json::text AS maf_json,
                  status, created_at, updated_at
           FROM mafs WHERE id = $1 AND status = 'active'"#,
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
