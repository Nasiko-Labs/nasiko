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
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use nasiko_runtime::DeploymentStatus;

use crate::auth::Claims;
use crate::build::routes::extract_zip_from_file;
use crate::build::{self, BuildStatus};
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
        .route("/uploads", get(list_upload_status))
        .route("/uploads/{upload_id}", get(get_upload_status))
        .route("/deploys/{build_id}/stream", get(deploy_status_sse))
}

pub fn user_routes() -> Router<AppState> {
    Router::new().route("/my-uploads", get(list_upload_agents))
}

/// Routes mounted at the top-level `/api` — not nested under `/agents`.
pub fn status_router() -> Router<AppState> {
    Router::new().route("/upload-status", get(list_upload_status))
}

#[derive(Debug, Serialize, ToSchema)]
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

#[derive(Debug, Serialize, ToSchema)]
pub struct UploadAndDeployResponse {
    pub data: UploadQueuedData,
    pub status_code: u16,
    pub message: String,
}

/// Multipart form for `POST /agents/upload` — a `.zip` `source`/`file` field
/// (an `AgentCard.json` + `Dockerfile` package) plus agent metadata fields.
#[derive(ToSchema)]
#[allow(dead_code)]
pub(crate) struct UploadAndDeployForm {
    #[schema(value_type = String, format = Binary)]
    source: Vec<u8>,
    name: String,
    version_tag: Option<String>,
    /// `openai` (default) | `anthropic` | `gemini` — which SDK the agent's code speaks.
    inbound_format: Option<String>,
    /// Comma-separated container ports (defaults to `8000`).
    ports: Option<String>,
    /// JSON object of extra env vars, e.g. `{"FOO":"bar"}`.
    env: Option<String>,
    /// `"true"`/`"false"` to mount / not mount a persistent, private-per-agent
    /// directory at `/workspace`. Tri-state: omit the field to keep an existing
    /// agent's stored setting (false for a new agent) — the CLI only sends it
    /// when `--writable` was passed.
    writable: Option<String>,
    /// Container-side mount target for the writable volume (`--writable-path`).
    /// Absolute path; implies `writable`. Omit to keep an existing agent's
    /// stored path (`/workspace` for a new agent).
    writable_path: Option<String>,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct UploadStatusRow {
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
        writable: bool,
        /// `--writable-path`; `None` = `/workspace`. `#[serde(default)]` so
        /// job rows queued before this field existed still deserialize.
        #[serde(default)]
        writable_path: Option<String>,
    },
    /// In-place agent update (PUT /api/agents/{id}/update).
    Update {
        build_id: Uuid,
        agent_id: Uuid,
        /// The caller who triggered this update (may differ from the agent's owner —
        /// a superuser or ACL-grantee can update another user's agent). Used only for
        /// status/audit tracking (`set_upload_status`, `build_jobs.owner_id`).
        owner_id: Uuid,
        /// The agent's actual `agents.owner_id` — always the true owner, regardless of
        /// who triggered the update. This is what must be injected into the LLM router
        /// JWT and persisted to `agent_deployments.owner_id`, so the agent keeps
        /// resolving its real owner's secrets/llm_config, not the caller's.
        agent_owner_id: Uuid,
        name: String,
        /// Path to uploaded zip on disk, or `None` for a GitHub re-deploy.
        zip_path: Option<String>,
        image_tag: String,
        new_version: String,
        prev_version: String,
        prev_image: Option<String>,
        changelog: Option<String>,
        /// Carried forward from `agents.writable` — this endpoint has no flag of
        /// its own to set it (see `update_agent`'s fetch of the agent row).
        writable: bool,
        /// Carried forward from `agents.writable_path`, same reasoning.
        #[serde(default)]
        writable_path: Option<String>,
    },
    /// Rollback to a prior version (POST /api/agents/{id}/rollback).
    Rollback {
        rollback_build_id: Uuid,
        agent_id: Uuid,
        /// Whoever triggered the rollback (may differ from the agent's owner — a
        /// superuser or ACL-grantee can roll back another user's agent). Used only
        /// for status/audit tracking (`agent_builds.triggered_by`).
        caller_id: Uuid,
        /// The agent's actual `agents.owner_id` — see the identical field on
        /// `Update` for why this must be kept separate from `caller_id`.
        agent_owner_id: Uuid,
        agent_name: String,
        target_version: String,
        target_image_tag: String,
        reason: Option<String>,
        /// Carried forward from `agents.writable` — same reasoning as
        /// `Update::writable`.
        writable: bool,
        /// Carried forward from `agents.writable_path`, same reasoning.
        #[serde(default)]
        writable_path: Option<String>,
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
    /// GitHub clone-and-deploy (POST /api/github/clone).
    Clone {
        build_id: Uuid,
        agent_id: Uuid,
        owner_id: Uuid,
        upload_id: String,
        name: String,
        /// Absolute path to the `.tar.gz` archive on disk.
        tar_gz_path: String,
        image_tag: String,
        ports: Vec<u16>,
        env: HashMap<String, String>,
    },
    /// GitHub clone-and-deploy via build worker (POST /api/github/clone).
    ///
    /// Unlike `Clone`, the git clone runs inside the build worker rather than the
    /// HTTP handler — the handler returns 202 immediately and the worker retries
    /// on transient failures.
    GithubClone {
        build_id: Uuid,
        agent_id: Uuid,
        owner_id: Uuid,
        upload_id: String,
        name: String,
        repo_full_name: String,
        branch: String,
        image_tag: String,
        ports: Vec<u16>,
        env: HashMap<String, String>,
        /// User-chosen version overriding whatever the cloned source
        /// declares (e.g. an auto-suggested patch bump after a conflict).
        /// Versions are immutable, so there's no overwrite option.
        #[serde(default)]
        version_override: Option<String>,
        /// This agent's `version`/`image`/`status` before the queueing
        /// handler optimistically overwrote them with a placeholder —
        /// `None` for a brand-new agent. Restored verbatim if this clone is
        /// rejected for a version conflict before any build/deploy runs.
        #[serde(default)]
        prior_version: Option<String>,
        #[serde(default)]
        prior_image: Option<String>,
        #[serde(default)]
        prior_status: Option<String>,
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
            | Self::Clone { build_id, .. }
            | Self::GithubClone { build_id, .. }
            | Self::McpServerUpload { build_id, .. } => *build_id,
            Self::Rollback {
                rollback_build_id, ..
            } => *rollback_build_id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Upload { name, .. }
            | Self::Update { name, .. }
            | Self::Clone { name, .. }
            | Self::GithubClone { name, .. }
            | Self::McpServerUpload { name, .. } => name,
            Self::Rollback { agent_name, .. } | Self::StandaloneBuild { agent_name, .. } => {
                agent_name
            }
        }
    }
}

const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB

// ─── POST /upload-and-deploy ─────────────────────────────────────────────────

/// Register a new agent from a source zip and queue an asynchronous
/// build-and-deploy job (poll via `/uploads/{upload_id}` or
/// `/deploys/{build_id}/stream`). Deployer role required.
#[utoipa::path(
    post,
    path = "/api/agents/upload",
    tag = "agents",
    request_body(content = UploadAndDeployForm, content_type = "multipart/form-data"),
    responses(
        (status = 202, description = "Build queued", body = UploadAndDeployResponse),
        (status = 400, description = "Missing/invalid name, version_tag, or source zip"),
        (status = 413, description = "Upload exceeds 100 MiB limit"),
    ),
)]
pub(crate) async fn upload_and_deploy(
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
    // Which LLM SDK the agent's code speaks (drives gateway env injection). Default openai.
    let mut inbound_format: Option<String> = None;
    // Mounts a persistent, private-per-agent directory at /workspace (see
    // DeploymentSpec::writable's doc comment). Tri-state: the CLI omits the
    // field entirely unless `--writable` was passed, and for an existing agent
    // "not specified" must mean "keep the stored value" — collapsing it to
    // false would silently detach the agent from its volume on every plain
    // re-upload of a new version. None = carry forward (false for a new agent).
    let mut writable: Option<bool> = None;
    // Container-side mount target (--writable-path); implies `writable`.
    // Tri-state for the same reason: an unspecified path must not move an
    // existing agent's mount back to /workspace.
    let mut writable_path: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" | "agent_name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "inbound_format" => inbound_format = field.text().await.ok(),
            "writable" => {
                // Anything other than an explicit true/false (unreadable field,
                // junk value) counts as "not specified", not as false.
                writable = match field.text().await.ok().as_deref() {
                    Some("true") => Some(true),
                    Some("false") => Some(false),
                    _ => None,
                };
            }
            "writable_path" => {
                writable_path = field.text().await.ok().filter(|s| !s.is_empty());
            }
            "source" | "file" => {
                use crate::multipart_util::{StreamUploadError, stream_field_to_fresh_temp_file};
                match stream_field_to_fresh_temp_file(
                    "nasiko-upload",
                    "upload.zip",
                    field,
                    MAX_UPLOAD_BYTES,
                )
                .await
                {
                    Ok(path) => zip_path = Some(path),
                    Err(StreamUploadError::TooLarge) => {
                        tracing::warn!(
                            limit = MAX_UPLOAD_BYTES,
                            "upload rejected: size limit exceeded"
                        );
                        return (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds 100 MiB")
                            .into_response();
                    }
                    Err(StreamUploadError::ReadFailed(e)) => {
                        tracing::warn!(%e, "upload: read multipart chunk failed");
                        return (StatusCode::BAD_REQUEST, "failed to read upload stream")
                            .into_response();
                    }
                    Err(StreamUploadError::Io(e)) => {
                        tracing::error!(%e, "upload: stream to disk failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
                            .into_response();
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
                    && let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text)
                {
                    env = map;
                }
            }
            _ => {}
        }
    }

    // A path implies the mount (`--writable-path X` alone must not silently
    // deploy without storage), and a bad path is a caller error — reject now
    // with a 400 instead of surfacing it minutes later as a failed deploy.
    if let Some(path) = &writable_path {
        writable = Some(true);
        if let Err(e) = nasiko_runtime::validate_writable_path(path) {
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
    }

    let name = match name {
        Some(n) if !n.is_empty() => n,
        _ => return (StatusCode::BAD_REQUEST, "name is required").into_response(),
    };
    // `version_tag` isn't resolved here — it may still come from the zip's
    // AgentCard.json/pyproject.toml/Cargo.toml, discovered during validation
    // below. Resolved and validated as a plain x.y.z once that's known (no
    // default to `"latest"` — that's the whole bug this PR exists to prevent).
    // Accept only the supported SDK formats; anything else falls back to openai.
    let inbound_format = match inbound_format.as_deref() {
        Some("anthropic") => "anthropic",
        Some("gemini") => "gemini",
        _ => "openai",
    };

    // Validate name charset (RUN-10): flows into the OCI image reference.
    if let Err(e) = crate::build::routes::validate_version_tag(&name) {
        return (StatusCode::BAD_REQUEST, format!("invalid name: {e}")).into_response();
    }
    let zip_path = match zip_path {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "source zip is required").into_response(),
    };

    // ── Agent structure validation ────────────────────────────────────────────
    // Extract to a temp dir, validate, then clean up (the zip stays for the worker).
    let validation_dir = zip_path
        .parent()
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
    })
    .await;

    // Clean up validation dir regardless of outcome
    let _ = tokio::fs::remove_dir_all(&validation_dir).await;

    let zip_meta = match validation_result {
        Ok(Ok(meta)) => meta,
        Ok(Err(msg)) => {
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
        Err(e) => {
            tracing::error!(%e, "upload: validation task join error");
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    // Version priority: explicit multipart field → extracted from the zip's
    // AgentCard.json/pyproject.toml/Cargo.toml. No default here (used to be
    // "latest", which broke version history) — the caller must end up with a
    // real x.y.z version, whether they typed it or the zip declared it.
    let version_tag = match version_tag.or(zip_meta.version) {
        Some(v) if super::versions::parse_plain_version(&v).is_some() => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "version_tag is required and must be in x.y.z format (e.g. 1.2.3) — pass it \
                 explicitly or declare a \"version\" in AgentCard.json/pyproject.toml/Cargo.toml",
            )
                .into_response();
        }
    };

    let image_tag =
        crate::agents::build_image_tag(&state.config.agent_image_registry, &name, &version_tag);
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
    //
    // `writable`/`writable_path` COALESCE on conflict, not overwrite: a NULL
    // bind means the caller didn't specify them (the CLI omits both fields
    // unless the flags were passed), and an unconditional `EXCLUDED.writable`
    // would reset the flag to false on every plain re-upload — silently
    // detaching a `--writable` agent from its volume, the exact drop the
    // update/rollback/restart paths already guard against. RETURNING hands
    // back the effective values so the deploy uses the DB's truth. (There is
    // deliberately no way to clear a stored writable_path back to NULL here —
    // pass an explicit new path instead; clearing would move the mount.)
    let (agent_id, writable, writable_path) = {
        match sqlx::query_as::<_, (Uuid, bool, Option<String>)>(
            "INSERT INTO agents (name, owner_id, version, image, status, inbound_format, writable, writable_path) \
             VALUES ($1, $2, $3, $4, 'deploying', $5, COALESCE($6, false), $7) \
             ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL \
             DO UPDATE SET version = EXCLUDED.version, image = EXCLUDED.image, \
                           inbound_format = EXCLUDED.inbound_format, \
                           writable = COALESCE($6, agents.writable), \
                           writable_path = COALESCE($7, agents.writable_path), \
                           status = 'deploying', updated_at = now() \
             RETURNING id, writable, writable_path",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(&version_tag)
        .bind(&image_tag)
        .bind(inbound_format)
        .bind(writable)
        .bind(&writable_path)
        .fetch_one(&mut *tx)
        .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::error!(%e, agent_name = %name, "upload: register agent db error");
                let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
    };

    // Reject a version already recorded in this agent's history — otherwise the
    // post-build write (below) silently overwrites that row via `ON CONFLICT`,
    // the same collapse this PR fixes. Checked here (before the build even
    // starts) rather than after, so a doomed upload fails fast.
    let version_already_used: bool = match sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_versions WHERE agent_id = $1 AND version = $2)",
    )
    .bind(agent_id)
    .bind(&version_tag)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(exists) => exists,
        Err(e) => {
            tracing::error!(%e, %agent_id, "upload: version history check db error");
            let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    if version_already_used {
        let _ = tokio::fs::remove_dir_all(zip_path.parent().unwrap_or(&zip_path)).await;
        return (
            StatusCode::CONFLICT,
            format!(
                "version {version_tag} already exists in this agent's history — choose a new version"
            ),
        )
            .into_response();
    }

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

    // Wire the agent's LLM SDK through the gateway (mint JWT + inject base-URL/key per the
    // agent's inbound_format). Best-effort; skipped (with a warning) if the gateway isn't
    // configured. Injected before the build job is enqueued so the worker deploys with it.
    crate::llm_router::wiring::inject_agent_llm_env(&state.db, &mut env, agent_id, Some(owner_id))
        .await;

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
        writable,
        writable_path: writable_path.clone(),
    };

    let payload_value = match serde_json::to_value(&payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(%e, %agent_id, "upload: serialize build payload failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    if let Err(e) =
        sqlx::query("INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)")
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
    set_upload_status(
        &state.db,
        &upload_id,
        &name,
        owner_id,
        "initiated",
        Some(agent_id),
        None,
    )
    .await;

    // Tag this upload so the UI can show the source type.
    let _ = sqlx::query(
        "UPDATE upload_status SET metadata = jsonb_set(metadata, '{upload_type}', '\"zip\"') WHERE upload_id = $1",
    )
    .bind(&upload_id)
    .execute(&state.db)
    .await;

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
/// Metadata extracted from the zip during validation.
struct ZipMeta {
    /// Version from AgentCard.json / pyproject.toml / Cargo.toml (if found).
    version: Option<String>,
}

fn validate_agent_zip(
    zip_path: &std::path::Path,
    dest: &std::path::Path,
) -> Result<ZipMeta, String> {
    extract_zip_from_file(zip_path, dest)?;

    // Dockerfile must exist in root and have at least one FROM line
    let dockerfile = dest.join("Dockerfile");
    if !dockerfile.exists() {
        tracing::warn!(zip_path = %zip_path.display(), reason = "missing Dockerfile", "upload rejected: invalid agent structure");
        return Err("no Dockerfile found in root of zip".into());
    }
    let contents =
        std::fs::read_to_string(&dockerfile).map_err(|e| format!("read Dockerfile: {e}"))?;
    if !contents
        .lines()
        .any(|l| l.trim_start().starts_with("FROM "))
    {
        tracing::warn!(zip_path = %zip_path.display(), reason = "Dockerfile missing FROM", "upload rejected: invalid agent structure");
        return Err("Dockerfile has no FROM instruction".into());
    }

    // At least one Python entrypoint must exist
    let entrypoints = ["main.py", "src/main.py", "src/__main__.py", "__main__.py"];
    let has_entrypoint = entrypoints.iter().any(|p| dest.join(p).exists());
    if !has_entrypoint {
        tracing::warn!(zip_path = %zip_path.display(), reason = "missing entrypoint", "upload rejected: invalid agent structure");
        return Err(
            "no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)"
                .into(),
        );
    }

    // ── Extract version from project files ───────────────────────────────────
    // Resolution order mirrors the CLI: AgentCard.json → pyproject.toml → Cargo.toml.
    let version = detect_version_from_dir(dest);

    Ok(ZipMeta { version })
}

/// Read a version string from common project files in a directory.
///
/// Resolution order:
///   1. AgentCard.json → `version`
///   2. pyproject.toml → `[project] version` or `[tool.poetry] version`
///   3. Cargo.toml     → `[package] version`
fn detect_version_from_dir(dir: &std::path::Path) -> Option<String> {
    // 1. AgentCard.json
    let card_path = dir.join("AgentCard.json");
    if card_path.exists()
        && let Ok(s) = std::fs::read_to_string(&card_path)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&s)
        && let Some(ver) = v.get("version").and_then(|v| v.as_str())
    {
        return Some(ver.strip_prefix('v').unwrap_or(ver).to_string());
    }

    // 2. pyproject.toml — [project] version or [tool.poetry] version
    let pyproject_path = dir.join("pyproject.toml");
    if pyproject_path.exists()
        && let Ok(s) = std::fs::read_to_string(&pyproject_path)
        && let Some(ver) = parse_toml_version(&s, &["project", "tool.poetry"])
    {
        return Some(ver);
    }

    // 3. Cargo.toml — [package] version
    let cargo_path = dir.join("Cargo.toml");
    if cargo_path.exists()
        && let Ok(s) = std::fs::read_to_string(&cargo_path)
        && let Some(ver) = parse_toml_version(&s, &["package"])
    {
        return Some(ver);
    }

    None
}

/// Minimal TOML version extractor: scans for `version = "..."` under any of the
/// given section headers. No TOML parser dependency.
fn parse_toml_version(content: &str, sections: &[&str]) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let header = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            in_section = sections.contains(&header);
            continue;
        }
        if in_section
            && let Some(rest) = trimmed.strip_prefix("version")
            && let Some(rest) = rest.trim().strip_prefix('=')
        {
            let ver = rest.trim().trim_matches('"').trim_matches('\'');
            if !ver.is_empty() {
                return Some(ver.to_string());
            }
        }
    }
    None
}

/// Records a just-deployed build's version in history through the shared
/// recorder — activates it, archiving whatever was running before and
/// marking it rollback-eligible. Shared by `execute_upload_and_deploy` and
/// `execute_clone_and_deploy`, whose build pipelines are otherwise
/// identical from this point on.
///
/// `agent_builds.version_tag` (not the `image_tag` parameter callers already
/// have) is the actual `x.y.z` version string `record_version_change` needs.
async fn record_uploaded_version(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    build_id: Uuid,
    image_tag: &str,
) {
    let version_tag: Option<String> =
        sqlx::query_scalar("SELECT version_tag FROM agent_builds WHERE id = $1")
            .bind(build_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    let Some(version_tag) = version_tag else {
        tracing::error!(
            %agent_id, %build_id,
            "upload: no version_tag found for build — version not recorded in history"
        );
        return;
    };

    super::versions::record_version_change_with_retry(db, || super::versions::VersionChange {
        agent_id,
        build_id: Some(build_id),
        version: &version_tag,
        image_tag,
        changelog: None,
    })
    .await;
}

/// Bumps `base`'s patch number until it finds a version not already used —
/// server-side mirror of the CLI's `suggest_unused_version`.
async fn suggest_next_version(db: &sqlx::PgPool, agent_id: Uuid, base: &str) -> String {
    let used: Vec<String> =
        sqlx::query_scalar("SELECT version FROM agent_versions WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_all(db)
            .await
            .unwrap_or_default();

    let bump = |v: &str| {
        super::versions::parse_plain_version(v)
            .map(|mut sv| {
                sv.patch += 1;
                sv.to_string()
            })
            .unwrap_or_else(|| "0.1.0".to_string())
    };
    let mut candidate = bump(base);
    while used.iter().any(|u| u == &candidate) {
        candidate = bump(&candidate);
    }
    candidate
}

// ─── Build-time OTel patching ────────────────────────────────────────────────

/// Python bootstrap script injected as `_nasiko_otel_boot.py` and loaded via
/// `PYTHONSTARTUP`. Runs before the agent's own code, so the agent doesn't need
/// to call `init_telemetry()` or install any OTel packages explicitly.
///
/// What it does:
/// - Sets up W3C TraceContext propagation (`traceparent` on all outbound HTTP)
/// - Auto-instruments httpx, requests, and the OpenAI/Anthropic SDKs
/// - Exports traces + metrics to the OTLP collector if `OTEL_EXPORTER_OTLP_ENDPOINT` is set
///
/// Gracefully no-ops if the OTel packages aren't installed (shouldn't happen
/// since `patch_otel_into_dockerfile` adds them to the Dockerfile).
const OTEL_BOOTSTRAP_PY: &str = r#""""Auto-injected by the Nasiko build pipeline — DO NOT EDIT."""
import os as _os, logging as _logging

def _nasiko_otel_boot():
    try:
        from opentelemetry import trace, metrics
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import BatchSpanProcessor
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.propagate import set_global_textmap
        from opentelemetry.propagators.composite import CompositePropagator
        from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator
    except ImportError:
        return

    name = _os.environ.get("OTEL_SERVICE_NAME", "nasiko-agent")
    resource = Resource.create({"service.name": name})
    set_global_textmap(CompositePropagator([TraceContextTextMapPropagator()]))
    tp = TracerProvider(resource=resource)

    endpoint = _os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    if endpoint:
        try:
            from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
            from opentelemetry.sdk.metrics import MeterProvider
            from opentelemetry.sdk.metrics.export import PeriodicExportingMetricReader
            from opentelemetry.exporter.otlp.proto.grpc.metric_exporter import OTLPMetricExporter
            tp.add_span_processor(BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True)))
            metrics.set_meter_provider(MeterProvider(
                resource=resource,
                metric_readers=[PeriodicExportingMetricReader(
                    OTLPMetricExporter(endpoint=endpoint, insecure=True),
                    export_interval_millis=10000,
                )],
            ))
        except Exception:
            pass

    trace.set_tracer_provider(tp)

    # Auto-instrument HTTP clients + LLM SDKs (best-effort per library).
    for mod_path, cls in [
        ("opentelemetry.instrumentation.httpx", "HTTPXClientInstrumentor"),
        ("opentelemetry.instrumentation.requests", "RequestsInstrumentor"),
        ("opentelemetry.instrumentation.openai_v2", "OpenAIInstrumentor"),
        ("opentelemetry.instrumentation.openai", "OpenAIInstrumentor"),
        ("opentelemetry.instrumentation.anthropic", "AnthropicInstrumentor"),
    ]:
        try:
            import importlib
            instrumentor = getattr(importlib.import_module(mod_path), cls)()
            if not instrumentor.is_instrumented_by_opentelemetry:
                instrumentor.instrument()
        except Exception:
            pass

_nasiko_otel_boot()
del _nasiko_otel_boot
"#;

/// OTel pip packages injected into the Dockerfile. Kept minimal — only what the
/// bootstrap script actually imports. `--no-deps` would be ideal but some of
/// these have transitive deps, so we let pip resolve.
const OTEL_PIP_PACKAGES: &str = "\
    opentelemetry-api \
    opentelemetry-sdk \
    opentelemetry-exporter-otlp-proto-grpc \
    opentelemetry-instrumentation-httpx \
    opentelemetry-instrumentation-requests \
    opentelemetry-instrumentation-openai-v2";

/// Patch a Python agent's Dockerfile to auto-install OTel packages and inject
/// the bootstrap script. Skips non-Python Dockerfiles (no `python` base image).
/// Best-effort: errors are logged and the build proceeds unpatched.
fn patch_otel_into_dockerfile(source_dir: &std::path::Path, dockerfile: &std::path::Path) {
    let contents = match std::fs::read_to_string(dockerfile) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(%e, "otel patch: cannot read Dockerfile, skipping");
            return;
        }
    };

    // Only patch Python-based images. Matching bare `slim`/`alpine` here also
    // catches `FROM node:20-slim`, `FROM ruby:3-alpine`, and friends — and since
    // the injected `pip install` layer then fails on an image with no pip, that
    // mismatch doesn't merely skip instrumentation, it fails the whole build for
    // an agent that was never Python to begin with. Require `python` in the base
    // image ref, matching this function's documented contract.
    let is_python = contents
        .lines()
        .any(|l| l.trim().starts_with("FROM ") && l.contains("python"));
    if !is_python {
        tracing::debug!("otel patch: Dockerfile does not appear Python-based, skipping");
        return;
    }

    // Don't double-patch if the agent already bundles the bootstrap.
    if source_dir.join("_nasiko_otel_boot.py").exists() {
        tracing::debug!("otel patch: _nasiko_otel_boot.py already exists, skipping");
        return;
    }

    // Write the bootstrap script.
    if let Err(e) = std::fs::write(source_dir.join("_nasiko_otel_boot.py"), OTEL_BOOTSTRAP_PY) {
        tracing::warn!(%e, "otel patch: failed to write bootstrap script, skipping");
        return;
    }

    // Append to Dockerfile: install OTel deps, copy bootstrap, set PYTHONSTARTUP.
    // Inserted before the last CMD/ENTRYPOINT line so the layer order is correct.
    // `PIP_BREAK_SYSTEM_PACKAGES=1` is scoped to this RUN layer (not a persistent
    // ENV) and keeps the install working on a distro-managed interpreter, where
    // PEP 668 otherwise aborts with `error: externally-managed-environment`.
    // pip older than 23.1 doesn't know the flag and simply ignores the env var.
    let patch = format!(
        "\n# ── Nasiko OTel auto-instrumentation (injected at build time) ──\n\
         RUN PIP_BREAK_SYSTEM_PACKAGES=1 pip install --no-cache-dir {OTEL_PIP_PACKAGES}\n\
         COPY _nasiko_otel_boot.py /opt/nasiko/_nasiko_otel_boot.py\n\
         ENV PYTHONSTARTUP=/opt/nasiko/_nasiko_otel_boot.py\n"
    );

    // Find the last CMD or ENTRYPOINT line and insert before it.
    let lines: Vec<&str> = contents.lines().collect();
    let insert_pos = lines
        .iter()
        .rposition(|l| {
            let t = l.trim();
            t.starts_with("CMD ") || t.starts_with("ENTRYPOINT ")
        })
        .unwrap_or(lines.len());

    let mut patched = lines[..insert_pos].join("\n");
    patched.push_str(&patch);
    patched.push_str(&lines[insert_pos..].join("\n"));
    patched.push('\n');

    if let Err(e) = std::fs::write(dockerfile, &patched) {
        tracing::warn!(%e, "otel patch: failed to write patched Dockerfile");
        return;
    }

    tracing::info!("otel patch: injected OTel auto-instrumentation into Dockerfile");
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
    max_replicas: u32,
    writable: bool,
    writable_path: Option<String>,
    default_memory: String,
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

        // ── OTel patch ───────────────────────────────────────────────────────
        // Inject traceparent propagation + GenAI instrumentation into Python
        // agents so they get traces, LLM spans, and classifier support without
        // any agent-side code changes. Best-effort: a non-Python Dockerfile is
        // left untouched.
        patch_otel_into_dockerfile(&tmp_dir, &dockerfile_path);

        // Build Docker image.
        let tar_bytes = build::tar_directory(&tmp_dir).map_err(|e| format!("tar source: {e}"))?;
        runtime
            .build(&tar_bytes, &image_tag)
            .await
            .map_err(|e| format!("docker build: {e}"))?;

        set_upload_status(
            &db,
            &upload_id,
            &name,
            owner_id,
            "orchestration_triggered",
            None,
            None,
        )
        .await;

        // Deploy container keyed on agent UUID (not name) — see build_agent_spec.
        let mut spec = crate::agents::build_agent_spec(
            agent_id,
            &name,
            image_tag.clone(),
            ports,
            env,
            &default_memory,
            max_replicas,
            writable,
            writable_path.clone(),
            owner_id,
        );
        crate::agents::attach_pull_credential(
            &db,
            &agent_runtime,
            &agent_image_registry,
            &mut spec,
            agent_id,
        )
        .await;
        let deploy_status = runtime
            .deploy(&spec)
            .await
            .map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(
            &db,
            &upload_id,
            &name,
            owner_id,
            "orchestration_processing",
            None,
            None,
        )
        .await;

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
            set_upload_status(
                &db,
                &upload_id,
                &name,
                owner_id,
                "completed",
                Some(agent_id),
                None,
            )
            .await;
            // `upload` upserts by (owner_id, name) — a second `upload` against an
            // already-deployed agent must land here too, which this activates and
            // archives whatever was previously running for (mirroring `update.rs`'s
            // redeploy path). A genuinely first upload has nothing to archive yet.
            record_uploaded_version(&db, agent_id, build_id, &image_tag).await;
            let agent_url = crate::agents::resolve_agent_url(
                &runtime,
                &deploy_status,
                &nasiko_runtime::ContainerId::from_uuid(agent_id),
            )
            .await;
            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
            )
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
            set_upload_status(
                &db,
                &upload_id,
                &name,
                owner_id,
                "failed",
                None,
                Some("upload and deploy failed"),
            )
            .await;
            super::utils::delete_agent_or_mark_failed(&db, agent_id).await;
            tracing::error!(build_id = %build_id, %e, "upload-and-deploy failed");
        }
    }
}

/// Execute the full clone-and-deploy pipeline: extract tar.gz, OTel patch, docker build, deploy.
/// Called by the build worker for `BuildJobPayload::Clone` jobs.
#[allow(clippy::too_many_arguments)]
pub async fn execute_clone_and_deploy(
    runtime: std::sync::Arc<dyn nasiko_runtime::ContainerRuntime>,
    db: sqlx::PgPool,
    http: reqwest::Client,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    upload_id: String,
    name: String,
    tar_gz_path: PathBuf,
    ports: Vec<u16>,
    mut env: HashMap<String, String>,
    openai_api_key: Option<String>,
    openai_base_url: Option<String>,
    agent_runtime: String,
    agent_image_registry: String,
    max_replicas: u32,
    default_memory: String,
    version_override: Option<String>,
    // What this agent's `version`/`image`/`status` were before the queueing
    // handler optimistically overwrote them with a placeholder — `None` for
    // a brand-new agent with nothing to restore. Used to put the row back
    // exactly as it was if this attempt is rejected for a version conflict
    // before any build/deploy runs (see the `VERSION_CONFLICT` branch below).
    prior_version: Option<String>,
    prior_image: Option<String>,
    prior_status: Option<String>,
) {
    if let Some(key) = openai_api_key {
        env.entry("OPENAI_API_KEY".to_owned()).or_insert(key);
    }
    if let Some(url) = openai_base_url {
        env.entry("OPENAI_BASE_URL".to_owned()).or_insert(url);
    }
    set_build_status(&db, build_id, BuildStatus::Building).await;
    set_upload_status(&db, &upload_id, &name, owner_id, "initiated", None, None).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-clone-{build_id}"));

    let result: Result<(DeploymentStatus, String), String> = async {
        // Read tar.gz bytes then extract on the blocking pool.
        let tp = tar_gz_path.clone();
        let td = tmp_dir.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(&tp).map_err(|e| format!("read tar.gz: {e}"))?;
            build::extract_tar_gzip(&bytes, &td)
        })
        .await
        .map_err(|e| format!("spawn_blocking extract: {e}"))??;

        set_upload_status(&db, &upload_id, &name, owner_id, "processing", None, None).await;

        // ── Extract version from project files (same logic as zip upload) ────
        // Resolution order: AgentCard.json → pyproject.toml → Cargo.toml.
        // If a valid x.y.z version is found, update the image tag and DB records
        // so the clone path doesn't default everything to "latest".
        let image_tag = {
            // An explicit override (the UI's "deploy as vX.Y.Z" suggestion)
            // takes precedence over whatever the source repo declares.
            let detected = version_override.clone().or_else(|| detect_version_from_dir(&tmp_dir));
            // No default here (used to be the placeholder "latest", which
            // broke version history) — the caller must end up with a real
            // x.y.z version, whether the repo declares one or the UI's
            // version-bump override supplied it.
            let ver = match detected
                .as_deref()
                .filter(|v| super::versions::parse_plain_version(v).is_some())
            {
                Some(v) => v.to_string(),
                None => {
                    return Err(
                        "no valid version found — add an x.y.z \"version\" field to \
                         AgentCard.json (or pyproject.toml/Cargo.toml) before importing"
                            .to_string(),
                    );
                }
            };

            // Fail fast before building, and before touching
            // agents/agent_builds, if this version already exists. Versions
            // are immutable — no overwrite, ever. This check can race (two
            // concurrent imports both seeing "not used yet"), but it's only
            // a fast-fail: the `UNIQUE(agent_id, version)` constraint is the
            // real guard, enforced when the version is actually recorded
            // after a successful deploy (see `record_uploaded_version`).
            let version_already_used = super::versions::version_exists(&db, agent_id, &ver)
                .await
                .map_err(|e| format!("check version history: {e}"))?;
            if version_already_used {
                return Err(format!("VERSION_CONFLICT:{ver}"));
            }

            let new_tag = crate::agents::build_image_tag(&agent_image_registry, &name, &ver);
            // Update agents.version + agent_builds.version_tag/image_reference
            // to reflect the real version instead of the placeholder.
            let _ = sqlx::query(
                "UPDATE agents SET version = $2, image = $3, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&ver)
            .bind(&new_tag)
            .execute(&db)
            .await;
            let _ = sqlx::query(
                "UPDATE agent_builds SET version_tag = $2, image_reference = $3 WHERE id = $1",
            )
            .bind(build_id)
            .bind(&ver)
            .bind(&new_tag)
            .execute(&db)
            .await;
            tracing::info!(%build_id, %agent_id, version = %ver, "clone: detected version from source");
            new_tag
        };

        let dockerfile_path = tmp_dir.join("Dockerfile");
        if !dockerfile_path.exists() {
            return Err("no Dockerfile found in cloned repository".into());
        }

        // OTel patch (same as upload path — see doc on `patch_otel_into_dockerfile`).
        patch_otel_into_dockerfile(&tmp_dir, &dockerfile_path);

        // Build Docker image.
        let tar_bytes = build::tar_directory(&tmp_dir).map_err(|e| format!("tar source: {e}"))?;
        runtime
            .build(&tar_bytes, &image_tag)
            .await
            .map_err(|e| format!("docker build: {e}"))?;

        set_upload_status(
            &db,
            &upload_id,
            &name,
            owner_id,
            "orchestration_triggered",
            None,
            None,
        )
        .await;

        // Deploy container keyed on agent UUID.
        let mut spec = crate::agents::build_agent_spec(
            agent_id,
            &name,
            image_tag.clone(),
            ports,
            env,
            &default_memory,
            max_replicas,
            // GitHub-clone deploys don't expose a --writable flag yet.
            false,
            None,
            owner_id,
        );
        crate::agents::attach_pull_credential(
            &db,
            &agent_runtime,
            &agent_image_registry,
            &mut spec,
            agent_id,
        )
        .await;
        let deploy_status = runtime
            .deploy(&spec)
            .await
            .map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(
            &db,
            &upload_id,
            &name,
            owner_id,
            "orchestration_processing",
            None,
            None,
        )
        .await;

        Ok((deploy_status, image_tag))
    }
    .await;

    // Clean up extracted dir and the tar.gz file.
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    if let Some(tar_dir) = tar_gz_path.parent() {
        let _ = tokio::fs::remove_dir_all(tar_dir).await;
    }

    match result {
        Ok((deploy_status, final_image_tag)) => {
            set_build_status(&db, build_id, BuildStatus::Success).await;
            set_upload_status(
                &db,
                &upload_id,
                &name,
                owner_id,
                "completed",
                Some(agent_id),
                None,
            )
            .await;
            // `upload` upserts by (owner_id, name) — a second `upload` against an
            // already-deployed agent must land here too, which this activates and
            // archives whatever was previously running for (mirroring `update.rs`'s
            // redeploy path). A genuinely first upload has nothing to archive yet.
            record_uploaded_version(&db, agent_id, build_id, &final_image_tag).await;
            let agent_url = crate::agents::resolve_agent_url(
                &runtime,
                &deploy_status,
                &nasiko_runtime::ContainerId::from_uuid(agent_id),
            )
            .await;
            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&agent_url)
            .execute(&db)
            .await;
            tokio::spawn(super::utils::fetch_agent_card_with_retry(
                db.clone(),
                http,
                agent_id,
                agent_url.clone(),
            ));
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
            tracing::info!(build_id = %build_id, agent_id = %agent_id, "clone-and-deploy succeeded");
        }
        Err(e) => {
            set_build_status(&db, build_id, BuildStatus::Failed).await;
            if let Some(ver) = e.strip_prefix("VERSION_CONFLICT:") {
                // Rejected before any build/deploy ran, so don't delete the
                // agent or mark it failed — restore it to exactly what it
                // was before the queuing handler optimistically overwrote
                // its version/image/status with a placeholder. A brand-new
                // agent has nothing to restore to, so clean it up instead —
                // same as any other pre-build rejection.
                match (&prior_version, &prior_status) {
                    (Some(pv), Some(ps)) => {
                        let _ = sqlx::query(
                            "UPDATE agents SET version = $2, image = $3, status = $4, \
                             updated_at = now() WHERE id = $1",
                        )
                        .bind(agent_id)
                        .bind(pv)
                        .bind(&prior_image)
                        .bind(ps)
                        .execute(&db)
                        .await;
                    }
                    _ => {
                        super::utils::delete_agent_or_mark_failed(&db, agent_id).await;
                    }
                }
                // Prefixed so the client can offer "deploy as vX" instead of
                // a dead-end error. See `add-agent-github-page.js`.
                let suggested = suggest_next_version(&db, agent_id, ver).await;
                set_upload_status(
                    &db,
                    &upload_id,
                    &name,
                    owner_id,
                    "failed",
                    None,
                    Some(&format!(
                        "VERSION_CONFLICT:{ver}:{suggested}:{name} version {ver} already \
                         exists and versions are immutable"
                    )),
                )
                .await;
                tracing::warn!(build_id = %build_id, %agent_id, version = %ver, "clone-and-deploy rejected: version already exists");
            } else {
                set_upload_status(
                    &db,
                    &upload_id,
                    &name,
                    owner_id,
                    "failed",
                    None,
                    Some("clone and deploy failed"),
                )
                .await;
                super::utils::delete_agent_or_mark_failed(&db, agent_id).await;
                tracing::error!(build_id = %build_id, %e, "clone-and-deploy failed");
            }
        }
    }
}

/// Execute the GitHub clone-and-deploy pipeline from inside the build worker.
///
/// Loads the owner's stored GitHub token, runs `git clone` via `GitHubService`,
/// writes the resulting archive to disk, then hands off to [`execute_clone_and_deploy`].
/// Because this runs in the build worker (not the HTTP handler), transient failures
/// are automatically retried up to `MAX_ATTEMPTS` times and the caller sees an
/// async 202 rather than a 502.
#[allow(clippy::too_many_arguments)]
pub async fn execute_github_clone_and_deploy(
    state: crate::state::AppState,
    build_id: Uuid,
    agent_id: Uuid,
    owner_id: Uuid,
    upload_id: String,
    name: String,
    repo_full_name: String,
    branch: String,
    ports: Vec<u16>,
    env: HashMap<String, String>,
    version_override: Option<String>,
    prior_version: Option<String>,
    prior_image: Option<String>,
    prior_status: Option<String>,
) {
    // GitHub service must be configured for cloning to work.
    let github_svc = match state.github_svc.as_ref() {
        Some(svc) => svc.clone(),
        None => {
            tracing::error!(%build_id, "github_clone_and_deploy: GitHub OAuth not configured");
            fail_github_clone_terminal(
                &state.db,
                build_id,
                agent_id,
                &upload_id,
                &name,
                owner_id,
                "GitHub OAuth not configured",
            )
            .await;
            return;
        }
    };

    // Load the owner's GitHub access token from the DB.
    let token: Option<String> = {
        use nasiko_secrets::SecretsCrypto;
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT provider_metadata FROM user_identities \
             WHERE user_id = $1 AND provider = 'github'",
        )
        .bind(owner_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        row.and_then(|(meta,)| {
            meta.get("access_token")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .and_then(|encrypted| SecretsCrypto::for_user(owner_id).decrypt(&encrypted).ok())
    };

    let token = match token {
        Some(t) => t,
        None => {
            tracing::error!(%build_id, %owner_id, "github_clone_and_deploy: no GitHub token for owner");
            fail_github_clone_terminal(
                &state.db,
                build_id,
                agent_id,
                &upload_id,
                &name,
                owner_id,
                "GitHub not connected",
            )
            .await;
            return;
        }
    };

    // Shallow-clone the repository into an in-memory tar.gz archive.
    let archive = match github_svc
        .clone_to_archive(&token, &repo_full_name, &branch)
        .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(%build_id, %e, repo = %repo_full_name, "github_clone_and_deploy: git clone failed");
            fail_github_clone_terminal(
                &state.db,
                build_id,
                agent_id,
                &upload_id,
                &name,
                owner_id,
                "git clone failed",
            )
            .await;
            return;
        }
    };

    // Persist the archive to disk so execute_clone_and_deploy can read it.
    let tar_dir = std::env::temp_dir().join(format!("nasiko-ghclone-src-{build_id}"));
    let tar_gz_path = tar_dir.join("source.tar.gz");
    let write_ok = tokio::fs::create_dir_all(&tar_dir)
        .await
        .and(tokio::fs::write(&tar_gz_path, &archive.archive_bytes).await);

    if let Err(e) = write_ok {
        tracing::error!(%build_id, %e, "github_clone_and_deploy: write archive failed");
        let _ = tokio::fs::remove_dir_all(&tar_dir).await;
        fail_github_clone_terminal(
            &state.db,
            build_id,
            agent_id,
            &upload_id,
            &name,
            owner_id,
            "internal error saving archive",
        )
        .await;
        return;
    }

    let mut platform_env = state.agent_env(agent_id).await;
    platform_env.extend(env);
    execute_clone_and_deploy(
        state.runtime.clone(),
        state.db.clone(),
        state.http_client.clone(),
        build_id,
        agent_id,
        owner_id,
        upload_id,
        name,
        tar_gz_path,
        ports,
        platform_env,
        state.config.openai_api_key.clone(),
        state.config.openai_base_url.clone(),
        state.config.agent_runtime.clone(),
        state.config.agent_image_registry.clone(),
        state.config.agent_max_replicas,
        state.config.agent_default_memory.clone(),
        version_override,
        prior_version,
        prior_image,
        prior_status,
    )
    .await;
}

/// Drive the agent and build to a terminal failed state when the clone step
/// fails before `execute_clone_and_deploy` can take over status management.
async fn fail_github_clone_terminal(
    db: &sqlx::PgPool,
    build_id: Uuid,
    agent_id: Uuid,
    upload_id: &str,
    name: &str,
    owner_id: Uuid,
    reason: &str,
) {
    set_build_status(db, build_id, BuildStatus::Failed).await;
    set_upload_status(db, upload_id, name, owner_id, "failed", None, Some(reason)).await;
    super::utils::delete_agent_or_mark_failed(db, agent_id).await;
}

// ─── GET /deploy-status/{build_id} (SSE) ─────────────────────────────────────

/// Server-Sent Events stream of `{"status": ..., "build_id": ...}` on each
/// status change, polling every 3s until the build reaches `success`/`failed`
/// (or `{"status": "not_found"}` once, if the build id doesn't exist).
/// Callers below deployer role get `200 {"available": false}` instead.
#[utoipa::path(
    get,
    path = "/api/agents/deploys/{build_id}/stream",
    tag = "agents",
    params(
        ("build_id" = Uuid, Path, description = "Build id"),
    ),
    responses(
        (status = 200, description = "`text/event-stream` of build status updates, or `{\"available\": false}`", content_type = "text/event-stream"),
    ),
)]
pub(crate) async fn deploy_status_sse(
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

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
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

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourceInfoJson {
    filename: String,
    content_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UploadStatusItem {
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
        "capabilities_generated"
            | "orchestration_triggered"
            | "orchestration_processing"
            | "completed"
    );
    let orch_trig = matches!(
        row.status.as_str(),
        "orchestration_triggered" | "orchestration_processing" | "completed"
    );
    let processing_duration =
        (row.updated_at - row.created_at).num_milliseconds().max(0) as f64 / 1000.0;
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

/// Get the status of a single upload/build (by `upload_id`, i.e. the build id
/// as a string). Callers below deployer role, or a non-owner non-superuser,
/// get `200 {"available": false}` instead of an error.
#[utoipa::path(
    get,
    path = "/api/agents/uploads/{upload_id}",
    tag = "agents",
    params(
        ("upload_id" = String, Path, description = "Upload/build id"),
    ),
    responses(
        (status = 200, description = "Upload status, or `{\"available\": false}`", body = UploadStatusItem),
        (status = 404, description = "No such upload"),
    ),
)]
pub(crate) async fn get_upload_status(
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

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct UploadListQuery {
    #[serde(default = "default_upload_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_upload_limit() -> i64 {
    10
}

/// Response envelope for `GET /upload-status` — documents the shape of the ad
/// hoc `serde_json::json!` object the handler returns.
#[derive(Serialize, ToSchema)]
pub(crate) struct UploadStatusListResponse {
    data: Vec<UploadStatusItem>,
    status_code: u16,
    message: String,
}

/// List the caller's uploads/builds, newest first (superuser → all). Also
/// mounted at the top-level `/api/upload-status` alias (`status_router`).
/// Callers below deployer role get `200 {"available": false}` instead of an error.
#[utoipa::path(
    get,
    path = "/api/agents/uploads",
    tag = "agents",
    params(UploadListQuery),
    responses(
        (status = 200, description = "Upload statuses, or `{\"available\": false}`", body = UploadStatusListResponse),
    ),
)]
pub(crate) async fn list_upload_status(
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
    metadata: sqlx::types::Json<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct UploadInfoResponse {
    upload_type: String,
    upload_status: String,
    status_message: Option<String>,
    error_detail: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct UploadAgentResponse {
    agent_id: String,
    agent_name: String,
    icon_url: Option<String>,
    upload_info: UploadInfoResponse,
    tags: Vec<String>,
    description: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct UploadAgentsListResponse {
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

/// List agents the caller has uploaded (superuser → all), one row per agent
/// (most recent upload), joined with live catalog metadata.
#[utoipa::path(
    get,
    path = "/api/agents/my-uploads",
    tag = "agents",
    responses(
        (status = 200, description = "Uploaded agents, newest first (max 50)", body = UploadAgentsListResponse),
    ),
)]
pub(crate) async fn list_upload_agents(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Join with agents to pull live metadata (tags, description, icon_url, version, status).
    // DISTINCT ON keeps the most recent upload row per agent.
    // Always filter by owner_id — even admins should only see their own uploads.
    let rows: Result<Vec<UploadAgentRow>, _> = sqlx::query_as(
        r#"SELECT DISTINCT ON (COALESCE(us.agent_id::text, us.upload_id))
               us.agent_id,
               us.agent_name,
               us.upload_id,
               us.error_message,
               a.description,
               COALESCE(a.tags, '{}') AS tags,
               a.icon_url,
               a.version,
               a.status AS agent_status,
               us.metadata
           FROM upload_status us
           JOIN agents a ON a.id = us.agent_id AND a.deleted_at IS NULL
           WHERE us.owner_id = $1
           ORDER BY COALESCE(us.agent_id::text, us.upload_id), us.created_at DESC
           LIMIT 50"#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(rows) => {
            let count = rows.len();
            let data = rows
                .into_iter()
                .map(|r| {
                    let display_status = agent_display_status(r.agent_status.as_deref());
                    let status_message = agent_status_message(display_status, r.version.as_deref());
                    UploadAgentResponse {
                        agent_id: r
                            .agent_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| r.upload_id.clone()),
                        agent_name: r.agent_name,
                        icon_url: r.icon_url,
                        upload_info: UploadInfoResponse {
                            upload_type: r
                                .metadata
                                .get("upload_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("zip")
                                .to_string(),
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

#[cfg(test)]
mod otel_patch_tests {
    use super::*;

    /// Write `dockerfile_contents` into a fresh temp dir, run the patch over it,
    /// and hand back what the Dockerfile looks like afterwards.
    fn patch(dockerfile_contents: &str, marker: &str) -> String {
        let dir = std::env::temp_dir().join(format!("nasiko-otel-patch-test-{marker}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dockerfile = dir.join("Dockerfile");
        std::fs::write(&dockerfile, dockerfile_contents).unwrap();

        patch_otel_into_dockerfile(&dir, &dockerfile);

        let patched = std::fs::read_to_string(&dockerfile).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        patched
    }

    #[test]
    fn node_slim_image_is_left_untouched() {
        // `node:20-slim` matches neither "python" nor a Python toolchain, but it
        // does contain "slim" — the old check patched it and the injected `pip`
        // layer failed the build outright.
        let original =
            "FROM node:20-slim\nRUN apt-get install -y python3\nENTRYPOINT [\"./run.sh\"]\n";

        assert_eq!(
            patch(original, "node-slim"),
            original,
            "a Node base image must not receive the Python OTel patch"
        );
    }

    #[test]
    fn alpine_non_python_image_is_left_untouched() {
        let original = "FROM ruby:3-alpine\nENTRYPOINT [\"./run.sh\"]\n";

        assert_eq!(patch(original, "ruby-alpine"), original);
    }

    #[test]
    fn python_image_is_patched_before_the_entrypoint() {
        let patched = patch(
            "FROM python:3.12-slim\nCOPY . /app\nENTRYPOINT [\"python\", \"main.py\"]\n",
            "python-slim",
        );

        assert!(patched.contains("pip install"), "expected the pip layer");
        assert!(patched.contains("ENV PYTHONSTARTUP=/opt/nasiko/_nasiko_otel_boot.py"));

        let pip_at = patched.find("pip install").unwrap();
        let entrypoint_at = patched.find("ENTRYPOINT").unwrap();
        assert!(
            pip_at < entrypoint_at,
            "the pip layer must be inserted before ENTRYPOINT"
        );
    }

    #[test]
    fn pip_layer_tolerates_a_distro_managed_interpreter() {
        let patched = patch(
            "FROM python:3.12-slim\nENTRYPOINT [\"python\", \"main.py\"]\n",
            "pep668",
        );

        // Without this, PEP 668 aborts the layer with
        // `error: externally-managed-environment` on a distro-managed Python.
        assert!(
            patched.contains("PIP_BREAK_SYSTEM_PACKAGES=1 pip install"),
            "pip install must be able to write to a distro-managed interpreter"
        );
    }
}
