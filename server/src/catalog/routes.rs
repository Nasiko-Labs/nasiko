use axum::{
    Json,
    Router,
    extract::{ Path, Query, State },
    http::StatusCode,
    response::IntoResponse,
    routing::{ get, post, put },
};
use chrono::{DateTime, Utc};
use serde::{ Deserialize, Serialize };
use std::collections::HashSet;
use uuid::Uuid;

use nasiko_runtime::ContainerId;

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
        .route("/agents/{id}/versions/{version}", axum::routing::delete(delete_version))
        .route("/agents/by-skill", get(by_skill))
        .route("/search/agents", get(search))
        .route("/search/users", get(search_users))
        .route("/registry/user/agents", get(registry_user_agents))
        .route("/registries/{id}", get(get_by_registry_id))
}

/// WHERE-clause fragment implementing the baseline catalog access predicate —
/// owner ∪ public ∪ user-grant — used to scope the `list`/`by_skill`/`search`
/// listing endpoints so a discoverable agent (public, or shared to the caller via
/// a user grant) actually shows up in listings rather than only being fetchable
/// directly by id via `get_one` (which uses `crate::acl::can_access_agent`) (CAT-3).
///
/// Mirrors the OSS `AuthServiceImpl::can_access_agent` SQL (oss/auth/src/service.rs).
///
/// `user_bind` is the positional bind parameter carrying the caller's user id as
/// `Option<Uuid>` — `NULL` short-circuits the whole predicate to "match everything",
/// which is how each call site already encodes the superuser bypass (see
/// `owner_filter` at each call site: `None` for superusers, `Some(user_id)`
/// otherwise). `table_ref` is however the `agents` table is referenced at the call
/// site (bare table name or a query alias) and must resolve unambiguously from
/// within the correlated `EXISTS` subquery.
///
/// EDITION-AWARE GAP: `EeAuthService::can_access_agent` (ee/auth/src/lib.rs)
/// additionally grants access via team/department membership, joining on
/// `users.team_id` / `users.department_id` — columns that only exist after the EE
/// `1002_org_hierarchy` migration. This file is compiled into and shared by both
/// the OSS and EE server binaries (`ee/server` wraps this crate's router rather
/// than forking it — see `nasiko_server::build_app_with_user_router`), so a single
/// static SQL string here cannot reference those EE-only columns without breaking
/// at runtime against an OSS-only-migrated database. Expressing the full
/// edition-aware predicate would require extending the `AuthService` trait
/// (oss/auth) with a listing-scoped method the EE impl can override, which is out
/// of scope for this file. Left as a known, intentional gap: under EE, an agent
/// granted to the caller only via team/department membership (not a direct
/// user-grant) still will not appear in `list`/`by_skill`/`search`, even though
/// `get_one`'s `can_access_agent` call allows fetching it directly by id.
fn agent_access_predicate(user_bind: &str, table_ref: &str) -> String {
    format!(
        r#"({user_bind}::uuid IS NULL
             OR {table_ref}.owner_id = {user_bind}
             OR {table_ref}.is_public = TRUE
             OR EXISTS (
                 SELECT 1 FROM agent_grants ag
                 WHERE ag.agent_id = {table_ref}.id
                   AND ((ag.grant_type = 'public' AND ag.grantee_id = '*')
                     OR (ag.grant_type = 'user'   AND ag.grantee_id = {user_bind}::text))
             ))"#
    )
}

#[derive(Deserialize)]
struct BySkillQuery {
    tag: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

/// Discover agents that have a skill tagged `tag`. Access-scoped like `list`
/// (superuser → all; otherwise owner ∪ public ∪ user-grant — see
/// `agent_access_predicate`). Uses the GIN `idx_agent_skills_tags` via the `@>`
/// containment operator and `EXISTS` (no join fan-out / DISTINCT).
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
        match claims.user_uuid() {
            Ok(id) => Some(id),
            Err(e) => return e.into_response(),
        }
    };

    // Normalise to lowercase before the GIN containment check.  Tags are
    // stored lowercase after migration 014, so this ensures the query matches
    // even if the caller passes "Data-Pipeline" instead of "data-pipeline".
    let tag_lower = tag.to_lowercase();

    let sql = format!(
        r#"SELECT a.id, a.name, a.display_name, a.description, a.url, a.icon_url,
                  a.version, a.status, a.tags, a.created_at
           FROM agents a
           WHERE ({access})
             AND EXISTS (
                 SELECT 1 FROM agent_skills s
                 WHERE s.agent_id = a.id AND s.tags @> ARRAY[$1]::text[]
             )
           ORDER BY a.created_at DESC
           LIMIT $2 OFFSET $3"#,
        access = agent_access_predicate("$4", "a")
    );

    let result = sqlx
        ::query_as::<_, AgentSummary>(&sql)
        .bind(&tag_lower)
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
    // Normalise tags to lowercase at write time for consistent GIN lookup.
    let mut tags: Vec<String> = body.tags.unwrap_or_default()
        .into_iter()
        .map(|t| t.to_lowercase())
        .collect();
    // Merge unique tags declared on each skill into the agent's tag set.
    // `seen` gives O(1) membership checks so the merge is O(n) overall instead of
    // O(n·m) from repeated `Vec::contains` scans; `tags` still gets pushed in
    // original encounter order.
    let mut seen: HashSet<String> = tags.iter().cloned().collect();
    for skill in &skills_vec {
        for tag in &skill.tags {
            let tag_lower = tag.to_lowercase();
            if seen.insert(tag_lower.clone()) {
                tags.push(tag_lower);
            }
        }
    }
    let skills = serde_json::to_value(&skills_vec).unwrap_or_default();
    let meta = body.metadata.unwrap_or(serde_json::json!({}));
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
        match claims.user_uuid() {
            Ok(id) => Some(id),
            Err(e) => return e.into_response(),
        }
    };

    let sql = format!(
        r#"SELECT * FROM agents
           WHERE ($1::uuid IS NULL OR owner_id = $1)
             AND ({access})
             AND ($2::text IS NULL OR status = $2)
           ORDER BY created_at DESC
           LIMIT $4 OFFSET $5"#,
        access = agent_access_predicate("$3", "agents")
    );

    let agents = sqlx
        ::query_as::<_, Agent>(&sql)
        .bind(q.owner)
        .bind(&q.status)
        .bind(owner_filter)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db).await;

    match agents {
        Ok(mut list) => {
            reconcile_running_status(&state, &mut list).await;
            Json(list).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list agents: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

/// A container that crashed after its last deploy/restart call still reads
/// `status = 'running'` from the DB — nothing rewrites that column on its own
/// for Docker-backed agents (the EE crash guardian only reconciles K8s
/// deployments). Cheap enough to check on every list call: `nasiko ps`-sized
/// fleets are small, and `runtime.status()` is a single local Docker API call
/// per agent. Read-only override on the response, not persisted — avoids
/// racing the crash guardian's own DB writes on K8s, and a stale read here is
/// harmless where a stale write would linger.
async fn reconcile_running_status(state: &AppState, agents: &mut [Agent]) {
    for agent in agents.iter_mut() {
        if agent.status != "running" {
            continue;
        }
        let container_id = ContainerId::from_uuid(agent.id);
        if let Ok(live) = state.runtime.status(&container_id).await {
            use nasiko_runtime::RuntimeState;
            if matches!(
                live.state,
                RuntimeState::Crashed | RuntimeState::Failed | RuntimeState::Stopped
            ) {
                agent.status = live.state.to_string();
            }
        }
    }
}

async fn get_one(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>
) -> impl IntoResponse {

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
            tracing::error!(%e, "get_one: db error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    if !crate::acl::can_access_agent(&state, &claims, agent.id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AgentDetailResponse {
        id: Uuid,
        name: String,
        version: String,
        description: String,
        url: String,
        preferred_transport: String,
        protocol_version: String,
        provider: Option<serde_json::Value>,
        icon_url: Option<String>,
        documentation_url: Option<String>,
        capabilities: serde_json::Value,
        security_schemes: serde_json::Value,
        security: Vec<serde_json::Value>,
        default_input_modes: Vec<String>,
        default_output_modes: Vec<String>,
        skills: Vec<serde_json::Value>,
        tags: Vec<String>,
        supports_authenticated_extended_card: bool,
        signatures: Vec<serde_json::Value>,
        additional_interfaces: Option<serde_json::Value>,
        #[serde(rename = "created_at")]
        created_at: DateTime<Utc>,
        #[serde(rename = "updated_at")]
        updated_at: DateTime<Utc>,
    }

    #[derive(Serialize)]
    struct SingleResponse {
        data: AgentDetailResponse,
        status_code: u16,
        message: String,
    }

    let skills: Vec<serde_json::Value> = agent.skills.0
        .iter()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .collect();

    let data = AgentDetailResponse {
        id: agent.id,
        name: agent.name.clone(),
        version: agent.version.clone(),
        description: agent.description.unwrap_or_default(),
        url: format!("/api/agents/{}", agent.id),
        preferred_transport: agent.preferred_transport.clone(),
        protocol_version: agent.protocol_version.clone(),
        provider: None,
        icon_url: agent.icon_url.clone(),
        documentation_url: agent.documentation_url.clone(),
        capabilities: agent.capabilities.0.clone(),
        security_schemes: agent.security_schemes.0.clone(),
        security: vec![],
        default_input_modes: agent.default_input_modes.0.clone(),
        default_output_modes: agent.default_output_modes.0.clone(),
        skills,
        tags: agent.tags.clone(),
        supports_authenticated_extended_card: false,
        signatures: vec![],
        additional_interfaces: None,
        created_at: agent.created_at,
        updated_at: agent.updated_at,
    };

    Json(SingleResponse {
        data,
        status_code: 200,
        message: "Registry retrieved successfully".to_string(),
    }).into_response()
}

async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAgent>
) -> impl IntoResponse {

    // Mutation → owner-or-superuser only (an invoke/public grant must not confer edit).
    if !crate::acl::can_manage_agent(&state, &claims, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let skills_changed = body.skills.is_some();

    // When skills are being updated, merge their tags into the provided tag list so
    // the COALESCE write carries all skill-derived tags alongside any explicit ones.
    // All tags are normalised to lowercase for consistent GIN lookup.
    let merged_tags = if let Some(ref skill_list) = body.skills {
        let mut tags: Vec<String> = body.tags.clone().unwrap_or_default()
            .into_iter()
            .map(|t| t.to_lowercase())
            .collect();
        // O(n) membership check via HashSet — see the analogous comment in `create`.
        let mut seen: HashSet<String> = tags.iter().cloned().collect();
        for skill in skill_list {
            for tag in &skill.tags {
                let tag_lower = tag.to_lowercase();
                if seen.insert(tag_lower.clone()) {
                    tags.push(tag_lower);
                }
            }
        }
        Some(tags)
    } else {
        body.tags.as_ref().map(|ts| ts.iter().map(|t| t.to_lowercase()).collect())
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

    if !crate::acl::can_manage_agent(&state, &claims, id).await {
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

    if !crate::acl::can_access_agent(&state, &claims, id).await {
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
        Err(e) => {
            tracing::error!(%e, agent_id = %id, "list_versions: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ── DELETE /agents/{id}/versions/{version} ────────────────────────────────────

async fn delete_version(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, version)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    // Deleting a version is destructive — owner-or-superuser only (RUN-9). A public
    // agent's viewer or an invoke-grantee must not be able to delete its versions.
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Prevent deleting the currently active version.
    let is_active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_versions WHERE agent_id = $1 AND version = $2 AND is_active = true)",
    )
    .bind(agent_id)
    .bind(&version)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if is_active {
        return (StatusCode::CONFLICT, "cannot delete the active version — rollback first").into_response();
    }

    match sqlx::query(
        "DELETE FROM agent_versions WHERE agent_id = $1 AND version = $2",
    )
    .bind(agent_id)
    .bind(&version)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, "version not found").into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, %version, "delete_version db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
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
            -- ILIKE $1 is case-insensitive and allows the trigram GIN index to
            -- activate.  lower(col) = lower($1) would defeat the GIN index because
            -- the index expression is 'name', not 'lower(name)'.
            WHEN name ILIKE $1                THEN 280.0
            WHEN name ILIKE $1 || '%'         THEN 252.0
            WHEN name ILIKE '%' || $1 || '%'  THEN 196.0
            ELSE 0.0
        END,
        CASE
            WHEN COALESCE(display_name,'') ILIKE $1                        THEN 240.0
            WHEN COALESCE(display_name,'') ILIKE $1 || '%'                 THEN 216.0
            WHEN COALESCE(display_name,'') ILIKE '%' || $1 || '%'          THEN 168.0
            ELSE 0.0
        END,
        CASE
            WHEN COALESCE(description,'') ILIKE $1                        THEN 200.0
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
            WHEN username ILIKE $1                     THEN 300.0
            WHEN username ILIKE $1 || '%'              THEN 270.0
            WHEN username ILIKE '%' || $1 || '%'       THEN 210.0
            ELSE 0.0
        END,
        CASE
            WHEN COALESCE(display_name,'') ILIKE $1                        THEN 250.0
            WHEN COALESCE(display_name,'') ILIKE $1 || '%'                 THEN 225.0
            WHEN COALESCE(display_name,'') ILIKE '%' || $1 || '%'          THEN 175.0
            ELSE 0.0
        END,
        CASE
            WHEN COALESCE(email,'') ILIKE $1           THEN 150.0
            WHEN COALESCE(email,'') ILIKE $1 || '%'    THEN 135.0
            WHEN COALESCE(email,'') ILIKE '%' || $1 || '%' THEN 105.0
            ELSE 0.0
        END
    )
"#;

/// Escape LIKE/ILIKE wildcards in a user-supplied search term so `%`/`_` can't be
/// injected (a bare `%` collapses the scoring CASEs to match-all). Postgres's
/// default LIKE escape character is backslash, so no `ESCAPE` clause is needed.
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

// ── /agents/search ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: i64,
}

/// Default page size for agent search — matches Python's `search_agents(limit=10)`.
/// (The `list` endpoint keeps its own larger default via `default_limit`.)
fn default_search_limit() -> i64 {
    10
}

/// One agent search hit: all agent fields plus its computed relevance `score`.
/// `_total` is the pre-limit match count (window function) — carried per row and
/// hoisted into the response envelope, not serialized on each item.
#[derive(Serialize, sqlx::FromRow)]
struct AgentSearchResult {
    #[serde(flatten)]
    #[sqlx(flatten)]
    agent: Agent,
    #[sqlx(rename = "_score")]
    score: f64,
    #[serde(skip)]
    #[sqlx(rename = "_total")]
    total: i64,
}

/// Agent search envelope — mirrors Python's `{agents, total, max_score}` and the
/// existing `UserSearchResponse` shape so both search surfaces are consistent.
#[derive(Serialize)]
struct AgentSearchResponse {
    agents: Vec<AgentSearchResult>,
    total: i64,
    max_score: f64,
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
        match claims.user_uuid() {
            Ok(id) => Some(id),
            Err(e) => return e.into_response(),
        }
    };

    // `COUNT(*) OVER()` yields the total match count (post-filter, pre-LIMIT) so
    // the envelope reports `total` without a second query.
    // Cast the score to double precision: the CASE/GREATEST numeric literals make
    // Postgres infer `numeric`, which sqlx cannot decode into f64.
    let sql = format!(
        r#"SELECT *, COUNT(*) OVER() AS _total FROM (
               SELECT *, ({AGENT_SCORE_SQL})::double precision AS _score
               FROM agents
               WHERE ({access})
           ) _s
           WHERE _score > 0
           ORDER BY _score DESC, name ASC
           LIMIT $2"#,
        access = agent_access_predicate("$3", "agents")
    );

    let result = sqlx
        ::query_as::<_, AgentSearchResult>(&sql)
        .bind(escape_like(&q))
        .bind(sq.limit.clamp(1, 50))
        .bind(owner_filter)
        .fetch_all(&state.db).await;

    match result {
        Ok(agents) => {
            // Rows are ORDER BY score DESC → first is the max; total is the shared
            // window count (0 when there are no hits).
            let max_score = agents.first().map(|a| a.score).unwrap_or(0.0);
            let total = agents.first().map(|a| a.total).unwrap_or(0);
            Json(AgentSearchResponse { agents, total, max_score }).into_response()
        }
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
    display_name: String,
    email: String,
    role: Option<String>,
    score: f64,
}

async fn search_users(
    State(state): State<AppState>,
    claims: Claims,
    Query(sq): Query<UserSearchQuery>
) -> impl IntoResponse {
    // The user directory (usernames + emails) is sensitive — restrict to superusers
    // (CAT-4). Previously any authenticated caller could enumerate every user's email.
    if !claims.is_superuser {
        return StatusCode::FORBIDDEN.into_response();
    }

    let q = sq.q.trim().to_string();
    if q.len() < 2 {
        return (StatusCode::BAD_REQUEST, "q must be at least 2 characters").into_response();
    }

    // Escaped term + a hard LIMIT so the endpoint can't be turned into a full-table
    // dump via a wildcard.
    let sql = format!(
        r#"SELECT id, username,
                  COALESCE(display_name, username) AS display_name,
                  COALESCE(email, '') AS email,
                  role::text AS role,
                  score
           FROM (
               SELECT id, username, display_name, email, role,
                      ({USER_SCORE_SQL})::double precision AS score
               FROM users
               WHERE deleted_at IS NULL
           ) _s
           WHERE score > 0
           ORDER BY score DESC, username ASC
           LIMIT 50"#
    );

    let result = sqlx
        ::query_as::<_, UserSearchResult>(&sql)
        .bind(escape_like(&q))
        .fetch_all(&state.db).await;

    match result {
        Ok(users) => {
            let total = users.len();
            Json(serde_json::json!({
                "data": users,
                "query": q,
                "total_matches": total,
                "showing": total,
            })).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "search_users: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

// ── GET /registry/user/agents ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct RegistryUserAgentsQuery {
    q: Option<String>,
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

/// Returns agents accessible to the current user: owned + public + explicitly granted.
/// Supports optional `?q` (name/description search), `?status`, `?limit`, `?offset`.
/// Superusers see all agents.
async fn registry_user_agents(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<RegistryUserAgentsQuery>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let agents = if claims.is_superuser {
        let pattern = q.q.as_deref().map(|s| format!("%{}%", s));
        sqlx::query_as::<_, Agent>(
            r#"SELECT * FROM agents
               WHERE ($1::text IS NULL OR (name ILIKE $1 OR description ILIKE $1))
                 AND ($2::text IS NULL OR status = $2)
               ORDER BY created_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(&pattern)
        .bind(&q.status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    } else {
        let pattern = q.q.as_deref().map(|s| format!("%{}%", s));
        sqlx::query_as::<_, Agent>(
            r#"SELECT * FROM agents
               WHERE (
                   owner_id = $1
                   OR is_public = true
                   OR EXISTS (
                       SELECT 1 FROM agent_grants ag
                       WHERE ag.agent_id = agents.id
                         AND ((ag.grant_type = 'public' AND ag.grantee_id = '*')
                           OR (ag.grant_type = 'user'   AND ag.grantee_id = $1::text))
                   )
               )
                 AND ($2::text IS NULL OR (name ILIKE $2 OR description ILIKE $2))
                 AND ($3::text IS NULL OR status = $3)
               ORDER BY created_at DESC
               LIMIT $4 OFFSET $5"#,
        )
        .bind(user_id)
        .bind(&pattern)
        .bind(&q.status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    };

    match agents {
        Ok(list) => {
            #[derive(serde::Serialize)]
            struct SimpleAgent {
                agent_id: Uuid,
                name: String,
                icon_url: Option<String>,
                tags: Vec<String>,
                description: Option<String>,
                owner_id: Uuid,
            }
            #[derive(serde::Serialize)]
            struct Response {
                data: Vec<SimpleAgent>,
                status_code: u16,
                message: String,
            }
            let data = list.into_iter().map(|a| SimpleAgent {
                agent_id: a.id,
                name: a.name,
                icon_url: a.icon_url,
                tags: a.tags,
                description: a.description,
                owner_id: a.owner_id,
            }).collect();
            Json(Response { data, status_code: 200, message: "success".to_string() }).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "registry_user_agents: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

// ── GET /registries/{id} ──────────────────────────────────────────────────────

/// Look up an agent by its UUID or name (registry-entry lookup).
/// Equivalent to GET /agents/{id} but exposed at the /registries/ path for
/// clients that use the registry-centric URL scheme.
async fn get_by_registry_id(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse {
    get_one(State(state), claims, Path(id)).await
}
