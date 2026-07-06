use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flows", get(list_flows))
        .route("/flows", post(create_flow))
        .route("/flows/{flow_id}", get(get_flow))
        .route("/flows/{flow_id}/complete", post(complete_flow))
        .route("/flows/{flow_id}/steps", get(list_steps))
        .route("/flows/{flow_id}/steps", post(add_step))
}

// ─── Models ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Flow {
    pub id: Uuid,
    pub flow_id: String,
    pub user_id: Uuid,
    pub root_agent_id: Option<Uuid>,
    pub root_agent_name: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub max_depth_reached: i32,
    pub total_invocations: i32,
    pub total_tokens_used: i64,
    pub total_cost_usd: Option<rust_decimal::Decimal>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct FlowStep {
    pub id: Uuid,
    pub flow_id: String,
    pub step_order: i32,
    pub depth: i32,
    pub agent_id: Option<Uuid>,
    pub agent_name: String,
    pub caller_agent_name: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub status: String,
    pub tokens_used: i32,
    pub latency_ms: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Handlers ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    status: Option<String>,
    q: Option<String>,
}
fn default_limit() -> i64 { 50 }

async fn list_flows(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let rows = sqlx::query_as::<_, Flow>(
        r#"SELECT * FROM flows
           WHERE user_id = $1
             AND ($2::text IS NULL OR status = $2)
             AND ($3::text IS NULL OR root_agent_name ILIKE '%' || $3 || '%' OR title ILIKE '%' || $3 || '%')
           ORDER BY created_at DESC
           LIMIT $4 OFFSET $5"#,
    )
    .bind(user_id)
    .bind(&q.status)
    .bind(&q.q)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => Json(crate::Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "list_flows: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn get_flow(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let flow = sqlx::query_as::<_, Flow>(
        "SELECT * FROM flows WHERE flow_id = $1 AND user_id = $2",
    )
    .bind(&flow_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    match flow {
        Ok(Some(f)) => {
            let steps = sqlx::query_as::<_, FlowStep>(
                "SELECT * FROM flow_steps WHERE flow_id = $1 ORDER BY step_order ASC",
            )
            .bind(&flow_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            Json(serde_json::json!({
                "flow": f,
                "steps": steps
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "get_flow: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateFlowRequest {
    flow_id: String,
    root_agent_id: Option<Uuid>,
    root_agent_name: Option<String>,
    title: Option<String>,
    metadata: Option<serde_json::Value>,
}

async fn create_flow(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateFlowRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let meta = body.metadata.unwrap_or(serde_json::json!({}));

    let result = sqlx::query_as::<_, Flow>(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, metadata)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING *"#,
    )
    .bind(&body.flow_id)
    .bind(user_id)
    .bind(body.root_agent_id)
    .bind(&body.root_agent_name)
    .bind(&body.title)
    .bind(sqlx::types::Json(meta))
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(flow) => (StatusCode::CREATED, Json(flow)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, flow_id = %body.flow_id, "create_flow: db error");
            (StatusCode::CONFLICT, "flow already exists or invalid reference").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompleteFlowRequest {
    status: String,
    total_tokens_used: Option<i64>,
    total_cost_usd: Option<f64>,
    error_message: Option<String>,
    max_depth_reached: Option<i32>,
    total_invocations: Option<i32>,
}

async fn complete_flow(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
    Json(body): Json<CompleteFlowRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query_as::<_, Flow>(
        r#"UPDATE flows SET
             status = $3,
             total_tokens_used = COALESCE($4, total_tokens_used),
             total_cost_usd = COALESCE($5, total_cost_usd),
             error_message = $6,
             max_depth_reached = COALESCE($7, max_depth_reached),
             total_invocations = COALESCE($8, total_invocations),
             duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
             completed_at = now()
           WHERE flow_id = $1 AND user_id = $2
           RETURNING *"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(&body.status)
    .bind(body.total_tokens_used)
    .bind(body.total_cost_usd.map(rust_decimal::Decimal::try_from).and_then(Result::ok))
    .bind(&body.error_message)
    .bind(body.max_depth_reached)
    .bind(body.total_invocations)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(f)) => Json(f).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "complete_flow: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn list_steps(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Verify ownership
    let owns = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM flows WHERE flow_id = $1 AND user_id = $2)",
    )
    .bind(&flow_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let steps = sqlx::query_as::<_, FlowStep>(
        "SELECT * FROM flow_steps WHERE flow_id = $1 ORDER BY step_order ASC",
    )
    .bind(&flow_id)
    .fetch_all(&state.db)
    .await;

    match steps {
        Ok(data) => Json(data).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "list_steps: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AddStepRequest {
    agent_id: Option<Uuid>,
    agent_name: String,
    caller_agent_name: Option<String>,
    depth: Option<i32>,
    input_summary: Option<String>,
}

async fn add_step(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
    Json(body): Json<AddStepRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Verify ownership
    let owns = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM flows WHERE flow_id = $1 AND user_id = $2)",
    )
    .bind(&flow_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let next_order: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(step_order), 0) + 1 FROM flow_steps WHERE flow_id = $1",
    )
    .bind(&flow_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(1);

    let result = sqlx::query_as::<_, FlowStep>(
        r#"INSERT INTO flow_steps (flow_id, step_order, depth, agent_id, agent_name, caller_agent_name, input_summary, status)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'running')
           RETURNING *"#,
    )
    .bind(&flow_id)
    .bind(next_order)
    .bind(body.depth.unwrap_or(0))
    .bind(body.agent_id)
    .bind(&body.agent_name)
    .bind(&body.caller_agent_name)
    .bind(&body.input_summary)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(step) => (StatusCode::CREATED, Json(step)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "add_step: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
