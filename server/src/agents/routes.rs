use std::collections::HashMap;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::acl::{allowed_targets, user_can_access_agent};
use crate::auth::Claims;
use crate::build::BuildStatus;
use crate::build::routes::extract_zip_to_dir;
use crate::catalog::agent_secrets;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload-and-deploy", post(upload_and_deploy))
        .route("/deploy-status/{build_id}", get(deploy_status_sse))
        .route("/upload-status/{upload_id}", get(get_upload_status))
        .route("/deployments", get(list_deployments))
        .route("/deployment/{deployment_id}/restart", post(restart_deployment))
        .route("/{id}/deployment", get(get_agent_deployment))
        .route("/{id}/acl", get(get_agent_acl))
        .route("/{id}/acl", post(add_agent_acl))
        .route("/{id}/acl/{target_id}", delete(remove_agent_acl))
}

/// Routes nested under /user in lib.rs (not under /agents).
pub fn user_routes() -> Router<AppState> {
    Router::new()
        .route("/upload-agents", get(list_upload_agents))
}

// ─── Response types ──────────────────────────────────────────────────────────

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

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DeploymentRow {
    id: Uuid,
    agent_id: Uuid,
    build_id: Uuid,
    namespace: String,
    replicas: i16,
    status: String,
    service_url: Option<String>,
    owner_id: Option<Uuid>,
    agent_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct AclResponse {
    unrestricted: bool,
    allowed: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct AddAclBody {
    target_agent_id: Uuid,
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
        let tar_bytes = crate::build::tar_directory(&tmp_dir)
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

async fn set_build_status(db: &sqlx::PgPool, build_id: Uuid, status: BuildStatus) {
    if let Err(e) =
        sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
            .bind(build_id)
            .bind(status)
            .execute(db)
            .await
    {
        tracing::error!(build_id = %build_id, ?status, %e, "failed to update build status");
    }
}

async fn set_upload_status(
    db: &sqlx::PgPool,
    upload_id: &str,
    agent_name: &str,
    owner_id: Uuid,
    status: &str,
    agent_id: Option<Uuid>,
    error: Option<&str>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status, agent_id, error_message)
         VALUES ($1, $2, $3, $4::upload_pipeline_status, $5, $6)
         ON CONFLICT (upload_id) DO UPDATE
           SET status = EXCLUDED.status,
               agent_id = COALESCE(EXCLUDED.agent_id, upload_status.agent_id),
               error_message = EXCLUDED.error_message",
    )
    .bind(upload_id)
    .bind(agent_name)
    .bind(owner_id)
    .bind(status)
    .bind(agent_id)
    .bind(error)
    .execute(db)
    .await
    {
        tracing::warn!(%e, upload_id, "failed to update upload_status");
    }
}

// ─── GET /deploy-status/{build_id} (SSE) ─────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
struct BuildStatusRow {
    status: BuildStatus,
}

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
