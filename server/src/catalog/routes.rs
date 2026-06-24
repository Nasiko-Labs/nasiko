use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::acl::user_can_access_agent;
use crate::auth::Claims;
use crate::state::AppState;

use super::models::{Agent, AgentVersion, CreateAgent, UpdateAgent};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", post(create))
        .route("/agents", get(list))
        .route("/agents/{id}", get(get_one))
        .route("/agents/{id}", put(update))
        .route("/agents/{id}", axum::routing::delete(delete))
        .route("/agents/{id}/versions", get(list_versions))
        .route("/agents/search", get(search))
}

async fn create(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateAgent>,
) -> impl IntoResponse {
    let caps = body.capabilities.unwrap_or(serde_json::json!({
        "streaming": false,
        "pushNotifications": false,
        "stateTransitionHistory": false,
        "chat_agent": false
    }));
    let skills = serde_json::to_value(body.skills.unwrap_or_default()).unwrap_or_default();
    let tags = body.tags.unwrap_or_default();
    let meta = body.metadata.unwrap_or(serde_json::json!({}));
    let owner_id: Uuid = claims.sub.parse().unwrap_or_default();

    let result = sqlx::query_as::<_, Agent>(
        r#"INSERT INTO agents (name, display_name, description, owner_id, url, icon_url, version, documentation_url, capabilities, skills, tags, metadata)
           VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '1.0.0'), $8, $9, $10, $11, $12)
           RETURNING *"#,
    )
    .bind(&body.name)
    .bind(&body.display_name)
    .bind(&body.description)
    .bind(owner_id)
    .bind(&body.url)
    .bind(&body.icon_url)
    .bind(&body.version)
    .bind(&body.documentation_url)
    .bind(caps)
    .bind(skills)
    .bind(&tags)
    .bind(meta)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(agent) => (StatusCode::CREATED, Json(agent)).into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    owner: Option<Uuid>,
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 { 50 }

async fn list(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id: Uuid = claims.sub.parse().unwrap_or_default();

    // Superusers see all agents; others see own agents + team agents
    let agents = if claims.is_superuser {
        sqlx::query_as::<_, Agent>(
            r#"SELECT * FROM agents
               WHERE ($1::uuid IS NULL OR owner_id = $1)
                 AND ($2::text IS NULL OR status = $2)
               ORDER BY created_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(q.owner)
        .bind(&q.status)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, Agent>(
            r#"SELECT * FROM agents
               WHERE owner_id = $5
                 AND ($1::uuid IS NULL OR owner_id = $1)
                 AND ($2::text IS NULL OR status = $2)
               ORDER BY created_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(q.owner)
        .bind(&q.status)
        .bind(q.limit)
        .bind(q.offset)
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match agents {
        Ok(list) => Json(list).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = match id.parse::<Uuid>() {
        Ok(uuid) => {
            sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1")
                .bind(uuid)
                .fetch_optional(&state.db)
                .await
        }
        Err(_) => {
            sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE name = $1")
                .bind(&id)
                .fetch_optional(&state.db)
                .await
        }
    };

    match result {
        Ok(Some(agent)) => Json(agent).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAgent>,
) -> impl IntoResponse {
    let user_id: Uuid = claims.sub.parse().unwrap_or_default();

    // Superusers can update any agent; others must own it or be on the team
    if !claims.is_superuser
        && !user_can_access_agent(&state.db, user_id, id).await {
            return StatusCode::FORBIDDEN.into_response();
        }

    let result = sqlx::query_as::<_, Agent>(
        r#"UPDATE agents SET
             display_name = COALESCE($2, display_name),
             description = COALESCE($3, description),
             url = COALESCE($4, url),
             icon_url = COALESCE($5, icon_url),
             version = COALESCE($6, version),
             documentation_url = COALESCE($7, documentation_url),
             capabilities = COALESCE($8, capabilities),
             skills = COALESCE($9, skills),
             tags = COALESCE($10, tags),
             metadata = COALESCE($11, metadata),
             status = COALESCE($12, status),
             image = COALESCE($13, image),
             updated_at = now()
           WHERE id = $1
           RETURNING *"#,
    )
    .bind(id)
    .bind(&body.display_name)
    .bind(&body.description)
    .bind(&body.url)
    .bind(&body.icon_url)
    .bind(&body.version)
    .bind(&body.documentation_url)
    .bind(&body.capabilities)
    .bind(body.skills.as_ref().and_then(|s| serde_json::to_value(s).ok()))
    .bind(&body.tags)
    .bind(&body.metadata)
    .bind(&body.status)
    .bind(&body.image)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(agent)) => Json(agent).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = claims.sub.parse().unwrap_or_default();

    if !claims.is_superuser
        && !user_can_access_agent(&state.db, user_id, id).await {
            return StatusCode::FORBIDDEN.into_response();
        }

    let result = sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_versions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result = sqlx::query_as::<_, AgentVersion>(
        "SELECT * FROM agent_versions WHERE agent_id = $1 ORDER BY created_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await;

    match result {
        Ok(versions) => Json(versions).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: i64,
}

async fn search(
    State(state): State<AppState>,
    Query(sq): Query<SearchQuery>,
) -> impl IntoResponse {
    let result = sqlx::query_as::<_, Agent>(
        r#"SELECT * FROM agents
           WHERE name ILIKE '%' || $1 || '%'
              OR display_name ILIKE '%' || $1 || '%'
              OR description ILIKE '%' || $1 || '%'
           ORDER BY similarity(name, $1) DESC
           LIMIT $2"#,
    )
    .bind(&sq.q)
    .bind(sq.limit)
    .fetch_all(&state.db)
    .await;

    match result {
        Ok(agents) => Json(agents).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
