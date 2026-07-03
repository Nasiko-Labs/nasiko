use axum::{
    Json, Router,
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{post, put},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use nasiko_runtime::DeploymentStatus;

use crate::agents::upload::BuildJobPayload;
use crate::auth::Claims;
use crate::build::{self, BuildStatus, download_repo_tarball, routes::extract_zip_to_dir};
use crate::catalog::agent_secrets;
use crate::github::load_github_token;
use crate::state::AppState;

use super::utils::{set_build_status, set_upload_status};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/update", put(update_agent))
        .route("/{id}/rollback", post(rollback_agent))
}

#[derive(Debug, Serialize)]
struct UpdateAgentResponse {
    build_id: Uuid,
    agent_id: Uuid,
    new_version: String,
    previous_version: String,
    status: &'static str,
}

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
pub struct AgentVersionRow {
    pub version: String,
    pub image_tag: String,
    pub can_rollback: bool,
}

// ─── PUT /{id}/update ─────────────────────────────────────────────────────────

async fn update_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
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
    // Accepts strategy keywords (auto, patch, minor, major) or an explicit semver string (e.g. "1.2.3").
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

    // Write zip to disk so the worker has a durable path after this request completes.
    let zip_path = if let Some(ref data) = source_data {
        let zip_dir = std::env::temp_dir().join(format!("nasiko-update-{build_id}"));
        if let Err(e) = tokio::fs::create_dir_all(&zip_dir).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("create update dir: {e}")).into_response();
        }
        let path = zip_dir.join("upload.zip");
        if let Err(e) = tokio::fs::write(&path, data).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("write update zip: {e}")).into_response();
        }
        Some(path.to_string_lossy().into_owned())
    } else {
        None
    };

    let payload = BuildJobPayload::Update {
        build_id,
        agent_id,
        owner_id,
        name: agent_name.clone(),
        zip_path,
        image_tag: image_tag.clone(),
        new_version: new_version.clone(),
        prev_version: current_version.clone(),
        prev_image,
        changelog,
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(serde_json::to_value(&payload).expect("serialize update payload"))
    .execute(&state.db)
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("queue update: {e}")).into_response();
    }
    let _ = state.build_tx.send(()).await;

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
pub async fn execute_agent_update(
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

    let result: Result<DeploymentStatus, String> = async {
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

        let tar_bytes = build::tar_directory(&agent_source_dir)
            .map_err(|e| format!("tar source: {e}"))?;
        state.runtime.build(&tar_bytes, &image_tag).await.map_err(|e| format!("build: {e}"))?;

        set_upload_status(db, &upload_id, &name, owner_id, "orchestration_triggered", None, None).await;

        let secrets = agent_secrets::resolve_agent_env(db, agent_id).await;
        // Key on the agent UUID (not name) so the update re-targets the existing
        // workload instead of spawning an orphaned name-keyed duplicate (RUN-2/7).
        let spec = crate::agents::build_agent_spec(agent_id, &name, image_tag.clone(), vec![], secrets, None);
        let deploy_status = state.runtime.deploy(&spec).await.map_err(|e| format!("deploy: {e}"))?;

        set_upload_status(db, &upload_id, &name, owner_id, "orchestration_processing", None, None).await;
        Ok(deploy_status)
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(deploy_status) => {
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

            let agent_url = deploy_status.endpoint.unwrap_or_default();
            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&agent_url)
            .execute(db)
            .await;

            // Persist k8s_deployment_name (= agent UUID string) + spec_image so the
            // crash guardian and restart can find/rebuild this workload after an
            // update (RUN-3); without it these columns were NULL and the deployment
            // was invisible to the guardian and restarted on the wrong runtime path.
            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id, k8s_deployment_name, spec_image) \
                 VALUES ($1, $2, 'running', $3, $4, $5)",
            )
            .bind(agent_id)
            .bind(build_id)
            .bind(owner_id)
            .bind(agent_id.to_string())
            .bind(&image_tag)
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

async fn rollback_agent(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    // Use raw bytes so malformed JSON returns 422 instead of silently defaulting
    // to "no body" (which Option<Json<T>> does in Axum 0.8).
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let caller_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
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

    let payload = BuildJobPayload::Rollback {
        rollback_build_id,
        agent_id,
        caller_id,
        agent_name: agent_name.clone(),
        target_version: target.version,
        target_image_tag: target.image_tag,
        reason,
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)",
    )
    .bind(agent_id)
    .bind(caller_id)
    .bind(serde_json::to_value(&payload).expect("serialize rollback payload"))
    .execute(&state.db)
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("queue rollback: {e}")).into_response();
    }
    let _ = state.build_tx.send(()).await;

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

pub async fn execute_agent_rollback(
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
    // UUID-keyed (see build_agent_spec) so rollback re-targets the live workload.
    let spec = crate::agents::build_agent_spec(agent_id, &agent_name, target.image_tag.clone(), vec![], secrets, None);

    match state.runtime.deploy(&spec).await {
        Ok(deploy_status) => {
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

            let agent_url = deploy_status.endpoint.unwrap_or_default();
            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', url = $2, version = $3, image = $4, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&agent_url)
            .bind(&target.version)
            .bind(&target.image_tag)
            .execute(db)
            .await;

            // Persist identity + image for guardian/restart (RUN-3), as on update.
            let _ = sqlx::query(
                "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id, k8s_deployment_name, spec_image) \
                 VALUES ($1, $2, 'running', $3, $4, $5)",
            )
            .bind(agent_id)
            .bind(rollback_build_id)
            .bind(caller_id)
            .bind(agent_id.to_string())
            .bind(&target.image_tag)
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
