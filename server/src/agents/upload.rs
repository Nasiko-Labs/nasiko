use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use nasiko_runtime::DeploymentStatus;

use crate::auth::Claims;
use crate::build::{self, BuildStatus};
use crate::build::routes::{extract_zip_from_file};
use crate::state::AppState;

use super::utils::{set_build_status, set_upload_status};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload", post(upload_and_deploy))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES as usize))
}

/// Mounted separately from `router()`, under `require_auth` only — each
/// handler checks `can_deploy` itself (matching who could reach these
/// before) and returns `crate::unavailable()` (200) instead of a
/// blanket 403.
pub fn degradable_router() -> Router<AppState> {
    Router::new()
        .route("/uploads",                   get(list_upload_status))
        .route("/uploads/{upload_id}",       get(get_upload_status))
        .route("/deploys/{build_id}/stream", get(deploy_status_sse))
}

pub fn user_routes() -> Router<AppState> {
    Router::new()
        .route("/my-uploads", get(list_upload_agents))
}

/// Routes mounted at the top-level `/api` — not nested under `/agents`.
pub fn status_router() -> Router<AppState> {
    Router::new()
        .route("/upload-status", get(list_upload_status))
}

#[derive(Debug, Serialize)]
pub struct UploadQueuedData {
    pub success: bool,
    pub agent_name: String,
    pub agent_id: String,
    pub build_id: String,
    pub status: &'static str,
    pub capabilities_generated: bool,
    pub orchestration_triggered: bool,
    pub validation_errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UploadAndDeployResponse {
    pub data: UploadQueuedData,
    pub status_code: u16,
    pub message: String,
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
    agent_url: Option<String>,
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
    /// MCP-server-upload build+deploy (POST /api/mcp/connectors/upload or
    /// /upload-github). `env` is encrypted (owner-scoped) at rest in this
    /// JSONB payload — see `build_worker::decrypt_build_secrets`, which
    /// decrypts it immediately before use and never persists the plaintext.
    McpServerUpload {
        build_id: Uuid,
        connector_id: Uuid,
        owner_id: Uuid,
        name: String,
        source: McpBuildSourcePayload,
        image_tag: String,
        env: HashMap<String, String>,
    },
}

/// Source payload for `BuildJobPayload::McpServerUpload` — mirrors
/// `crate::mcp::build::BuildSource`, but serializable (that type is
/// constructed fresh per build and never itself persisted).
#[derive(Debug, Serialize, Deserialize)]
pub enum McpBuildSourcePayload {
    Zip { zip_path: String },
    Github { url: String },
}

impl BuildJobPayload {
    pub fn build_id(&self) -> Uuid {
        match self {
            Self::Upload { build_id, .. }
            | Self::Update { build_id, .. }
            | Self::StandaloneBuild { build_id, .. }
            | Self::McpServerUpload { build_id, .. } => *build_id,
            Self::Rollback { rollback_build_id, .. } => *rollback_build_id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Upload { name, .. } | Self::Update { name, .. } | Self::McpServerUpload { name, .. } => name,
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
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let mut name: Option<String> = None;
    let mut version_tag: Option<String> = None;
    let mut zip_path: Option<PathBuf> = None;
    let mut ports: Vec<u16> = vec![];
    let mut env: HashMap<String, String> = HashMap::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" | "agent_name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "source" | "file" => {
                use crate::multipart_util::{StreamUploadError, stream_field_to_fresh_temp_file};
                match stream_field_to_fresh_temp_file("nasiko-upload", "upload.zip", field, MAX_UPLOAD_BYTES).await {
                    Ok(path) => zip_path = Some(path),
                    Err(StreamUploadError::TooLarge) => {
                        tracing::warn!(limit = MAX_UPLOAD_BYTES, "upload rejected: size limit exceeded");
                        return (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds 100 MiB").into_response();
                    }
                    Err(StreamUploadError::ReadFailed(e)) => {
                        tracing::warn!(%e, "upload: read multipart chunk failed");
                        return (StatusCode::BAD_REQUEST, "failed to read upload stream").into_response();
                    }
                    Err(StreamUploadError::Io(e)) => {
                        tracing::error!(%e, "upload: stream to disk failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
                    }
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

    // Validate name + version_tag charset (RUN-10): both flow into the OCI image
    // reference `{name}:{tag}`; unvalidated values allow push-target redirection
    // (e.g. `/` or `@` smuggling a different registry/digest) when no registry
    // prefix is configured.
    if let Err(e) = crate::build::routes::validate_version_tag(&name) {
        return (StatusCode::BAD_REQUEST, format!("invalid name: {e}")).into_response();
    }
    if let Err(e) = crate::build::routes::validate_version_tag(&version_tag) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
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
        tracing::error!(%e, "upload: create validation dir failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
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
            tracing::error!(%e, "upload: validation task join error");
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    }

    let image_tag = crate::agents::build_image_tag(&state.config.agent_image_registry, &name, &version_tag);
    // Empty → canonical default is applied by build_agent_spec (8000, matching the
    // agent images' EXPOSE); never default to 5000 here.

    // ── Upsert agent + build record + job (one transaction — SRV-5) ───────────
    // These 3 writes must succeed or fail together: if the build_jobs insert
    // failed after the agent/build rows committed separately, the agent was
    // left stuck in "deploying" with no job that would ever move it out of
    // that state. A single transaction, committed only after the job row is
    // queued, closes that gap.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(%e, agent_name = %name, "upload: begin transaction failed");
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    let ports = if ports.is_empty() { vec![8000] } else { ports };

    // Atomic INSERT ... ON CONFLICT against the (owner_id, name) partial unique
    // index (migration 015) — closes the SELECT-then-INSERT TOCTOU that let two
    // concurrent same-name uploads create duplicate rows (SRV-2).
    let agent_id = {
        match sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agents (name, owner_id, version, image, status) \
             VALUES ($1, $2, $3, $4, 'deploying') \
             ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL \
             DO UPDATE SET version = EXCLUDED.version, image = EXCLUDED.image, \
                           status = 'deploying', updated_at = now() \
             RETURNING id",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(&version_tag)
        .bind(&image_tag)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(%e, agent_name = %name, "upload: register agent db error");
                let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
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
    .fetch_one(&mut *tx)
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(%e, %agent_id, "upload: create build record db error");
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    let upload_id = build_id.to_string();

    // NOTE: server-level secrets (OPENAI_API_KEY / OPENAI_BASE_URL) are deliberately
    // NOT injected here — that would serialize the server API key in cleartext into
    // build_jobs.payload (RUN-5). The worker injects them from live config at
    // execution time (see execute_upload_and_deploy), mirroring update/rollback.

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
            tracing::error!(%e, %agent_id, "upload: serialize build payload failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    if let Err(e) = sqlx::query(
        "INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(&payload_value)
    .execute(&mut *tx)
    .await
    {
        tracing::error!(%e, %agent_id, "upload: queue build_jobs db error");
        let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(%e, %agent_id, "upload: commit transaction failed");
        let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }

    // Notify the build worker immediately so it doesn't wait for the 5s poll interval.
    let _ = state.build_tx.send(()).await;

    // Seed the upload_status row now (with the real agent_id) so that
    // GET /api/agents/my-uploads returns the correct agent_id immediately.
    // The build worker's first set_upload_status call will hit the ON CONFLICT
    // path and preserve agent_id via COALESCE.
    set_upload_status(&state.db, &upload_id, &name, owner_id, "initiated", Some(agent_id), None).await;

    tracing::info!(
        %build_id,
        %agent_id,
        %name,
        "upload-and-deploy queued"
    );

    (
        StatusCode::ACCEPTED,
        Json(UploadAndDeployResponse {
            data: UploadQueuedData {
                success: true,
                agent_name: name,
                agent_id: agent_id.to_string(),
                build_id: build_id.to_string(),
                status: "queued",
                capabilities_generated: false,
                orchestration_triggered: false,
                validation_errors: vec![],
            },
            status_code: 202,
            message: "Agent upload queued successfully".to_string(),
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
    http: reqwest::Client,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    upload_id: String,
    name: String,
    zip_path: PathBuf,
    image_tag: String,
    ports: Vec<u16>,
    mut env: HashMap<String, String>,
    // Server-level LLM defaults, injected here (not baked into the DB payload) so
    // the API key is never persisted in cleartext (RUN-5). Caller env wins.
    openai_api_key: Option<String>,
    openai_base_url: Option<String>,
    agent_runtime: String,
    agent_image_registry: String,
) {
    if let Some(key) = openai_api_key {
        env.entry("OPENAI_API_KEY".to_owned()).or_insert(key);
    }
    if let Some(url) = openai_base_url {
        env.entry("OPENAI_BASE_URL".to_owned()).or_insert(url);
    }
    set_build_status(&db, build_id, BuildStatus::Building).await;
    set_upload_status(&db, &upload_id, &name, owner_id, "initiated", None, None).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-agent-{build_id}"));

    let result: Result<DeploymentStatus, String> = async {
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

            // Write the Python sitecustomize file into the build context so the
            // Dockerfile COPY step can include it in the agent image.
            nasiko_observability::write_otel_patch_file(&tmp_dir)
                .map_err(|e| format!("write OTel patch file to build context: {e}"))?;
            tracing::info!(build_id = %build_id, "wrote .nasiko_otel_patch.py to build context");
        }

        // Build Docker image.
        let tar_bytes = build::tar_directory(&tmp_dir)
            .map_err(|e| format!("tar source: {e}"))?;
        runtime
            .build(&tar_bytes, &image_tag)
            .await
            .map_err(|e| format!("docker build: {e}"))?;

        set_upload_status(&db, &upload_id, &name, owner_id, "orchestration_triggered", None, None).await;

        // Deploy container keyed on agent UUID (not name) — see build_agent_spec.
        let mut spec = crate::agents::build_agent_spec(agent_id, &name, image_tag.clone(), ports, env, None);
        crate::agents::attach_pull_credential(&db, &agent_runtime, &agent_image_registry, &mut spec, agent_id).await;
        let deploy_status = runtime
            .deploy(&spec)
            .await
            .map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(&db, &upload_id, &name, owner_id, "orchestration_processing", None, None).await;

        Ok(deploy_status)
    }
    .await;

    // Clean up both the extracted dir and the original zip directory.
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    if let Some(zip_dir) = zip_path.parent() {
        let _ = tokio::fs::remove_dir_all(zip_dir).await;
    }

    match result {
        Ok(deploy_status) => {
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
            let agent_url = crate::agents::resolve_agent_url(
                &runtime,
                &deploy_status,
                &nasiko_runtime::ContainerId::from_uuid(agent_id),
            )
            .await;
            let _ = sqlx::query("UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .bind(&agent_url)
                .execute(&db)
                .await;
            // Fetch the agent's card in the background to persist skills,
            // description and the advertised transport_path (chat URL).
            tokio::spawn(super::utils::fetch_agent_card_with_retry(
                db.clone(),
                http,
                agent_id,
                agent_url.clone(),
            ));
            // Record the deployment. k8s_deployment_name stores the ContainerId value
            // (agent UUID string) so that restart and crash guardian can reconstruct
            // the same ContainerId. The runtime derives the actual K8s/Docker name via
            // object_name() — do not pre-compute the prefix here.
            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status, k8s_deployment_name) \
                 VALUES ($1, $2, $3, 'running', $4)",
            )
            .bind(agent_id)
            .bind(build_id)
            .bind(owner_id)
            .bind(agent_id.to_string())
            .execute(&db)
            .await;
            tracing::info!(build_id = %build_id, agent_id = %agent_id, "upload-and-deploy succeeded");
        }
        Err(e) => {
            set_build_status(&db, build_id, BuildStatus::Failed).await;
            // `e` may embed raw docker/tar/IO error text — log it, but
            // upload_status.error_message is read back by clients via
            // GET /agents/uploads, so store a generic reason there.
            set_upload_status(&db, &upload_id, &name, owner_id, "failed", None, Some("upload and deploy failed")).await;
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
    claims: Claims,
    Path(build_id): Path<Uuid>,
) -> axum::response::Response {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
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

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

// ─── Upload status helpers ────────────────────────────────────────────────────

fn status_progress(status: &str) -> i32 {
    match status {
        "initiated" => 10,
        "processing" => 30,
        "capabilities_generated" => 50,
        "orchestration_triggered" => 60,
        "orchestration_processing" => 80,
        "completed" => 100,
        _ => 0,
    }
}

fn status_message_str(status: &str, agent_name: &str) -> String {
    match status {
        "initiated" => format!("Upload initiated for '{agent_name}'"),
        "processing" => "Processing agent source...".to_string(),
        "capabilities_generated" => "Agent capabilities generated".to_string(),
        "orchestration_triggered" => "Deployment triggered".to_string(),
        "orchestration_processing" => "Agent is being deployed...".to_string(),
        "completed" => format!("Agent '{agent_name}' deployed successfully"),
        "failed" => "Deployment failed".to_string(),
        _ => status.to_string(),
    }
}

#[derive(Debug, Serialize)]
struct SourceInfoJson {
    filename: String,
    content_type: String,
}

#[derive(Debug, Serialize)]
struct UploadStatusItem {
    upload_id: String,
    agent_name: String,
    status: String,
    progress_percentage: i32,
    source_info: SourceInfoJson,
    file_size: i64,
    capabilities_generated: bool,
    orchestration_triggered: bool,
    registry_updated: bool,
    agent_url: Option<String>,
    registry_id: Option<String>,
    status_message: String,
    error_details: Vec<String>,
    validation_errors: Vec<serde_json::Value>,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
    processing_duration: f64,
    orchestration_duration: Option<f64>,
}

fn row_to_status_item(row: UploadStatusRow) -> UploadStatusItem {
    let progress = status_progress(&row.status);
    let is_done = matches!(row.status.as_str(), "completed" | "failed");
    let cap_gen = matches!(
        row.status.as_str(),
        "capabilities_generated" | "orchestration_triggered" | "orchestration_processing" | "completed"
    );
    let orch_trig = matches!(
        row.status.as_str(),
        "orchestration_triggered" | "orchestration_processing" | "completed"
    );
    let processing_duration = (row.updated_at - row.created_at)
        .num_milliseconds()
        .max(0) as f64
        / 1000.0;
    let status_msg = status_message_str(&row.status, &row.agent_name);
    let error_details: Vec<String> = row.error_message.into_iter().collect();
    let filename = format!("{}.zip", row.agent_name);
    UploadStatusItem {
        upload_id: row.upload_id,
        agent_name: row.agent_name.clone(),
        status: row.status,
        progress_percentage: progress,
        source_info: SourceInfoJson {
            filename,
            content_type: "application/zip".to_string(),
        },
        file_size: 0,
        capabilities_generated: cap_gen,
        orchestration_triggered: orch_trig,
        registry_updated: is_done && progress == 100,
        agent_url: row.agent_url,
        registry_id: None,
        status_message: status_msg,
        error_details,
        validation_errors: vec![],
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        completed_at: is_done.then(|| row.updated_at.to_rfc3339()),
        processing_duration,
        orchestration_duration: None,
    }
}

// ─── GET /upload-status/{upload_id} ─────────────────────────────────────────

async fn get_upload_status(
    State(state): State<AppState>,
    claims: Claims,
    Path(upload_id): Path<String>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let result = if claims.is_superuser {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT us.id, us.upload_id, us.agent_id, us.agent_name, us.status::text as status, us.owner_id, us.error_message, a.url as agent_url, us.created_at, us.updated_at
             FROM upload_status us
             LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
             WHERE us.upload_id = $1",
        )
        .bind(&upload_id)
        .fetch_optional(&state.db)
        .await
    } else {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT us.id, us.upload_id, us.agent_id, us.agent_name, us.status::text as status, us.owner_id, us.error_message, a.url as agent_url, us.created_at, us.updated_at
             FROM upload_status us
             LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
             WHERE us.upload_id = $1 AND us.owner_id = $2",
        )
        .bind(&upload_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
    };

    match result {
        Ok(Some(row)) => Json(row_to_status_item(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, upload_id, "get_upload_status db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── GET /agents/uploads ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct UploadListQuery {
    #[serde(default = "default_upload_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_upload_limit() -> i64 {
    10
}

async fn list_upload_status(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<UploadListQuery>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let limit = q.limit.clamp(1, 100);
    let offset = q.offset.max(0);

    let rows = if claims.is_superuser {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT us.id, us.upload_id, us.agent_id, us.agent_name, us.status::text as status, us.owner_id, us.error_message, a.url as agent_url, us.created_at, us.updated_at
             FROM upload_status us
             LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
             ORDER BY us.created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, UploadStatusRow>(
            "SELECT us.id, us.upload_id, us.agent_id, us.agent_name, us.status::text as status, us.owner_id, us.error_message, a.url as agent_url, us.created_at, us.updated_at
             FROM upload_status us
             LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
             WHERE us.owner_id = $1 ORDER BY us.created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(data) => {
            let items: Vec<UploadStatusItem> = data.into_iter().map(row_to_status_item).collect();
            let count = items.len();
            Json(serde_json::json!({
                "data": items,
                "status_code": 200,
                "message": format!("Retrieved {count} upload records"),
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_upload_status db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── GET /agents/upload-agents ───────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
struct UploadAgentRow {
    agent_id: Option<Uuid>,
    agent_name: String,
    upload_id: String,
    error_message: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    icon_url: Option<String>,
    version: Option<String>,
    agent_status: Option<String>,
}

#[derive(Serialize)]
struct UploadInfoResponse {
    upload_type: &'static str,
    upload_status: String,
    status_message: Option<String>,
    error_detail: Option<String>,
}

#[derive(Serialize)]
struct UploadAgentResponse {
    agent_id: String,
    agent_name: String,
    icon_url: Option<String>,
    upload_info: UploadInfoResponse,
    tags: Vec<String>,
    description: Option<String>,
}

#[derive(Serialize)]
struct UploadAgentsListResponse {
    data: Vec<UploadAgentResponse>,
    status_code: u16,
    message: String,
}

fn agent_display_status(agent_status: Option<&str>) -> &'static str {
    match agent_status {
        Some("running") => "Active",
        Some("deploying") | Some("pending") | Some("building") => "Deploying",
        Some("failed") => "Failed",
        Some("stopped") | Some("registered") => "Stopped",
        _ => "Unknown",
    }
}

fn agent_status_message(display_status: &str, version: Option<&str>) -> Option<String> {
    match display_status {
        "Active" => Some(format!(
            "Agent v{} deployed successfully",
            version.unwrap_or("1.0.0")
        )),
        "Deploying" => Some("Agent is being deployed...".to_string()),
        "Failed" => Some("Deployment failed".to_string()),
        "Stopped" => Some("Agent is stopped".to_string()),
        _ => None,
    }
}

async fn list_upload_agents(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Join with agents to pull live metadata (tags, description, icon_url, version, status).
    // DISTINCT ON keeps the most recent upload row per agent.
    let rows: Result<Vec<UploadAgentRow>, _> = if claims.is_superuser {
        sqlx::query_as(
            r#"SELECT DISTINCT ON (COALESCE(us.agent_id::text, us.upload_id))
                   us.agent_id,
                   us.agent_name,
                   us.upload_id,
                   us.error_message,
                   a.description,
                   COALESCE(a.tags, '{}') AS tags,
                   a.icon_url,
                   a.version,
                   a.status AS agent_status
               FROM upload_status us
               LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
               ORDER BY COALESCE(us.agent_id::text, us.upload_id), us.created_at DESC
               LIMIT 50"#,
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            r#"SELECT DISTINCT ON (COALESCE(us.agent_id::text, us.upload_id))
                   us.agent_id,
                   us.agent_name,
                   us.upload_id,
                   us.error_message,
                   a.description,
                   COALESCE(a.tags, '{}') AS tags,
                   a.icon_url,
                   a.version,
                   a.status AS agent_status
               FROM upload_status us
               LEFT JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
               WHERE us.owner_id = $1
               ORDER BY COALESCE(us.agent_id::text, us.upload_id), us.created_at DESC
               LIMIT 50"#,
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => {
            let count = rows.len();
            let data = rows
                .into_iter()
                .map(|r| {
                    let display_status =
                        agent_display_status(r.agent_status.as_deref());
                    let status_message =
                        agent_status_message(display_status, r.version.as_deref());
                    UploadAgentResponse {
                        agent_id: r
                            .agent_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| r.upload_id.clone()),
                        agent_name: r.agent_name,
                        icon_url: r.icon_url,
                        upload_info: UploadInfoResponse {
                            upload_type: "zip",
                            upload_status: display_status.to_string(),
                            status_message,
                            error_detail: r.error_message,
                        },
                        tags: r.tags,
                        description: r.description,
                    }
                })
                .collect();
            Json(UploadAgentsListResponse {
                data,
                status_code: 200,
                message: format!("Retrieved {count} upload agents for user"),
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_upload_agents db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
