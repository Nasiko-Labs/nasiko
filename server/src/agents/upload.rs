use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post, put},
};
use serde::Serialize;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::auth::Claims;
<<<<<<< HEAD:oss/server/src/agents/upload.rs
use crate::build::{self, BuildStatus, routes::extract_zip_to_dir};
=======
use crate::build::{self, BuildStatus, download_repo_tarball};
use crate::build::routes::extract_zip_to_dir;
use crate::catalog::agent_secrets;
use crate::github::load_github_token;
>>>>>>> a6aa95b (add update and rollback endpoints with lifecycle tracking):oss/server/src/agents/routes.rs
use crate::state::AppState;

use super::utils::{set_build_status, set_upload_status};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload-and-deploy",        post(upload_and_deploy))
        .route("/deploy-status/{build_id}", get(deploy_status_sse))
        .route("/upload-status/{upload_id}", get(get_upload_status))
<<<<<<< HEAD:oss/server/src/agents/upload.rs
=======
        .route("/deployments", get(list_deployments))
        .route("/deployment/{deployment_id}/restart", post(restart_deployment))
        .route("/{id}/deployment", get(get_agent_deployment))
        .route("/{id}/acl", get(get_agent_acl))
        .route("/{id}/acl", post(add_agent_acl))
        .route("/{id}/acl/{target_id}", delete(remove_agent_acl))
        .route("/{id}/update", put(update_agent))
        .route("/{id}/rollback", post(rollback_agent))
>>>>>>> a6aa95b (add update and rollback endpoints with lifecycle tracking):oss/server/src/agents/routes.rs
}

pub fn user_routes() -> Router<AppState> {
    Router::new()
        .route("/upload-agents", get(list_upload_agents))
}

#[derive(Debug, Serialize)]
pub struct UploadAndDeployResponse {
    pub build_id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub image_tag: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct UploadStatusRow {
    id: Uuid,
    upload_id: String,
    agent_id: Option<Uuid>,
    agent_name: String,
    status: String,
    owner_id: Option<Uuid>,
    error_message: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct BuildStatusRow {
    status: BuildStatus,
}

// ─── POST /upload-and-deploy ─────────────────────────────────────────────────

async fn upload_and_deploy(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    let mut name: Option<String> = None;
    let mut version_tag: Option<String> = None;
    let mut source_data: Option<Vec<u8>> = None;
    let mut ports: Vec<u16> = vec![];
    let mut env: HashMap<String, String> = HashMap::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "source" => {
                let data = field.bytes().await.unwrap_or_default();
                if !data.is_empty() {
                    source_data = Some(data.to_vec());
                }
            }
            "ports" => {
                if let Ok(text) = field.text().await {
                    ports = text
                        .split(',')
                        .filter_map(|p| p.trim().parse().ok())
                        .collect();
                }
            }
            "env" => {
                if let Ok(text) = field.text().await
                    && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) {
                        env = map;
                    }
            }
            _ => {}
        }
    }

    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => return (StatusCode::BAD_REQUEST, "name is required").into_response(),
    };
    let version_tag = version_tag.unwrap_or_else(|| "latest".to_string());
    let source_data = match source_data {
        Some(d) => d,
        None => return (StatusCode::BAD_REQUEST, "source zip is required").into_response(),
    };

    let image_tag = format!("{name}:{version_tag}");

    // Upsert the agent. Agent names are not globally unique (migration 006 dropped
    // the unique constraint), so scope the lookup to this owner instead of
    // relying on ON CONFLICT (name).
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agents WHERE owner_id = $1 AND name = $2 LIMIT 1",
    )
    .bind(owner_id)
    .bind(&name)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let agent_id = if let Some(id) = existing {
        let _ = sqlx::query(
            "UPDATE agents SET version = $2, image = $3, status = 'deploying', updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&version_tag)
        .bind(&image_tag)
        .execute(&state.db)
        .await;
        id
    } else {
        match sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agents (name, owner_id, version, image, status) \
             VALUES ($1, $2, $3, $4, 'deploying') RETURNING id",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(&version_tag)
        .bind(&image_tag)
        .fetch_one(&state.db)
        .await
        {
            Ok(id) => id,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("register agent: {e}"))
                    .into_response();
            }
        }
    };

    // Persist a build record (status defaults to 'queued').
    let build_id = match sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(agent_id)
    .bind(&version_tag)
    .bind(&image_tag)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("create build record: {e}"))
                .into_response();
        }
    };

    let runtime = state.runtime.clone();
    let db = state.db.clone();
    let name_clone = name.clone();
    let image_tag_clone = image_tag.clone();
    let ports_clone = if ports.is_empty() { vec![8000] } else { ports };
    // Use build_id as the upload_id so the client can poll both SSE and REST with the same ID.
    let upload_id = build_id.to_string();

    tokio::spawn(async move {
        execute_upload_and_deploy(
            runtime,
            db,
            build_id,
            agent_id,
            owner_id,
            upload_id,
            name_clone,
            source_data,
            image_tag_clone,
            ports_clone,
            env,
        )
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(UploadAndDeployResponse {
            build_id,
            agent_id,
            name,
            image_tag,
            status: "queued",
        }),
    )
        .into_response()
}

#[allow(clippy::too_many_arguments)]
async fn execute_upload_and_deploy(
    runtime: std::sync::Arc<dyn nasiko_runtime::ContainerRuntime>,
    db: sqlx::PgPool,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    upload_id: String,
    name: String,
    source_data: Vec<u8>,
    image_tag: String,
    ports: Vec<u16>,
    env: HashMap<String, String>,
) {
    set_build_status(&db, build_id, BuildStatus::Building).await;
    set_upload_status(&db, &upload_id, &name, owner_id, "initiated", None, None).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-agent-{build_id}"));

    let result: Result<(), String> = async {
        // Extract zip.
        extract_zip_to_dir(&source_data, &tmp_dir)?;
        set_upload_status(&db, &upload_id, &name, owner_id, "processing", None, None).await;

        let dockerfile_path = tmp_dir.join("Dockerfile");
        if !dockerfile_path.exists() {
            return Err("no Dockerfile found in source zip".into());
        }

        // Patch Dockerfile to inject OTel auto-instrumentation.
        let original = tokio::fs::read_to_string(&dockerfile_path)
            .await
            .map_err(|e| format!("read Dockerfile: {e}"))?;
        let patched = nasiko_observability::patch_dockerfile_for_otel(&original);
        if patched != original {
            tokio::fs::write(&dockerfile_path, &patched)
                .await
                .map_err(|e| format!("write Dockerfile: {e}"))?;
            tracing::info!(build_id = %build_id, "patched Dockerfile with OTel instrumentation");
        }

        // Build Docker image.
        let tar_bytes = build::tar_directory(&tmp_dir)
            .map_err(|e| format!("tar source: {e}"))?;
        runtime
            .build(&tar_bytes, &image_tag)
            .await
            .map_err(|e| format!("docker build: {e}"))?;

        set_upload_status(&db, &upload_id, &name, owner_id, "orchestration_triggered", None, None).await;

        // Deploy container.
        let container_id = ContainerId::new(&name);
        let spec = DeploymentSpec {
            container_id,
            name: name.clone(),
            image: image_tag.clone(),
            ports,
            env_vars: env,
            min_replicas: 1,
            max_replicas: 1,
            resources: None,
        };
        runtime
            .deploy(&spec)
            .await
            .map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(&db, &upload_id, &name, owner_id, "orchestration_processing", None, None).await;

        Ok(())
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(()) => {
            set_build_status(&db, build_id, BuildStatus::Success).await;
            set_upload_status(&db, &upload_id, &name, owner_id, "completed", Some(agent_id), None).await;
            // Record the built version (idempotent on agent_id+version).
            let _ = sqlx::query(
                "INSERT INTO agent_versions (agent_id, build_id, version, image_tag, is_active) \
                 SELECT agent_id, $1, version_tag, image_reference, false FROM agent_builds WHERE id = $1 \
                 ON CONFLICT (agent_id, version) DO UPDATE \
                   SET build_id = EXCLUDED.build_id, image_tag = EXCLUDED.image_tag",
            )
            .bind(build_id)
            .execute(&db)
            .await;
            let _ = sqlx::query("UPDATE agents SET status = 'running', updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .execute(&db)
                .await;
            tracing::info!(build_id = %build_id, agent_id = %agent_id, "upload-and-deploy succeeded");
        }
        Err(e) => {
            set_build_status(&db, build_id, BuildStatus::Failed).await;
            set_upload_status(&db, &upload_id, &name, owner_id, "failed", None, Some(&e)).await;
            let _ = sqlx::query("UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .execute(&db)
                .await;
            tracing::error!(build_id = %build_id, %e, "upload-and-deploy failed");
        }
    }
}

// ─── GET /deploy-status/{build_id} (SSE) ─────────────────────────────────────

async fn deploy_status_sse(
    State(state): State<AppState>,
    Path(build_id): Path<Uuid>,
) -> impl IntoResponse {
    let db = state.db.clone();

    let stream = async_stream::stream! {
        let mut last_status: Option<BuildStatus> = None;

        loop {
            let row: Option<BuildStatusRow> = sqlx::query_as(
                "SELECT status FROM agent_builds WHERE id = $1"
            )
            .bind(build_id)
            .fetch_optional(&db)
            .await
            .ok()
            .flatten();

            let Some(row) = row else {
                yield Ok::<_, Infallible>(Event::default().data(
                    serde_json::json!({"status": "not_found"}).to_string()
                ));
                break;
            };

            if Some(row.status) != last_status {
                last_status = Some(row.status);
                yield Ok(Event::default().data(
                    serde_json::json!({
                        "status": row.status,
                        "build_id": build_id,
                    })
                    .to_string(),
                ));
            }

            if matches!(row.status, BuildStatus::Success | BuildStatus::Failed) {
                break;
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ─── GET /upload-status/{upload_id} ─────────────────────────────────────────

async fn get_upload_status(
    State(state): State<AppState>,
    Path(upload_id): Path<String>,
) -> impl IntoResponse {
    match sqlx::query_as::<_, UploadStatusRow>(
        "SELECT id, upload_id, agent_id, agent_name, status::text as status, owner_id, error_message, created_at, updated_at
         FROM upload_status WHERE upload_id = $1",
    )
    .bind(&upload_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, upload_id, "get_upload_status db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── GET /user/upload-agents ─────────────────────────────────────────────────

async fn list_upload_agents(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let rows = if claims.is_superuser {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT id, upload_id, agent_id, agent_name, status::text as status, owner_id, error_message, created_at, updated_at
             FROM upload_status ORDER BY created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT id, upload_id, agent_id, agent_name, status::text as status, owner_id, error_message, created_at, updated_at
             FROM upload_status WHERE owner_id = $1 ORDER BY created_at DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_upload_agents db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── Deployment routes ────────────────────────────────────────────────────────

async fn list_deployments(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let rows = if claims.is_superuser {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             WHERE d.owner_id = $1
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_deployments db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_agent_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match sqlx::query_as::<_, DeploymentRow>(
        "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                d.status::text as status, d.service_url, d.owner_id,
                a.name as agent_name, d.created_at
         FROM agent_deployments d
         LEFT JOIN agents a ON a.id = d.agent_id
         WHERE d.agent_id = $1 AND d.status != 'stopped'
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "get_agent_deployment db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(sqlx::FromRow)]
struct AgentDeployInfo {
    name: String,
    image: String,
    agent_id: Uuid,
    build_id: Option<Uuid>,
    owner_id: Option<Uuid>,
}

async fn restart_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(deployment_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Fetch deployment and agent info together.
    let info = match sqlx::query_as::<_, AgentDeployInfo>(
        "SELECT a.name, a.image, a.id as agent_id,
                d.build_id, d.owner_id
         FROM agent_deployments d
         JOIN agents a ON a.id = d.agent_id
         WHERE d.id = $1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %deployment_id, "restart_deployment: fetch error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Owner or superuser only.
    if !claims.is_superuser {
        let is_owner = info.owner_id.map(|o| o == user_id).unwrap_or(false);
        if !is_owner && !user_can_access_agent(&state.db, user_id, info.agent_id).await {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let container_id = ContainerId::new(&info.name);

    // Destroy current container (ignore failure — may already be stopped).
    let _ = state.runtime.destroy(&container_id).await;

    // Resolve agent secrets for environment.
    let secrets = agent_secrets::resolve_agent_env(&state.db, info.agent_id).await;

    let spec = DeploymentSpec {
        container_id: ContainerId::new(&info.name),
        name: info.name.clone(),
        image: info.image.clone(),
        ports: vec![8000],
        env_vars: secrets,
        min_replicas: 1,
        max_replicas: 1,
        resources: None,
    };

    if let Err(e) = state.runtime.deploy(&spec).await {
        tracing::error!(%e, %deployment_id, "restart_deployment: deploy failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("deploy failed: {e}")).into_response();
    }

    // Mark old deployment stopped, record new one.
    let _ = sqlx::query(
        "UPDATE agent_deployments SET status = 'stopped', updated_at = now()
         WHERE id = $1",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await;

    if let Some(build_id) = info.build_id {
        let _ = sqlx::query(
            "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id)
             VALUES ($1, $2, 'running', $3)",
        )
        .bind(info.agent_id)
        .bind(build_id)
        .bind(info.owner_id)
        .execute(&state.db)
        .await;
    }

    let _ = sqlx::query("UPDATE agents SET status = 'running', updated_at = now() WHERE id = $1")
        .bind(info.agent_id)
        .execute(&state.db)
        .await;

    StatusCode::OK.into_response()
}

// ─── ACL routes ──────────────────────────────────────────────────────────────

async fn get_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match allowed_targets(&state.db, agent_id).await {
        Ok(None) => Json(AclResponse { unrestricted: true, allowed: vec![] }).into_response(),
        Ok(Some(targets)) => Json(AclResponse { unrestricted: false, allowed: targets }).into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "get_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn add_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<AddAclBody>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Verify target agent exists.
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(body.target_agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !target_exists {
        return (StatusCode::NOT_FOUND, "target agent not found").into_response();
    }

    match sqlx::query(
        "INSERT INTO agent_acl (caller_agent_id, target_agent_id, granted_by)
         VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(agent_id)
    .bind(body.target_agent_id)
    .bind(user_id)
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "add_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn remove_agent_acl(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, target_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    if !claims.is_superuser && !user_can_access_agent(&state.db, user_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    match sqlx::query(
        "DELETE FROM agent_acl WHERE caller_agent_id = $1 AND target_agent_id = $2",
    )
    .bind(agent_id)
    .bind(target_id)
    .execute(&state.db)
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, %target_id, "remove_agent_acl db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── PUT /{id}/update ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct UpdateAgentResponse {
    build_id: Uuid,
    agent_id: Uuid,
    new_version: String,
    previous_version: String,
    status: &'static str,
}

async fn update_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    // Fetch agent — verify it exists and capture state we need for the update.
    // Include `image` so we can roll it back if the build fails.
    let agent: Option<(String, String, Option<String>, Uuid)> = match sqlx::query_as(
        "SELECT name, version, image, owner_id FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, %agent_id, "update_agent: db error fetching agent");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let (agent_name, current_version, prev_image, _agent_owner_id) = match agent {
        Some(r) => r,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Superusers bypass ACL; everyone else needs owner or explicit ACL grant.
    if !claims.is_superuser && !user_can_access_agent(&state.db, owner_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Parse multipart fields.
    let mut source_data: Option<Vec<u8>> = None;
    let mut requested_version: Option<String> = None;
    let mut changelog: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "source" => {
                // Validate file extension before buffering the entire payload.
                let fname = field.file_name().unwrap_or("").to_string();
                if !fname.is_empty() && !fname.ends_with(".zip") {
                    return (StatusCode::BAD_REQUEST, "source must be a .zip file").into_response();
                }
                let data = field.bytes().await.unwrap_or_default();
                if !data.is_empty() {
                    source_data = Some(data.to_vec());
                }
            }
            "version" => {
                requested_version = field.text().await.ok().filter(|s| !s.is_empty());
            }
            "changelog" => {
                changelog = field.text().await.ok().filter(|s| !s.is_empty());
            }
            _ => {}
        }
    }

    // Determine new version.
    // Accepts strategy keywords (auto, patch, minor, major) for Python-client compat,
    // or an explicit semver string (e.g. "1.2.3").
    let new_version = match requested_version.as_deref() {
        None | Some("auto") | Some("patch") => {
            if semver::Version::parse(&current_version).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "current version '{current_version}' is not valid semver — \
                         supply an explicit version (e.g. 1.0.0)"
                    ),
                )
                    .into_response();
            }
            bump_patch_version(&current_version)
        }
        Some("minor") => {
            if semver::Version::parse(&current_version).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "current version '{current_version}' is not valid semver — \
                         supply an explicit version (e.g. 1.0.0)"
                    ),
                )
                    .into_response();
            }
            bump_minor_version(&current_version)
        }
        Some("major") => {
            if semver::Version::parse(&current_version).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "current version '{current_version}' is not valid semver — \
                         supply an explicit version (e.g. 1.0.0)"
                    ),
                )
                    .into_response();
            }
            bump_major_version(&current_version)
        }
        Some(v) => {
            let new_sv = match semver::Version::parse(v) {
                Ok(sv) => sv,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        "version must be valid semver (e.g. 1.2.3) or a strategy keyword: auto, patch, minor, major",
                    )
                        .into_response()
                }
            };
            // Enforce strictly-greater-than only when current is valid semver too.
            if let Ok(cur) = semver::Version::parse(&current_version)
                && new_sv <= cur
            {
                return (
                    StatusCode::CONFLICT,
                    format!("version {v} must be greater than current {current_version}"),
                )
                    .into_response();
            }
            v.to_string()
        }
    };

    // 409 if this exact version was already successfully built.
    let version_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_versions WHERE agent_id = $1 AND version = $2)",
    )
    .bind(agent_id)
    .bind(&new_version)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if version_exists {
        return (
            StatusCode::CONFLICT,
            format!("version {new_version} already exists for this agent"),
        )
            .into_response();
    }

    let image_tag = format!("{agent_name}:{new_version}");

    // Optimistic write — lets the UI show "deploying to <image_tag>".
    // On failure the background task rolls both version and image back.
    let _ = sqlx::query(
        "UPDATE agents SET status = 'deploying', version = $2, image = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(agent_id)
    .bind(&new_version)
    .bind(&image_tag)
    .execute(&state.db)
    .await;

    // Create a build record.
    let build_id = match sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, triggered_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(agent_id)
    .bind(&new_version)
    .bind(&image_tag)
    .bind(owner_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create build record: {e}"),
            )
                .into_response()
        }
    };

    let prev_version = current_version.clone();
    let state_clone = state.clone();
    let name_clone = agent_name.clone();
    let image_tag_clone = image_tag.clone();
    let new_version_clone = new_version.clone();

    tokio::spawn(async move {
        execute_agent_update(
            state_clone,
            build_id,
            agent_id,
            owner_id,
            name_clone,
            source_data,
            image_tag_clone,
            new_version_clone,
            prev_version,
            prev_image,
            changelog,
        )
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(UpdateAgentResponse {
            build_id,
            agent_id,
            new_version,
            previous_version: current_version,
            status: "queued",
        }),
    )
        .into_response()
}

fn bump_patch_version(current: &str) -> String {
    semver::Version::parse(current)
        .map(|mut v| { v.patch += 1; v.to_string() })
        .unwrap_or_else(|_| format!("{current}.1"))
}

fn bump_minor_version(current: &str) -> String {
    semver::Version::parse(current)
        .map(|mut v| { v.minor += 1; v.patch = 0; v.to_string() })
        .unwrap_or_else(|_| format!("{current}.1"))
}

fn bump_major_version(current: &str) -> String {
    semver::Version::parse(current)
        .map(|mut v| { v.major += 1; v.minor = 0; v.patch = 0; v.to_string() })
        .unwrap_or_else(|_| format!("{current}.1"))
}

/// Look up the stored GitHub source for an agent and decrypt the owner's token.
/// Returns `(full_repo, token)` where `full_repo` is `"owner/repo"`.
async fn resolve_agent_github_source(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    owner_id: Uuid,
) -> Option<(String, String)> {
    let github_url: String = sqlx::query_scalar(
        "SELECT github_url FROM agent_builds \
         WHERE agent_id = $1 AND github_url IS NOT NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()?;

    // Normalise the stored URL to "owner/repo":
    // handles https://github.com/owner/repo, https://github.com/owner/repo.git,
    // git@github.com:owner/repo, and bare owner/repo strings.
    let normalised = github_url.trim_end_matches('/');
    let bare = normalised
        .strip_prefix("git@github.com:")
        .unwrap_or(normalised)
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/");
    let bare = bare.strip_suffix(".git").unwrap_or(bare);

    if !build::is_valid_repo_name(bare) {
        tracing::warn!(agent_id = %agent_id, raw_url = %github_url, "stored github_url is not a valid owner/repo");
        return None;
    }

    let token = load_github_token(db, owner_id).await?;
    Some((bare.to_string(), token))
}

#[allow(clippy::too_many_arguments)]
async fn execute_agent_update(
    state: AppState,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    name: String,
    source_data: Option<Vec<u8>>,
    image_tag: String,
    new_version: String,
    prev_version: String,
    prev_image: Option<String>,
    changelog: Option<String>,
) {
    let db = &state.db;
    let upload_id = build_id.to_string();

    set_build_status(db, build_id, BuildStatus::Building).await;
    set_upload_status(db, &upload_id, &name, owner_id, "initiated", None, None).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-update-{build_id}"));

    let result: Result<(), String> = async {
        // Resolve the source directory — either from a zip or a GitHub re-deploy.
        let agent_source_dir = if let Some(zip_data) = source_data {
            extract_zip_to_dir(&zip_data, &tmp_dir)?;
            tmp_dir.clone()
        } else {
            // GitHub re-deploy: download the tarball and find the inner top-level dir.
            let (full_repo, token) =
                resolve_agent_github_source(db, agent_id, owner_id)
                    .await
                    .ok_or_else(|| {
                        "no source provided and no GitHub source on record".to_string()
                    })?;

            let tarball = download_repo_tarball(&state.http_client, &token, &full_repo)
                .await?;

            build::extract_tar_gzip(&tarball, &tmp_dir)?;

            // GitHub archives unpack into a single top-level dir (e.g. owner-repo-<sha>/).
            // Find it so we can look for the Dockerfile there.
            let inner = tokio::fs::read_dir(&tmp_dir)
                .await
                .map_err(|e| format!("read extracted dir: {e}"))?
                .next_entry()
                .await
                .map_err(|e| format!("read inner dir entry: {e}"))?
                .map(|e| e.path());

            match inner {
                Some(p) if p.is_dir() => p,
                _ => tmp_dir.clone(),
            }
        };

        set_upload_status(db, &upload_id, &name, owner_id, "processing", None, None).await;

        let dockerfile_path = agent_source_dir.join("Dockerfile");
        if !dockerfile_path.exists() {
            return Err("no Dockerfile found in source".into());
        }

        // Patch Dockerfile for OTel.
        let original = tokio::fs::read_to_string(&dockerfile_path)
            .await
            .map_err(|e| format!("read Dockerfile: {e}"))?;
        let patched = nasiko_observability::patch_dockerfile_for_otel(&original);
        if patched != original {
            tokio::fs::write(&dockerfile_path, &patched)
                .await
                .map_err(|e| format!("write Dockerfile: {e}"))?;
        }

        let tar_bytes = crate::build::tar_directory(&agent_source_dir)
            .map_err(|e| format!("tar source: {e}"))?;
        state.runtime.build(&tar_bytes, &image_tag).await.map_err(|e| format!("build: {e}"))?;

        set_upload_status(db, &upload_id, &name, owner_id, "orchestration_triggered", None, None).await;

        let secrets = agent_secrets::resolve_agent_env(db, agent_id).await;
        let spec = DeploymentSpec {
            container_id: ContainerId::new(&name),
            name: name.clone(),
            image: image_tag.clone(),
            ports: vec![8000],
            env_vars: secrets,
            min_replicas: 1,
            max_replicas: 1,
            resources: None,
        };
        state.runtime.deploy(&spec).await.map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(db, &upload_id, &name, owner_id, "orchestration_processing", None, None).await;
        Ok(())
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(()) => {
            // Archive the previously active version, insert the new one, mark old as rollback-eligible.
            let _ = sqlx::query(
                "UPDATE agent_versions SET is_active = false, status = 'archived' \
                 WHERE agent_id = $1 AND is_active = true",
            )
            .bind(agent_id)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "INSERT INTO agent_versions \
                   (agent_id, build_id, version, image_tag, changelog, is_active, can_rollback, previous_version, status) \
                 VALUES ($1, $2, $3, $4, $5, true, false, $6, 'active') \
                 ON CONFLICT (agent_id, version) DO UPDATE \
                   SET build_id = EXCLUDED.build_id, is_active = true, status = 'active', \
                       can_rollback = false, previous_version = EXCLUDED.previous_version",
            )
            .bind(agent_id)
            .bind(build_id)
            .bind(&new_version)
            .bind(&image_tag)
            .bind(&changelog)
            .bind(&prev_version)
            .execute(db)
            .await;

            // Allow rolling back to the previous version.
            let _ = sqlx::query(
                "UPDATE agent_versions SET can_rollback = true \
                 WHERE agent_id = $1 AND version = $2",
            )
            .bind(agent_id)
            .bind(&prev_version)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
                 VALUES ($1, $2, 'running', $3)",
            )
            .bind(agent_id)
            .bind(build_id)
            .bind(owner_id)
            .execute(db)
            .await;

            set_build_status(db, build_id, BuildStatus::Success).await;
            set_upload_status(db, &upload_id, &name, owner_id, "completed", Some(agent_id), None).await;
            tracing::info!(build_id = %build_id, %agent_id, %new_version, "agent update succeeded");
        }
        Err(e) => {
            // Roll back both version and image so the row stays consistent with
            // what is actually running. prev_image is None only for the very first
            // deploy, in which case NULL is the correct fallback anyway.
            let _ = sqlx::query(
                "UPDATE agents SET status = 'failed', version = $2, image = $3, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&prev_version)
            .bind(&prev_image)
            .execute(db)
            .await;

            set_build_status(db, build_id, BuildStatus::Failed).await;
            set_upload_status(db, &upload_id, &name, owner_id, "failed", None, Some(&e)).await;
            tracing::error!(build_id = %build_id, %agent_id, %e, "agent update failed");
        }
    }
}

// ─── POST /{id}/rollback ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RollbackRequest {
    target_version: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct RollbackResponse {
    build_id: Uuid,
    agent_id: Uuid,
    rolled_back_to: String,
    rolled_back_from: String,
    status: &'static str,
}

#[derive(Debug, sqlx::FromRow)]
struct AgentVersionRow {
    version: String,
    image_tag: String,
    can_rollback: bool,
}

async fn rollback_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    // Use raw bytes so malformed JSON returns 422 instead of silently defaulting
    // to "no body" (which Option<Json<T>> does in Axum 0.8).
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let caller_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    // Parse optional JSON body — empty body is valid (use defaults).
    let req: Option<RollbackRequest> = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice(&body) {
            Ok(r) => Some(r),
            Err(_) => return (StatusCode::UNPROCESSABLE_ENTITY, "invalid JSON body").into_response(),
        }
    };

    // Fetch agent — verify exists.
    let agent: Option<(String, String, Uuid)> = match sqlx::query_as(
        "SELECT name, version, owner_id FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, %agent_id, "rollback_agent: db error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let (agent_name, current_version, _agent_owner_id) = match agent {
        Some(r) => r,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Superusers bypass ACL; everyone else needs owner or explicit ACL grant.
    if !claims.is_superuser && !user_can_access_agent(&state.db, caller_id, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let reason = req.as_ref().and_then(|b| b.reason.clone());
    let target_version_req = req.and_then(|b| b.target_version);

    // Resolve the target version.
    let target: AgentVersionRow = match &target_version_req {
        Some(v) => {
            let row: Option<AgentVersionRow> = match sqlx::query_as(
                "SELECT version, image_tag, can_rollback \
                 FROM agent_versions WHERE agent_id = $1 AND version = $2",
            )
            .bind(agent_id)
            .bind(v)
            .fetch_optional(&state.db)
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(%e, %agent_id, "rollback_agent: fetch target version");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };

            match row {
                None => {
                    return (StatusCode::NOT_FOUND, format!("version {v} not found")).into_response()
                }
                Some(r) if !r.can_rollback => {
                    return (
                        StatusCode::BAD_REQUEST,
                        format!("version {v} is not rollback-eligible"),
                    )
                        .into_response()
                }
                Some(r) => r,
            }
        }
        None => {
            // Default: most recent can_rollback=true, is_active=false version.
            match sqlx::query_as::<_, AgentVersionRow>(
                "SELECT version, image_tag, can_rollback \
                 FROM agent_versions \
                 WHERE agent_id = $1 AND can_rollback = true AND is_active = false \
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(r)) => r,
                Ok(None) => {
                    return (
                        StatusCode::CONFLICT,
                        "no eligible rollback version — agent has only one version",
                    )
                        .into_response()
                }
                Err(e) => {
                    tracing::error!(%e, %agent_id, "rollback_agent: find rollback version");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    };

    // Create a synthetic build record (no Docker build) so SSE polling works.
    let rollback_build_id = match sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference, triggered_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(agent_id)
    .bind(&target.version)
    .bind(&target.image_tag)
    .bind(caller_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("create rollback record: {e}"),
            )
                .into_response()
        }
    };

    let rolled_back_to = target.version.clone();
    let state_clone = state.clone();
    let name_clone = agent_name.clone();

    tokio::spawn(async move {
        execute_agent_rollback(
            state_clone,
            rollback_build_id,
            agent_id,
            caller_id,
            name_clone,
            target,
            reason,
        )
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(RollbackResponse {
            build_id: rollback_build_id,
            agent_id,
            rolled_back_to,
            rolled_back_from: current_version,
            status: "queued",
        }),
    )
        .into_response()
}

async fn execute_agent_rollback(
    state: AppState,
    rollback_build_id: Uuid,
    agent_id: Uuid,
    caller_id: Uuid,
    agent_name: String,
    target: AgentVersionRow,
    reason: Option<String>,
) {
    let db = &state.db;

    if let Some(r) = &reason {
        tracing::info!(build_id = %rollback_build_id, %agent_id, reason = %r, "agent rollback initiated");
    }

    set_build_status(db, rollback_build_id, BuildStatus::Building).await;

    let _ = sqlx::query("UPDATE agents SET status = 'deploying', updated_at = now() WHERE id = $1")
        .bind(agent_id)
        .execute(db)
        .await;

    let secrets = agent_secrets::resolve_agent_env(db, agent_id).await;
    let spec = DeploymentSpec {
        container_id: ContainerId::new(&agent_name),
        name: agent_name.clone(),
        image: target.image_tag.clone(),
        ports: vec![8000],
        env_vars: secrets,
        min_replicas: 1,
        max_replicas: 1,
        resources: None,
    };

    match state.runtime.deploy(&spec).await {
        Ok(_) => {
            // Deactivate current version, activate rollback target.
            let _ = sqlx::query(
                "UPDATE agent_versions SET is_active = false, status = 'archived' \
                 WHERE agent_id = $1 AND is_active = true",
            )
            .bind(agent_id)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "UPDATE agent_versions SET is_active = true, status = 'active' \
                 WHERE agent_id = $1 AND version = $2",
            )
            .bind(agent_id)
            .bind(&target.version)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', version = $2, image = $3, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&target.version)
            .bind(&target.image_tag)
            .execute(db)
            .await;

            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
                 VALUES ($1, $2, 'running', $3)",
            )
            .bind(agent_id)
            .bind(rollback_build_id)
            .bind(caller_id)
            .execute(db)
            .await;

            set_build_status(db, rollback_build_id, BuildStatus::Success).await;
            tracing::info!(build_id = %rollback_build_id, %agent_id, version = %target.version, "agent rollback succeeded");
        }
        Err(e) => {
            let _ = sqlx::query(
                "UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .execute(db)
            .await;

            set_build_status(db, rollback_build_id, BuildStatus::Failed).await;
            tracing::error!(build_id = %rollback_build_id, %agent_id, %e, "agent rollback failed");
        }
    }
}
