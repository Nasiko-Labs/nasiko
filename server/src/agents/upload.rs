use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::auth::Claims;
use crate::build::{self, BuildStatus};
use crate::build::routes::{extract_zip_from_file};
use crate::state::AppState;

use super::utils::{set_build_status, set_upload_status};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload-and-deploy",        post(upload_and_deploy))
        .route("/deploy-status/{build_id}", get(deploy_status_sse))
        .route("/upload-status/{upload_id}", get(get_upload_status))
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

/// Payload stored in build_jobs.payload JSONB. The `kind` tag selects the dispatch path in
/// `build_worker`. All variants must include everything the worker needs — no DB re-reads.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BuildJobPayload {
    /// Initial upload-and-deploy (POST /api/agents/upload-and-deploy).
    Upload {
        build_id: Uuid,
        agent_id: Uuid,
        owner_id: Uuid,
        upload_id: String,
        name: String,
        /// Absolute path to the zip file on disk (streamed there by the upload handler).
        zip_path: String,
        image_tag: String,
        ports: Vec<u16>,
        env: HashMap<String, String>,
    },
    /// In-place agent update (PUT /api/agents/{id}/update).
    Update {
        build_id: Uuid,
        agent_id: Uuid,
        owner_id: Uuid,
        name: String,
        /// Path to uploaded zip on disk, or `None` for a GitHub re-deploy.
        zip_path: Option<String>,
        image_tag: String,
        new_version: String,
        prev_version: String,
        prev_image: Option<String>,
        changelog: Option<String>,
    },
    /// Rollback to a prior version (POST /api/agents/{id}/rollback).
    Rollback {
        rollback_build_id: Uuid,
        agent_id: Uuid,
        caller_id: Uuid,
        agent_name: String,
        target_version: String,
        target_image_tag: String,
        reason: Option<String>,
    },
    /// Standalone image build without deploy (POST /api/build/builds).
    StandaloneBuild {
        build_id: Uuid,
        agent_id: Uuid,
        agent_name: String,
        github_url: Option<String>,
        source_key: Option<String>,
        version_tag: String,
    },
}

impl BuildJobPayload {
    pub fn build_id(&self) -> Uuid {
        match self {
            Self::Upload { build_id, .. }
            | Self::Update { build_id, .. }
            | Self::StandaloneBuild { build_id, .. } => *build_id,
            Self::Rollback { rollback_build_id, .. } => *rollback_build_id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Upload { name, .. } | Self::Update { name, .. } => name,
            Self::Rollback { agent_name, .. } | Self::StandaloneBuild { agent_name, .. } => agent_name,
        }
    }
}

const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB

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
    let mut zip_path: Option<PathBuf> = None;
    let mut ports: Vec<u16> = vec![];
    let mut env: HashMap<String, String> = HashMap::new();

    // Build a temporary directory early so we have a path to stream into.
    // The worker cleans this up after the job completes.
    let tmp_base = std::env::temp_dir();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "source" => {
                // Stream zip to disk rather than buffering it all in RAM.
                let field_name = name.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let upload_dir = tmp_base.join(format!("nasiko-upload-{field_name}"));
                if let Err(e) = tokio::fs::create_dir_all(&upload_dir).await {
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("create upload dir: {e}")).into_response();
                }
                let path = upload_dir.join("upload.zip");

                let mut f = match tokio::fs::File::create(&path).await {
                    Ok(f) => f,
                    Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("create zip file: {e}")).into_response(),
                };

                let mut total_bytes: u64 = 0;
                let mut chunk_stream = field;
                loop {
                    match chunk_stream.chunk().await {
                        Ok(Some(chunk)) => {
                            total_bytes += chunk.len() as u64;
                            if total_bytes > MAX_UPLOAD_BYTES {
                                tracing::warn!(total_bytes, limit = MAX_UPLOAD_BYTES, "upload rejected: size limit exceeded");
                                let _ = tokio::fs::remove_dir_all(&upload_dir).await;
                                return (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds 100 MiB").into_response();
                            }
                            use tokio::io::AsyncWriteExt;
                            if let Err(e) = f.write_all(&chunk).await {
                                return (StatusCode::INTERNAL_SERVER_ERROR, format!("write chunk: {e}")).into_response();
                            }
                        }
                        Ok(None) => break,
                        Err(e) => return (StatusCode::BAD_REQUEST, format!("read upload: {e}")).into_response(),
                    }
                }
                zip_path = Some(path);
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
    let zip_path = match zip_path {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "source zip is required").into_response(),
    };

    // ── Agent structure validation ────────────────────────────────────────────
    // Extract to a temp dir, validate, then clean up (the zip stays for the worker).
    let validation_dir = zip_path.parent()
        .unwrap_or(&std::env::temp_dir().join("nasiko-val"))
        .join("validate");

    if let Err(e) = std::fs::create_dir_all(&validation_dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("create validation dir: {e}")).into_response();
    }

    let zip_path_clone = zip_path.clone();
    let validation_dir_clone = validation_dir.clone();
    let validation_result = tokio::task::spawn_blocking(move || {
        validate_agent_zip(&zip_path_clone, &validation_dir_clone)
    }).await;

    // Clean up validation dir regardless of outcome
    let _ = tokio::fs::remove_dir_all(&validation_dir).await;

    match validation_result {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => {
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("validation error: {e}")).into_response();
        }
    }

    let image_tag = if state.config.agent_image_registry.is_empty() {
        format!("{name}:{version_tag}")
    } else {
        format!("{}/{name}:{version_tag}", state.config.agent_image_registry)
    };
    let ports = if ports.is_empty() { vec![8000] } else { ports };

    // ── Upsert agent ──────────────────────────────────────────────────────────
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
                let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("register agent: {e}"))
                    .into_response();
            }
        }
    };

    // ── Persist build record ──────────────────────────────────────────────────
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
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("create build record: {e}"))
                .into_response();
        }
    };

    let upload_id = build_id.to_string();

    // ── Insert build job (worker picks this up via SKIP LOCKED) ──────────────
    let payload = BuildJobPayload::Upload {
        build_id,
        agent_id,
        owner_id,
        upload_id: upload_id.clone(),
        name: name.clone(),
        zip_path: zip_path.to_string_lossy().into_owned(),
        image_tag: image_tag.clone(),
        ports,
        env,
    };

    let payload_value = match serde_json::to_value(&payload) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize payload: {e}")).into_response();
        }
    };

    if let Err(e) = sqlx::query(
        "INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(&payload_value)
    .execute(&state.db)
    .await
    {
        let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("queue build: {e}")).into_response();
    }

    // Notify the build worker immediately so it doesn't wait for the 5s poll interval.
    let _ = state.build_tx.send(()).await;

    tracing::info!(
        %build_id,
        %agent_id,
        %name,
        "upload-and-deploy queued"
    );

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

/// Validate the agent zip structure synchronously (blocking, run in spawn_blocking).
fn validate_agent_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    extract_zip_from_file(zip_path, dest)?;

    // Dockerfile must exist in root and have at least one FROM line
    let dockerfile = dest.join("Dockerfile");
    if !dockerfile.exists() {
        tracing::warn!(zip_path = %zip_path.display(), reason = "missing Dockerfile", "upload rejected: invalid agent structure");
        return Err("no Dockerfile found in root of zip".into());
    }
    let contents = std::fs::read_to_string(&dockerfile)
        .map_err(|e| format!("read Dockerfile: {e}"))?;
    if !contents.lines().any(|l| l.trim_start().starts_with("FROM ")) {
        tracing::warn!(zip_path = %zip_path.display(), reason = "Dockerfile missing FROM", "upload rejected: invalid agent structure");
        return Err("Dockerfile has no FROM instruction".into());
    }

    // At least one Python entrypoint must exist
    let entrypoints = ["main.py", "src/main.py", "src/__main__.py", "__main__.py"];
    let has_entrypoint = entrypoints.iter().any(|p| dest.join(p).exists());
    if !has_entrypoint {
        tracing::warn!(zip_path = %zip_path.display(), reason = "missing entrypoint", "upload rejected: invalid agent structure");
        return Err("no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)".into());
    }

    Ok(())
}

/// Execute the full upload-and-deploy pipeline: extract, OTel patch, docker build, deploy.
/// Called by the build worker.
#[allow(clippy::too_many_arguments)]
pub async fn execute_upload_and_deploy(
    runtime: std::sync::Arc<dyn nasiko_runtime::ContainerRuntime>,
    db: sqlx::PgPool,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    upload_id: String,
    name: String,
    zip_path: PathBuf,
    image_tag: String,
    ports: Vec<u16>,
    env: HashMap<String, String>,
) {
    set_build_status(&db, build_id, BuildStatus::Building).await;
    set_upload_status(&db, &upload_id, &name, owner_id, "initiated", None, None).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-agent-{build_id}"));

    let result: Result<(), String> = async {
        // Extract zip (with guards — re-run here so the worker is self-contained).
        let zp = zip_path.clone();
        let td = tmp_dir.clone();
        tokio::task::spawn_blocking(move || extract_zip_from_file(&zp, &td))
            .await
            .map_err(|e| format!("spawn_blocking extract: {e}"))??;

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

    // Clean up both the extracted dir and the original zip directory.
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    if let Some(zip_dir) = zip_path.parent() {
        let _ = tokio::fs::remove_dir_all(zip_dir).await;
    }

    match result {
        Ok(()) => {
            set_build_status(&db, build_id, BuildStatus::Success).await;
            set_upload_status(&db, &upload_id, &name, owner_id, "completed", Some(agent_id), None).await;
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
            // Record the deployment so it appears in list_deployments, restart, and crash guardian.
            // k8s_deployment_name stores the raw agent name (ContainerId value); the runtime
            // derives the actual K8s name via object_name() — do not pre-compute the prefix here.
            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status, k8s_deployment_name) \
                 VALUES ($1, $2, $3, 'running', $4)",
            )
            .bind(agent_id)
            .bind(build_id)
            .bind(owner_id)
            .bind(&name)
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
