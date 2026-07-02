use axum::{
    Json,
    Router,
    extract::{ Path, Query, State },
    http::StatusCode,
    response::IntoResponse,
    routing::{ get, post, put },
};
use serde::{ Deserialize, Serialize };
use uuid::Uuid;

use nasiko_runtime::ContainerId;

use crate::acl::user_can_access_agent;
use crate::auth::Claims;
use crate::state::AppState;

use super::models::{ Agent, AgentSummary, AgentVersion, CreateAgent, UpdateAgent };

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", post(create))
        .route("/agents", get(list))
        .route("/agents/{id}", get(get_one))
        .route("/agents/{id}", put(update))
        .route("/agents/{id}", axum::routing::delete(delete))
        .route("/agents/{id}/versions", get(list_versions))
        .route("/agents/search", get(search))
        .route("/agents/by-skill", get(by_skill))
        .route("/search/users", get(search_users))
}

#[derive(Deserialize)]
struct BySkillQuery {
    tag: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

/// Discover agents that have a skill tagged `tag`. Owner-scoped like `list`
/// (superuser → all; otherwise own). Uses the GIN `idx_agent_skills_tags` via
/// the `@>` containment operator and `EXISTS` (no join fan-out / DISTINCT).
async fn by_skill(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<BySkillQuery>
) -> impl IntoResponse {
    let tag = q.tag.trim();
    if tag.is_empty() {
        return (StatusCode::BAD_REQUEST, "tag is required").into_response();
    }
    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let owner_filter: Option<Uuid> = if claims.is_superuser {
        None
    } else {
        match claims.sub.parse() {
            Ok(id) => Some(id),
            Err(_) => {
                return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
            }
        }
    };

    let result = sqlx
        ::query_as::<_, AgentSummary>(
            r#"SELECT a.id, a.name, a.display_name, a.description, a.url, a.icon_url,
                  a.version, a.status, a.tags, a.created_at
           FROM agents a
           WHERE ($4::uuid IS NULL OR a.owner_id = $4)
             AND EXISTS (
                 SELECT 1 FROM agent_skills s
                 WHERE s.agent_id = a.id AND s.tags @> ARRAY[$1]::text[]
             )
           ORDER BY a.created_at DESC
           LIMIT $2 OFFSET $3"#
        )
        .bind(tag)
        .bind(limit)
        .bind(offset)
        .bind(owner_filter)
        .fetch_all(&state.db).await;

    match result {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::error!(%e, "by_skill: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

async fn create(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateAgent>
) -> impl IntoResponse {
    let caps = body.capabilities.unwrap_or(
        serde_json::json!({
        "streaming": false,
        "pushNotifications": false,
        "stateTransitionHistory": false,
        "chat_agent": false
    })
    );
    let skills_vec = body.skills.unwrap_or_default();
    let mut tags = body.tags.unwrap_or_default();
    // Merge unique tags declared on each skill into the agent's tag set.
    for skill in &skills_vec {
        for tag in &skill.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }
    let skills = serde_json::to_value(&skills_vec).unwrap_or_default();
    let meta = body.metadata.unwrap_or(serde_json::json!({}));
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
        }
    };

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(%e, "create agent: begin tx");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    let result = sqlx
        ::query_as::<_, Agent>(
            r#"INSERT INTO agents (name, display_name, description, owner_id, url, icon_url, version, documentation_url, capabilities, skills, tags, metadata)
           VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '1.0.0'), $8, $9, $10, $11, $12)
           RETURNING *"#
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
        .fetch_one(&mut *tx).await;

    let agent = match result {
        Ok(a) => a,
        Err(e) => {
            let is_conflict = e
                .as_database_error()
                .and_then(|d| d.code())
                .map(|c| c == "23505")
                .unwrap_or(false);
            if is_conflict {
                return (StatusCode::CONFLICT, "agent name already exists").into_response();
            }
            tracing::error!(%e, "create agent: insert failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    // Sync the normalized skills projection atomically with the agent row.
    if let Err(e) = super::skills::sync_agent_skills(&mut tx, agent.id, &agent.skills.0).await {
        tracing::error!(%e, agent_id = %agent.id, "create agent: sync skills failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(%e, "create agent: commit failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
    }

    (StatusCode::CREATED, Json(agent)).into_response()
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
fn default_limit() -> i64 {
    50
}

async fn list(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>
) -> impl IntoResponse {
    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let owner_filter: Option<Uuid> = if claims.is_superuser {
        None
    } else {
        match claims.sub.parse() {
            Ok(id) => Some(id),
            Err(_) => {
                return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
            }
        }
    };

    let agents = sqlx
        ::query_as::<_, Agent>(
            r#"SELECT * FROM agents
           WHERE ($1::uuid IS NULL OR owner_id = $1)
             AND ($3::uuid IS NULL OR owner_id = $3)
             AND ($2::text IS NULL OR status = $2)
           ORDER BY created_at DESC
           LIMIT $4 OFFSET $5"#
        )
        .bind(q.owner)
        .bind(&q.status)
        .bind(owner_filter)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db).await;

    match agents {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::error!(%e, "list agents: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

async fn get_one(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
        }
    };

    let result = match id.parse::<Uuid>() {
        Ok(uuid) => {
            sqlx
                ::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1")
                .bind(uuid)
                .fetch_optional(&state.db).await
        }
        Err(_) => {
            sqlx
                ::query_as::<_, Agent>("SELECT * FROM agents WHERE name = $1")
                .bind(&id)
                .fetch_optional(&state.db).await
        }
    };

    let agent = match result {
        Ok(Some(a)) => a,
        Ok(None) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent.id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    Json(agent).into_response()
}

async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAgent>
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
        }
    };

    // Superusers can update any agent; others must own it or be on the team.
    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let skills_changed = body.skills.is_some();

    // When skills are being updated, merge their tags into the provided tag list so
    // the COALESCE write carries all skill-derived tags alongside any explicit ones.
    let merged_tags = if let Some(ref skill_list) = body.skills {
        let mut tags = body.tags.clone().unwrap_or_default();
        for skill in skill_list {
            for tag in &skill.tags {
                if !tags.contains(tag) {
                    tags.push(tag.clone());
                }
            }
        }
        Some(tags)
    } else {
        body.tags.clone()
    };

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(%e, "update agent: begin tx");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    let result = sqlx
        ::query_as::<_, Agent>(
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
           RETURNING *"#
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
        .bind(&merged_tags)
        .bind(&body.metadata)
        .bind(&body.status)
        .bind(&body.image)
        .fetch_optional(&mut *tx).await;

    let agent = match result {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => {
            tracing::error!(%e, %id, "update agent: db error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    // Only re-sync the skills projection when skills were actually in the request body.
    if
        skills_changed &&
        let Err(e) = super::skills::sync_agent_skills(&mut tx, agent.id, &agent.skills.0).await
    {
        tracing::error!(%e, agent_id = %agent.id, "update agent: sync skills failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
    }
    match tx.commit().await {
        Ok(()) => Json(agent).into_response(),
        Err(e) => {
            tracing::error!(%e, "update agent: commit failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

#[derive(Serialize)]
struct DeletedAgent {
    deleted: bool,
    agent_id: Uuid,
    containers_stopped: usize,
    runtime_errors: Vec<String>,
}

async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
        }
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Fetch agent name early — gives a clean 404 before touching the runtime,
    // and provides the primary container name needed for teardown.
    let name: String = match
        sqlx
            ::query_scalar("SELECT name FROM agents WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db).await
    {
        Ok(Some(n)) => n,
        Ok(None) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => {
            tracing::error!(%e, %id, "delete agent: fetch name");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    // Collect all non-stopped deployment container names for this agent.
    // Collect distinct K8s workload names from non-stopped deployment rows.
    // k8s_deployment_name is the actual workload/container identifier; namespace is
    // the K8s namespace (e.g. 'nasiko-agents') and must not be used as a container ID.
    // In Docker OSS, k8s_deployment_name is NULL so no extra entries are added and
    // teardown falls through to the agent name only.
    let k8s_names: Vec<String> = sqlx
        ::query_scalar(
            "SELECT DISTINCT k8s_deployment_name FROM agent_deployments
         WHERE agent_id = $1 AND status != 'stopped' AND k8s_deployment_name IS NOT NULL"
        )
        .bind(id)
        .fetch_all(&state.db).await
        .unwrap_or_default();

    let mut containers_to_stop: Vec<String> = vec![name.clone()];
    for kn in k8s_names {
        if !containers_to_stop.contains(&kn) {
            containers_to_stop.push(kn);
        }
    }

    // Tear down all identified containers before deleting DB records (best-effort).
    let mut containers_stopped = 0usize;
    let mut runtime_errors: Vec<String> = vec![];
    for container_name in &containers_to_stop {
        match state.runtime.destroy(&ContainerId::new(container_name)).await {
            Ok(()) => {
                containers_stopped += 1;
            }
            Err(e) => {
                // Absent/already-stopped containers are expected; log but don't fail.
                tracing::debug!(%e, %id, container_name, "delete agent: runtime.destroy — absent or already stopped");
                runtime_errors.push(format!("{container_name}: {e}"));
            }
        }
    }

    let result = sqlx::query("DELETE FROM agents WHERE id = $1").bind(id).execute(&state.db).await;

    match result {
        Ok(r) if r.rows_affected() > 0 =>
            (
                StatusCode::OK,
                Json(DeletedAgent {
                    deleted: true,
                    agent_id: id,
                    containers_stopped,
                    runtime_errors,
                }),
            ).into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "delete agent: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

async fn list_versions(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(uid) => uid,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
        }
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let result = sqlx
        ::query_as::<_, AgentVersion>(
            "SELECT * FROM agent_versions WHERE agent_id = $1 ORDER BY created_at DESC"
        )
        .bind(id)
        .fetch_all(&state.db).await;

    match result {
        Ok(versions) => Json(versions).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Scoring helpers ───────────────────────────────────────────────────────────
//
//
//   exact match   → 100.0 * boost
//   prefix match  →  90.0 * boost   (text starts with query)
//   contains      →  70.0 * boost   (query appears anywhere in text)
//   no match      →   0.0
//
// Python takes max() across fields — we use GREATEST() in SQL.
//
// Agent field boosts (from redis_search_service.py lines 289-321):
//   name         → 2.8   (exact=280, prefix=252, contains=196)
//   display_name → 2.4   (exact=240, prefix=216, contains=168)
//   description  → 2.0   (exact=200, prefix=180, contains=140)
//   tag exact    → 95.0  (fixed)
//   tag partial  → 70.0  (fixed)
//
// User field boosts (from redis_search_service.py lines 216-226):
//   username     → 3.0  (exact=300, prefix=270, contains=210)
//   display_name → 2.5  (exact=250, prefix=225, contains=175)
//   email        → 1.5  (exact=150, prefix=135, contains=105)

const AGENT_SCORE_SQL: &str =
    r#"
    GREATEST(
        CASE
            WHEN lower(name) = lower($1)     THEN 280.0
            WHEN name ILIKE $1 || '%'        THEN 252.0
            WHEN name ILIKE '%' || $1 || '%' THEN 196.0
            ELSE 0.0
        END,
        CASE
            WHEN lower(COALESCE(display_name,'')) = lower($1)              THEN 240.0
            WHEN COALESCE(display_name,'') ILIKE $1 || '%'                 THEN 216.0
            WHEN COALESCE(display_name,'') ILIKE '%' || $1 || '%'          THEN 168.0
            ELSE 0.0
        END,
        CASE
            WHEN lower(COALESCE(description,'')) = lower($1)              THEN 200.0
            WHEN COALESCE(description,'') ILIKE $1 || '%'                 THEN 180.0
            WHEN COALESCE(description,'') ILIKE '%' || $1 || '%'          THEN 140.0
            ELSE 0.0
        END,
        CASE
            WHEN EXISTS (
                SELECT 1 FROM unnest(tags) t
                WHERE lower(t) = lower($1)
            ) THEN 95.0
            WHEN EXISTS (
                SELECT 1 FROM unnest(tags) t
                WHERE t ILIKE '%' || $1 || '%'
            ) THEN 70.0
            ELSE 0.0
        END
    )
"#;

const USER_SCORE_SQL: &str =
    r#"
    GREATEST(
        CASE
            WHEN lower(username) = lower($1)          THEN 300.0
            WHEN username ILIKE $1 || '%'              THEN 270.0
            WHEN username ILIKE '%' || $1 || '%'       THEN 210.0
            ELSE 0.0
        END,
        CASE
            WHEN lower(COALESCE(display_name,'')) = lower($1)              THEN 250.0
            WHEN COALESCE(display_name,'') ILIKE $1 || '%'                 THEN 225.0
            WHEN COALESCE(display_name,'') ILIKE '%' || $1 || '%'          THEN 175.0
            ELSE 0.0
        END,
        CASE
            WHEN lower(COALESCE(email,'')) = lower($1)     THEN 150.0
            WHEN COALESCE(email,'') ILIKE $1 || '%'        THEN 135.0
            WHEN COALESCE(email,'') ILIKE '%' || $1 || '%' THEN 105.0
            ELSE 0.0
        END
    )
"#;

// ── /agents/search ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: i64,
}

/// Agent-only search.  Scoring: GREATEST across (name×2.8, description×2.0, tag score) with tiered
/// exact/prefix/contains scoring.  Minimum query length: 2 chars.
async fn search(
    State(state): State<AppState>,
    claims: Claims,
    Query(sq): Query<SearchQuery>
) -> impl IntoResponse {
    let q = sq.q.trim().to_string();
    if q.len() < 2 {
        return (StatusCode::BAD_REQUEST, "q must be at least 2 characters").into_response();
    }

    let owner_filter: Option<Uuid> = if claims.is_superuser {
        None
    } else {
        match claims.sub.parse() {
            Ok(id) => Some(id),
            Err(_) => {
                return (StatusCode::UNAUTHORIZED, "invalid user id").into_response();
            }
        }
    };

    let sql = format!(
        r#"SELECT * FROM (
               SELECT *, {AGENT_SCORE_SQL} AS _score
               FROM agents
               WHERE ($3::uuid IS NULL OR owner_id = $3)
           ) _s
           WHERE _score > 0
           ORDER BY _score DESC, name ASC
           LIMIT $2"#
    );

    let result = sqlx
        ::query_as::<_, Agent>(&sql)
        .bind(&q)
        .bind(sq.limit.clamp(1, 50))
        .bind(owner_filter)
        .fetch_all(&state.db).await;

    match result {
        Ok(agents) => Json(agents).into_response(),
        Err(e) => {
            tracing::error!(%e, "agents search: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

// ── /search/users ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UserSearchQuery {
    q: String,
}

/// User search. Field boosts: username×3.0, display_name×2.5, email×1.5.
/// Scoring: exact (100×boost) > prefix (90×boost) > contains (70×boost).
/// Minimum query length: 2 chars. Sort: score DESC, username ASC.
/// Returns all matching users (no limit).
#[derive(Serialize, sqlx::FromRow)]
struct UserSearchResult {
    id: Uuid,
    username: String,
    display_name: Option<String>,
    email: Option<String>,
    score: f64,
}

#[derive(Serialize)]
struct UserSearchResponse {
    users: Vec<UserSearchResult>,
    total: usize,
    max_score: f64,
}

async fn search_users(
    State(state): State<AppState>,
    Query(sq): Query<UserSearchQuery>
) -> impl IntoResponse {
    let q = sq.q.trim().to_string();
    if q.len() < 2 {
        return (StatusCode::BAD_REQUEST, "q must be at least 2 characters").into_response();
    }

    let sql = format!(
        r#"SELECT id, username, display_name, email, score FROM (
               SELECT id, username, display_name, email,
                      {USER_SCORE_SQL} AS score
               FROM users
               WHERE deleted_at IS NULL
           ) _s
           WHERE score > 0
           ORDER BY score DESC, username ASC"#
    );

    let result = sqlx
        ::query_as::<_, UserSearchResult>(&sql)
        .bind(&q)
        .fetch_all(&state.db).await;

    match result {
        Ok(users) => {
            let max_score = users
                .first()
                .map(|u| u.score)
                .unwrap_or(0.0);
            let total = users.len();
            Json(UserSearchResponse { users, total, max_score }).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "search_users: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}
