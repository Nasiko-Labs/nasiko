use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use nasiko_runtime::ContainerId;

use crate::auth::Claims;
use crate::state::AppState;

use super::models::{Agent, AgentSummary, AgentVersion, CreateAgent, UpdateAgent};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", post(create))
        .route("/agents", get(list))
        .route("/agents/{id}", get(get_one))
        .route("/agents/{id}", put(update))
        .route("/agents/{id}", axum::routing::delete(delete))
        .route("/agents/{id}/versions", get(list_versions))
        .route(
            "/agents/{id}/versions/{version}",
            axum::routing::delete(delete_version),
        )
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
/// `org_bind` carries the ids of agents reachable through an org-hierarchy grant
/// (`team`/`department`/`organization`), as resolved by
/// `AuthService::org_granted_agent_ids`. `EeAuthService::can_access_agent`
/// (ee/auth/src/lib.rs) grants access via team/department membership by joining on
/// `users.team_id` / `users.department_id` — columns that only exist after the EE
/// `1002_org_hierarchy` migration. This file is compiled into and shared by both
/// the OSS and EE server binaries (`ee/server` wraps this crate's router rather
/// than forking it — see `nasiko_server::build_app_with_user_router`), so a single
/// static SQL string here cannot reference those EE-only columns; the trait
/// resolves them per edition and hands back plain ids instead, which is what
/// closes the gap this comment used to describe. OSS returns an empty list, so
/// the extra disjunct never matches there.
fn agent_access_predicate(user_bind: &str, org_bind: &str, table_ref: &str) -> String {
    format!(
        r#"({user_bind}::uuid IS NULL
             OR {table_ref}.owner_id = {user_bind}
             OR {table_ref}.is_public = TRUE
             OR {table_ref}.id::text = ANY({org_bind})
             OR EXISTS (
                 SELECT 1 FROM agent_grants ag
                 WHERE ag.agent_id = {table_ref}.id
                   AND ((ag.grant_type = 'public' AND ag.grantee_id = '*')
                     OR (ag.grant_type = 'user'   AND ag.grantee_id = {user_bind}::text))
             ))"#
    )
}

/// The caller's id for scoping purposes, and the agents an org grant opens up.
///
/// `None` is the superuser bypass — [`agent_access_predicate`] short-circuits on
/// it, which is what the helper has always documented but never actually did:
/// every call site passed `Some(user_id)` unconditionally, so an admin saw
/// exactly what a `member` saw.
async fn listing_scope(state: &AppState, claims: &Claims) -> Result<ListingScope, Response> {
    if claims.is_superuser {
        return Ok(ListingScope {
            user: None,
            org_granted: Vec::new(),
        });
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return Err(e.into_response()),
    };
    let identity: nasiko_auth::Identity = claims.clone().into();
    Ok(ListingScope {
        user: Some(user_id),
        org_granted: state.auth.org_granted_agent_ids(&identity).await,
    })
}

struct ListingScope {
    user: Option<Uuid>,
    org_granted: Vec<String>,
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct BySkillQuery {
    /// Skill tag to match (case-insensitive).
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
#[utoipa::path(
    get,
    path = "/api/agents/by-skill",
    tag = "catalog",
    params(BySkillQuery),
    responses(
        (status = 200, description = "Agents with a matching skill tag", body = [AgentSummary]),
        (status = 400, description = "Missing or empty `tag`"),
    ),
)]
pub(crate) async fn by_skill(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<BySkillQuery>,
) -> impl IntoResponse {
    let tag = q.tag.trim();
    if tag.is_empty() {
        return (StatusCode::BAD_REQUEST, "tag is required").into_response();
    }
    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let scope = match listing_scope(&state, &claims).await {
        Ok(s) => s,
        Err(resp) => return resp,
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
        access = agent_access_predicate("$4", "$5", "a")
    );

    let result = sqlx::query_as::<_, AgentSummary>(&sql)
        .bind(&tag_lower)
        .bind(limit)
        .bind(offset)
        .bind(scope.user)
        .bind(&scope.org_granted)
        .fetch_all(&state.db)
        .await;

    match result {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::error!(%e, "by_skill: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

/// Register a new agent in the catalog.
#[utoipa::path(
    post,
    path = "/api/agents",
    tag = "catalog",
    request_body = CreateAgent,
    responses(
        (status = 201, description = "Agent registered", body = Agent),
        (status = 409, description = "An agent with this name already exists"),
    ),
)]
pub(crate) async fn create(
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
    let skills_vec = body.skills.unwrap_or_default();
    // Normalise tags to lowercase at write time for consistent GIN lookup.
    let mut tags: Vec<String> = body
        .tags
        .unwrap_or_default()
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
            r#"INSERT INTO agents (name, display_name, description, owner_id, url, icon_url, version, documentation_url, capabilities, skills, tags, metadata, image)
           VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '1.0.0'), $8, $9, $10, $11, $12, $13)
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
        .bind(&body.image)
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

    // Seed the version history with this agent's starting version. Without
    // this, `push`/`deploy`-registered agents have zero rows in
    // `agent_versions` until their first `reupload` — which then has nothing
    // active to archive, so `rollback` has no eligible target until a
    // *second* reupload. `nasiko upload`'s build pipeline seeds its own row
    // separately (`agents/upload.rs`, `build/routes.rs`) once the build
    // completes; this covers the `push`/`deploy` path, which registers
    // before any build job exists.
    //
    // Skipped when the version isn't a plain `x.y.z` — the `agents.version`
    // column itself stays free-form (some callers rely on creating an agent
    // with a legacy/non-semver version and then getting a clear error from a
    // later explicit-version-required update, rather than being blocked at
    // creation), but `agent_versions` history must never carry that free-form
    // text, which is the actual bug this seeding step must not reintroduce.
    if crate::agents::versions::parse_plain_version(&agent.version).is_some()
        && let Err(e) = sqlx::query(
            "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, status) \
             VALUES ($1, $2, $3, true, 'active')",
        )
        .bind(agent.id)
        .bind(&agent.version)
        .bind(agent.image.clone().unwrap_or_default())
        .execute(&mut *tx)
        .await
    {
        tracing::error!(%e, agent_id = %agent.id, "create agent: seed initial version failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(%e, "create agent: commit failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
    }

    (StatusCode::CREATED, Json(agent)).into_response()
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct ListQuery {
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

/// List agents visible to the caller (superuser → all; otherwise owner ∪
/// public ∪ user-grant — see `agent_access_predicate`).
#[utoipa::path(
    get,
    path = "/api/agents",
    tag = "catalog",
    params(ListQuery),
    responses(
        (status = 200, description = "Agents visible to the caller", body = [Agent]),
    ),
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let scope = match listing_scope(&state, &claims).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let sql = format!(
        r#"SELECT * FROM agents
           WHERE deleted_at IS NULL
             AND ($1::uuid IS NULL OR owner_id = $1)
             AND ({access})
             AND ($2::text IS NULL OR status = $2)
           ORDER BY created_at DESC
           LIMIT $4 OFFSET $5"#,
        access = agent_access_predicate("$3", "$6", "agents")
    );

    let agents = sqlx::query_as::<_, Agent>(&sql)
        .bind(q.owner)
        .bind(&q.status)
        .bind(scope.user)
        .bind(limit)
        .bind(offset)
        .bind(&scope.org_granted)
        .fetch_all(&state.db)
        .await;

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

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDetailResponse {
    id: Uuid,
    name: String,
    #[serde(rename = "display_name")]
    display_name: Option<String>,
    /// Owner's user UUID — lets the UI label the owner row in grant lists.
    #[serde(rename = "owner_id")]
    owner_id: Uuid,
    /// True when the caller may manage this agent (owner or superuser) —
    /// computed with the same predicate the mutating routes enforce
    /// (`crate::acl::can_manage_agent`), so the UI can gate its management
    /// tabs without guessing.
    #[serde(rename = "can_manage")]
    can_manage: bool,
    status: String,
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

#[derive(Serialize, ToSchema)]
pub(crate) struct SingleResponse {
    data: AgentDetailResponse,
    status_code: u16,
    message: String,
}

/// Fetch a single agent by UUID or name, rendered as an A2A AgentCard-shaped envelope.
#[utoipa::path(
    get,
    path = "/api/agents/{id}",
    tag = "catalog",
    params(
        ("id" = String, Path, description = "Agent UUID or name"),
    ),
    responses(
        (status = 200, description = "Agent card", body = SingleResponse),
        (status = 403, description = "Caller cannot access this agent"),
        (status = 404, description = "No agent with this id/name"),
    ),
)]
pub(crate) async fn get_one(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Soft-deleted agents must not resolve here: the `(owner_id, name)` uniqueness
    // constraint is a partial index scoped to `deleted_at IS NULL`
    // (`agents_owner_name_active_uniq`, oss/migrations/0001_schema.sql) — the schema's
    // own intent is that a deleted agent's name is free for a fresh row to reuse.
    // Without this filter, a caller redeploying under a previously-deleted name (e.g.
    // `nasiko deploy` re-checking "does this name already exist") found the old
    // deleted row instead of getting a clean "not found", updated it in place, and
    // left it permanently invisible to `nasiko ps`/`rm` (which do filter deleted_at)
    // even though its container was genuinely running again.
    let result = match id.parse::<Uuid>() {
        Ok(uuid) => {
            sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1 AND deleted_at IS NULL")
                .bind(uuid)
                .fetch_optional(&state.db)
                .await
        }
        Err(_) => {
            sqlx::query_as::<_, Agent>(
                "SELECT * FROM agents WHERE name = $1 AND deleted_at IS NULL",
            )
            .bind(&id)
            .fetch_optional(&state.db)
            .await
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

    let can_manage = crate::acl::can_manage_agent(&state, &claims, agent.id).await;

    let skills: Vec<serde_json::Value> = agent
        .skills
        .0
        .iter()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .collect();

    let data = AgentDetailResponse {
        id: agent.id,
        name: agent.name.clone(),
        display_name: agent.display_name.clone(),
        owner_id: agent.owner_id,
        can_manage,
        status: agent.status.clone(),
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
    })
    .into_response()
}

/// Records a version change if `body` includes a new version. This is what
/// `nasiko deploy`/`push` call on every redeploy, so version history and
/// rollback depend on it running here.
///
/// Runs inside the caller's transaction (`tx`) so the version-history write
/// and the catalog `agents` row update below either both commit or both roll
/// back — otherwise a later failure in the same request (e.g. the skills
/// sync, or the commit itself) would leave a new version active in
/// `agent_versions` while the `agents` row never changed to match.
///
/// Returns `Some(response)` to reject the request (bad/reused version, or a
/// DB error) — the caller should return that immediately. Returns `None` to
/// continue as normal.
async fn record_version_change_if_needed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    agent_id: Uuid,
    body: &UpdateAgent,
) -> Option<axum::response::Response> {
    let new_version = body.version.as_ref()?;

    let current: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT version, image FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&mut **tx)
            .await
            .ok()
            .flatten();
    let (current_version, current_image) = match current {
        Some((v, img)) => (Some(v), img),
        None => (None, None),
    };

    // Skip if the version hasn't changed — versions are immutable, so
    // there's no "refresh this version's content in place" path anymore.
    if current_version.as_deref() == Some(new_version.as_str()) {
        return None;
    }

    // A version-only update (no `image`) must keep the agent's current image —
    // an empty image_tag here would leave this history row's rollback target
    // pointing at no image at all.
    let image_tag = match body.image.as_deref() {
        Some(img) => img,
        None => current_image.as_deref().unwrap_or_default(),
    };
    let version_change = crate::agents::versions::VersionChange {
        agent_id,
        build_id: None,
        version: new_version,
        image_tag,
        changelog: None,
    };
    // `nasiko deploy`/`update` activate the version (default); `nasiko push`
    // sets `activate_version: false` — it only registers an image, so it
    // must not claim to be live or archive whatever's genuinely running.
    let result = if body.activate_version {
        crate::agents::versions::record_version_change_in_tx(tx, version_change).await
    } else {
        crate::agents::versions::record_pushed_version_in_tx(tx, version_change).await
    };
    match result {
        Ok(()) => None,
        Err(crate::agents::versions::VersionChangeError::VersionAlreadyExists(v)) => Some(
            (
                StatusCode::CONFLICT,
                format!("version {v} already exists for this agent — choose a distinct version"),
            )
                .into_response(),
        ),
        Err(e @ crate::agents::versions::VersionChangeError::InvalidVersion(_)) => {
            Some((StatusCode::BAD_REQUEST, e.to_string()).into_response())
        }
        Err(e) => {
            tracing::error!(%e, %agent_id, "update agent: record version change failed");
            Some((StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response())
        }
    }
}

/// Update an agent's catalog metadata. Owner-or-superuser only.
#[utoipa::path(
    put,
    path = "/api/agents/{id}",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    request_body = UpdateAgent,
    responses(
        (status = 200, description = "Updated agent", body = Agent),
        (status = 403, description = "Caller cannot manage this agent"),
        (status = 404, description = "No agent with this id"),
    ),
)]
pub(crate) async fn update(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAgent>,
) -> impl IntoResponse {
    // Mutation → owner-or-superuser only (an invoke/public grant must not confer edit).
    if !crate::acl::can_manage_agent(&state, &claims, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(%e, "update agent: begin tx");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    if let Some(resp) = record_version_change_if_needed(&mut tx, id, &body).await {
        return resp;
    }

    let skills_changed = body.skills.is_some();

    // When skills are being updated, merge their tags into the provided tag list so
    // the COALESCE write carries all skill-derived tags alongside any explicit ones.
    // All tags are normalised to lowercase for consistent GIN lookup.
    let merged_tags = if let Some(ref skill_list) = body.skills {
        let mut tags: Vec<String> = body
            .tags
            .clone()
            .unwrap_or_default()
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
        body.tags
            .as_ref()
            .map(|ts| ts.iter().map(|t| t.to_lowercase()).collect())
    };

    // `push` (activate_version = false) must not move `agents.version`/`image` —
    // those columns mean "what's currently deployed", and push never deploys
    // anything. Only a real deploy/update advances them.
    let (agent_version, agent_image) = if body.activate_version {
        (body.version.clone(), body.image.clone())
    } else {
        (None, None)
    };

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
    .bind(&agent_version)
    .bind(&body.documentation_url)
    .bind(&body.capabilities)
    .bind(
        body.skills
            .as_ref()
            .and_then(|s| serde_json::to_value(s).ok()),
    )
    .bind(&merged_tags)
    .bind(&body.metadata)
    .bind(&body.status)
    .bind(&agent_image)
    .fetch_optional(&mut *tx)
    .await;

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
    if skills_changed
        && let Err(e) = super::skills::sync_agent_skills(&mut tx, agent.id, &agent.skills.0).await
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

#[derive(Serialize, ToSchema)]
pub(crate) struct DeletedAgent {
    deleted: bool,
    agent_id: Uuid,
    containers_stopped: usize,
    runtime_errors: Vec<String>,
}

/// Delete an agent and tear down its running containers (best-effort).
/// Owner-or-superuser only.
#[utoipa::path(
    delete,
    path = "/api/agents/{id}",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    responses(
        (status = 200, description = "Deleted", body = DeletedAgent),
        (status = 403, description = "Caller cannot manage this agent"),
        (status = 404, description = "No agent with this id"),
    ),
)]
pub(crate) async fn delete(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if !crate::acl::can_manage_agent(&state, &claims, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Claim the delete up front: one statement is both the existence check and
    // the mutual exclusion, and it hands back the primary container name needed
    // for teardown.
    //
    // This route soft-deletes, so `deleted_at IS NULL` is what makes a repeat
    // delete 404 instead of re-stamping the row and reporting success forever.
    // Doing it as a single `UPDATE ... RETURNING` rather than SELECT-then-UPDATE
    // also means only the caller that actually flipped `deleted_at` proceeds —
    // two concurrent deletes would otherwise both pass a separate SELECT and
    // both run the whole container teardown below before one of them lost.
    //
    // Teardown therefore runs *after* the row is marked. A runtime failure
    // still leaves the agent deleted, which is the pre-existing behavior: the
    // errors are reported in `runtime_errors` rather than rolling the delete
    // back.
    let name: String = match sqlx::query_scalar(
        "UPDATE agents SET deleted_at = NOW() \
         WHERE id = $1 AND deleted_at IS NULL \
         RETURNING name",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(n)) => n,
        Ok(None) => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => {
            tracing::error!(%e, %id, "delete agent: claim soft delete");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };

    // Every real deploy path keys the running container on the agent's UUID, never the
    // display name (see build_agent_spec's doc comment) — so the UUID-keyed id must always
    // be tried, not just when an `agent_deployments` row happens to confirm it. Relying
    // solely on that join left agents deployed before this row existed (or never tracked
    // for any other reason) permanently un-removable: the name-keyed guess below found
    // nothing, `destroy` no-op'd successfully, and `nasiko rm` reported success while the
    // real container kept running.
    //
    // Collect distinct K8s workload names from non-stopped deployment rows too — for
    // pre-UUID-keying legacy containers, `k8s_deployment_name` may be the only place the
    // real identifier was recorded. `name` is kept as a last-resort fallback for
    // containers created before UUID-keying existed at all.
    let k8s_names: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT k8s_deployment_name FROM agent_deployments
         WHERE agent_id = $1 AND status != 'stopped' AND k8s_deployment_name IS NOT NULL",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut containers_to_stop: Vec<String> = vec![id.to_string(), name.clone()];
    for kn in k8s_names {
        if !containers_to_stop.contains(&kn) {
            containers_to_stop.push(kn);
        }
    }

    // Tear down all identified containers before deleting DB records (best-effort).
    let mut containers_stopped = 0usize;
    let mut runtime_errors: Vec<String> = vec![];
    for container_name in &containers_to_stop {
        match state
            .runtime
            .destroy(&ContainerId::new(container_name))
            .await
        {
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

    // The row was already marked by the claiming UPDATE above, so reaching here
    // means this caller owns the delete — nothing left to decide.
    (
        StatusCode::OK,
        Json(DeletedAgent {
            deleted: true,
            agent_id: id,
            containers_stopped,
            runtime_errors,
        }),
    )
        .into_response()
}

/// List an agent's build/version history.
#[utoipa::path(
    get,
    path = "/api/agents/{id}/versions",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
    ),
    responses(
        (status = 200, description = "Version history, newest first", body = [AgentVersion]),
        (status = 403, description = "Caller cannot access this agent"),
    ),
)]
pub(crate) async fn list_versions(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if !crate::acl::can_access_agent(&state, &claims, id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let result = sqlx::query_as::<_, AgentVersion>(
        "SELECT * FROM agent_versions WHERE agent_id = $1 ORDER BY created_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await;

    match result {
        Ok(versions) => Json(serde_json::json!({
            "data": versions,
            "status_code": 200,
            "message": "version history retrieved successfully",
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(%e, agent_id = %id, "list_versions: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ── DELETE /agents/{id}/versions/{version} ────────────────────────────────────

/// Delete a specific version record. Rejects deleting the currently active
/// version — roll back to another version first. Owner-or-superuser only.
#[utoipa::path(
    delete,
    path = "/api/agents/{id}/versions/{version}",
    tag = "catalog",
    params(
        ("id" = Uuid, Path, description = "Agent id"),
        ("version" = String, Path, description = "Version tag to delete"),
    ),
    responses(
        (status = 204, description = "Version deleted"),
        (status = 403, description = "Caller cannot manage this agent"),
        (status = 404, description = "No such version"),
        (status = 409, description = "Cannot delete the active version"),
    ),
)]
pub(crate) async fn delete_version(
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
        return (
            StatusCode::CONFLICT,
            "cannot delete the active version — rollback first",
        )
            .into_response();
    }

    match sqlx::query("DELETE FROM agent_versions WHERE agent_id = $1 AND version = $2")
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

const AGENT_SCORE_SQL: &str = r#"
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

const USER_SCORE_SQL: &str = r#"
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
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ── /agents/search ────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
pub(crate) struct SearchQuery {
    /// Search term, minimum 2 characters.
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
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct AgentSearchResult {
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
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentSearchResponse {
    agents: Vec<AgentSearchResult>,
    total: i64,
    max_score: f64,
}

/// Agent-only search.  Scoring: GREATEST across (name×2.8, description×2.0, tag score) with tiered
/// exact/prefix/contains scoring.  Minimum query length: 2 chars.
#[utoipa::path(
    get,
    path = "/api/search/agents",
    tag = "catalog",
    params(SearchQuery),
    responses(
        (status = 200, description = "Ranked agent search hits", body = AgentSearchResponse),
        (status = 400, description = "`q` shorter than 2 characters"),
    ),
)]
pub(crate) async fn search(
    State(state): State<AppState>,
    claims: Claims,
    Query(sq): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = sq.q.trim().to_string();
    if q.len() < 2 {
        return (StatusCode::BAD_REQUEST, "q must be at least 2 characters").into_response();
    }

    let scope = match listing_scope(&state, &claims).await {
        Ok(s) => s,
        Err(resp) => return resp,
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
        access = agent_access_predicate("$3", "$4", "agents")
    );

    let result = sqlx::query_as::<_, AgentSearchResult>(&sql)
        .bind(escape_like(&q))
        .bind(sq.limit.clamp(1, 50))
        .bind(scope.user)
        .bind(&scope.org_granted)
        .fetch_all(&state.db)
        .await;

    match result {
        Ok(agents) => {
            // Rows are ORDER BY score DESC → first is the max; total is the shared
            // window count (0 when there are no hits).
            let max_score = agents.first().map(|a| a.score).unwrap_or(0.0);
            let total = agents.first().map(|a| a.total).unwrap_or(0);
            Json(AgentSearchResponse {
                agents,
                total,
                max_score,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "agents search: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

// ── /search/users ─────────────────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
pub(crate) struct UserSearchQuery {
    /// Search term, minimum 2 characters.
    q: String,
}

/// User search. Field boosts: username×3.0, display_name×2.5, email×1.5.
/// Scoring: exact (100×boost) > prefix (90×boost) > contains (70×boost).
/// Minimum query length: 2 chars. Sort: score DESC, username ASC.
/// Returns all matching users (no limit).
#[derive(Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct UserSearchResult {
    id: Uuid,
    username: String,
    display_name: String,
    email: String,
    role: Option<String>,
    score: f64,
}

/// Response envelope for `/search/users` — documents the shape of the ad hoc
/// `serde_json::json!` object the handler returns.
#[derive(Serialize, ToSchema)]
pub(crate) struct UserSearchResponse {
    data: Vec<UserSearchResult>,
    query: String,
    total_matches: usize,
    showing: usize,
}

/// Search the user directory. The user directory (usernames + emails) is
/// sensitive (CAT-4), so results are scoped via `AuthService::org_visible_user_ids`
/// — OSS returns `None` (unrestricted, no org hierarchy to scope by); EE
/// restricts non-admin callers to their own department/team, same as the MCP
/// share-target picker (`oss/mcp-gateway`'s `search_share_targets_view`).
/// An exact username/display-name match bypasses that scope, mirroring
/// `resolve_share_target`/`departments.rs::resolve`/`teams.rs::resolve` —
/// a caller who already knows exactly who they're looking for (e.g. a
/// connector owner sharing outside their own team) can still find them by
/// typing the full name, they just can't browse/enumerate people outside
/// their scope via a partial query.
#[utoipa::path(
    get,
    path = "/api/search/users",
    tag = "catalog",
    params(UserSearchQuery),
    responses(
        (status = 200, description = "Ranked user search hits (max 50)", body = UserSearchResponse),
        (status = 400, description = "`q` shorter than 2 characters"),
    ),
)]
pub(crate) async fn search_users(
    State(state): State<AppState>,
    claims: Claims,
    Query(sq): Query<UserSearchQuery>,
) -> impl IntoResponse {
    let q = sq.q.trim().to_string();
    if q.len() < 2 {
        return (StatusCode::BAD_REQUEST, "q must be at least 2 characters").into_response();
    }

    let identity: nasiko_auth::Identity = claims.clone().into();
    let visible_ids: Option<Vec<Uuid>> = state
        .auth
        .org_visible_user_ids(&identity)
        .await
        .map(|ids| ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect());

    // Escaped term + a hard LIMIT so the endpoint can't be turned into a full-table
    // dump via a wildcard. The extra AND clause: unrestricted (superuser/admin),
    // OR in the caller's visible set, OR an exact username/display_name match —
    // see the doc comment above for why the exact-match escape hatch exists.
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
             AND ($2::uuid[] IS NULL OR id = ANY($2)
                  OR lower(username) = lower($3) OR lower(display_name) = lower($3))
           ORDER BY score DESC, username ASC
           LIMIT 50"#
    );

    let result = sqlx::query_as::<_, UserSearchResult>(&sql)
        .bind(escape_like(&q))
        .bind(&visible_ids)
        .bind(&q)
        .fetch_all(&state.db)
        .await;

    match result {
        Ok(users) => {
            let total = users.len();
            Json(serde_json::json!({
                "data": users,
                "query": q,
                "total_matches": total,
                "showing": total,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "search_users: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

// ── GET /registry/user/agents ─────────────────────────────────────────────────

#[derive(Deserialize, IntoParams)]
pub(crate) struct RegistryUserAgentsQuery {
    q: Option<String>,
    status: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RegistryUserAgentSummary {
    agent_id: Uuid,
    name: String,
    icon_url: Option<String>,
    tags: Vec<String>,
    description: Option<String>,
    owner_id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RegistryUserAgentsResponse {
    data: Vec<RegistryUserAgentSummary>,
    status_code: u16,
    message: String,
}

/// Returns agents accessible to the current user: owned + public + explicitly granted.
/// Supports optional `?q` (name/description search), `?status`, `?limit`, `?offset`.
/// Superusers see all agents.
#[utoipa::path(
    get,
    path = "/api/registry/user/agents",
    tag = "catalog",
    params(RegistryUserAgentsQuery),
    responses(
        (status = 200, description = "Agents accessible to the caller", body = RegistryUserAgentsResponse),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn registry_user_agents(
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

    // All users (including admins) see: owned + public + explicitly granted agents.
    let pattern = q.q.as_deref().map(|s| format!("%{}%", s));
    let agents = sqlx::query_as::<_, Agent>(
        r#"SELECT * FROM agents
           WHERE deleted_at IS NULL
             AND (
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
    .await;

    match agents {
        Ok(list) => {
            let data = list
                .into_iter()
                .map(|a| RegistryUserAgentSummary {
                    agent_id: a.id,
                    name: a.name,
                    icon_url: a.icon_url,
                    tags: a.tags,
                    description: a.description,
                    owner_id: a.owner_id,
                })
                .collect();
            Json(RegistryUserAgentsResponse {
                data,
                status_code: 200,
                message: "success".to_string(),
            })
            .into_response()
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
#[utoipa::path(
    get,
    path = "/api/registries/{id}",
    tag = "catalog",
    params(
        ("id" = String, Path, description = "Agent UUID or name"),
    ),
    responses(
        (status = 200, description = "Agent card", body = SingleResponse),
        (status = 403, description = "Caller cannot access this agent"),
        (status = 404, description = "No agent with this id/name"),
    ),
)]
pub(crate) async fn get_by_registry_id(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse {
    get_one(State(state), claims, Path(id)).await
}
