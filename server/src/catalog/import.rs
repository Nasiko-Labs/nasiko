use axum::{
    Json,
    Router,
    extract::{ Multipart, State },
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use serde::{ Deserialize, Serialize };
use uuid::Uuid;

use crate::auth::Claims;
use crate::build::{ download_repo_tarball, extract_tar_gzip, is_valid_repo_name };
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload", post(import_upload))
        .route("/github", post(import_github))
        .route("/registry", post(import_registry))
}

// ─── Response ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct ImportResult {
    pub(crate) agent_id: Uuid,
    pub(crate) build_id: Option<Uuid>,
    pub(crate) container_name: Option<String>,
    pub(crate) status: String,
}

// ─── Shared Pipeline ────────────────────────────────────────────────────────

pub(crate) struct AgentMetadata {
    name: String,
    display_name: Option<String>,
    description: Option<String>,
    version: String,
    skills: serde_json::Value,
    capabilities: serde_json::Value,
}

pub(crate) fn read_agent_card(dir: &std::path::Path) -> Result<AgentMetadata, String> {
    let card_path = dir.join("AgentCard.json");
    let content = std::fs
        ::read_to_string(&card_path)
        .map_err(|e| format!("cannot read AgentCard.json: {e}"))?;
    let card: serde_json::Value = serde_json
        ::from_str(&content)
        .map_err(|e| format!("invalid AgentCard.json: {e}"))?;

    Ok(AgentMetadata {
        name: card
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("agent")
            .to_string(),
        display_name: card
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from),
        description: card
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from),
        // Normalize a single leading "v" so "v2.0.0" and "2.0.0" compare equal,
        // matching Python's store-time strip (registry_repository.py) and its
        // version-equality/rollback checks (agent_update_service.py).
        version: {
            let raw = card.get("version").and_then(|v| v.as_str()).unwrap_or("1.0.0");
            raw.strip_prefix('v').unwrap_or(raw).to_string()
        },
        skills: card.get("skills").cloned().unwrap_or(serde_json::json!([])),
        capabilities: card
            .get("capabilities")
            .cloned()
            .unwrap_or(
                serde_json::json!({
            "streaming": false,
            "pushNotifications": false,
            "stateTransitionHistory": false,
        })
            ),
    })
}

/// Run a blocking archive/filesystem closure on Tokio's blocking pool so a large
/// zip/tar never stalls an async worker thread. The `JoinError` is flattened into
/// the `String` error channel the import handlers already use.
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| {
            tracing::error!(%e, "run_blocking: background task join failed");
            "background task failed".to_string()
        })?
}

/// Find the caller's own agent by name, if any.
///
/// Scoped to `owner_id` — agent names are only unique **per owner**, per migration
/// 015's partial unique index `(owner_id, name) WHERE deleted_at IS NULL`. Without
/// this scope, importing a name another owner already uses would match THEIR row,
/// and the caller's subsequent `UPDATE` would silently rewrite that owner's agent's
/// version/image, redeploying it under the importer's build (cross-owner takeover).
async fn find_owned_agent<'e, E>(
    executor: E,
    name: &str,
    owner_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT id FROM agents WHERE name = $1 AND owner_id = $2 AND deleted_at IS NULL")
        .bind(name)
        .bind(owner_id)
        .fetch_optional(executor)
        .await
}

pub(crate) async fn build_and_deploy(
    source_dir: &std::path::Path,
    meta: &AgentMetadata,
    owner_id: Uuid,
    state: &AppState
) -> Result<ImportResult, (StatusCode, String)> {
    let image_tag = crate::agents::build_image_tag(&state.config.agent_image_registry, &meta.name, &meta.version);

    // Verify Dockerfile exists
    if !source_dir.join("Dockerfile").exists() {
        return Err((StatusCode::BAD_REQUEST, "no Dockerfile found in source".into()));
    }

    // Register agent in catalog and sync skills projection atomically.
    let mut tx = state.db.begin().await
        .map_err(|e| {
            tracing::error!(%e, agent_name = %meta.name, %owner_id, "build_and_deploy: begin tx");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?;
    // Run the existence check and the update on the SAME transaction as the
    // INSERT/skills/build-record writes — otherwise a later commit failure leaves
    // the agent pointing at a rolled-back build (CAT-1), and two concurrent
    // same-name imports both read None and race.
    let existing_id = find_owned_agent(&mut *tx, &meta.name, owner_id).await
        .map_err(|e| {
            tracing::error!(%e, agent_name = %meta.name, %owner_id, "build_and_deploy: lookup agent");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?;

    let agent_id: Uuid = if let Some(id) = existing_id {
        sqlx::query("UPDATE agents SET version = $1, image = $2, updated_at = now() WHERE id = $3 AND owner_id = $4")
            .bind(&meta.version)
            .bind(&image_tag)
            .bind(id)
            .bind(owner_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                tracing::error!(%e, agent_id = %id, "build_and_deploy: update agent");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            })?;
        id
    } else {
        sqlx::query_scalar(
            r#"INSERT INTO agents (name, display_name, description, owner_id, version, image, skills, capabilities)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING id"#,
        )
        .bind(&meta.name)
        .bind(&meta.display_name)
        .bind(&meta.description)
        .bind(owner_id)
        .bind(&meta.version)
        .bind(&image_tag)
        .bind(&meta.skills)
        .bind(&meta.capabilities)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!(%e, agent_name = %meta.name, %owner_id, "build_and_deploy: register agent");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?
        .ok_or_else(|| (StatusCode::CONFLICT, "agent name already in use by another owner".into()))?
    };

    let skills: Vec<crate::catalog::models::Skill> =
        serde_json::from_value(meta.skills.clone())
            .map_err(|_| (StatusCode::BAD_REQUEST, "skills must be an array of skill objects".into()))?;
    crate::catalog::skills::sync_agent_skills(&mut tx, agent_id, &skills).await
        .map_err(|e| {
            tracing::error!(%e, %agent_id, "build_and_deploy: sync skills");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?;

    // Create the build record inside the same transaction so the agent row
    // and its first build record are always committed together.
    let build_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO agent_builds (agent_id, version_tag, image_reference, status)
           VALUES ($1, $2, $3, 'building')
           RETURNING id"#,
    )
    .bind(agent_id)
    .bind(&meta.version)
    .bind(&image_tag)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!(%e, %agent_id, "build_and_deploy: create build record");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
    })?;

    tx.commit().await
        .map_err(|e| {
            tracing::error!(%e, %agent_id, %build_id, "build_and_deploy: commit tx");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?;

    // Build image
    // TODO: migrate to new runtime API — build() now takes tar bytes, not a directory path.
    // For now, read the directory into a tar archive in-memory.
    let source_dir_owned = source_dir.to_path_buf();
    let tar_bytes = run_blocking(move || crate::build::tar_directory(&source_dir_owned))
        .await
        .map_err(|e| {
            tracing::error!(%e, %agent_id, %build_id, "build_and_deploy: tar source");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
        })?;
    if let Err(e) = state.runtime.build(&tar_bytes, &image_tag).await {
        tracing::error!(%e, %agent_id, %build_id, "build_and_deploy: docker build failed");
        let _ = sqlx
            ::query("UPDATE agent_builds SET status = 'failed' WHERE id = $1")
            .bind(build_id)
            .execute(&state.db).await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string()));
    }

    // Mark build successful
    let _ = sqlx
        ::query("UPDATE agent_builds SET status = 'success', updated_at = now() WHERE id = $1")
        .bind(build_id)
        .execute(&state.db).await;

    // Deploy container — UUID-keyed (see build_agent_spec) so import re-targets the
    // existing workload on re-import and can't collide cross-team on the name.
    let spec = crate::agents::build_agent_spec(
        agent_id,
        &meta.name,
        image_tag,
        vec![],
        std::collections::HashMap::new(),
        None,
    );

    let container_name = match state.runtime.deploy(&spec).await {
        Ok(status) => Some(status.container_id.to_string()),
        Err(e) => {
            tracing::warn!(agent_id = %agent_id, %e, "deploy after build failed");
            None
        }
    };

    Ok(ImportResult {
        agent_id,
        build_id: Some(build_id),
        container_name,
        status: "success".into(),
    })
}

// ─── POST /import/upload ────────────────────────────────────────────────────

async fn import_upload(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
    let mut package_data: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("package") {
            let data = match field.bytes().await {
                Ok(d) if !d.is_empty() => d,
                _ => {
                    continue;
                }
            };
            if data.len() > MAX_UPLOAD_BYTES {
                return (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "upload exceeds 200 MB limit",
                ).into_response();
            }
            package_data = Some(data.to_vec());
        }
    }

    let data = match package_data {
        Some(d) => d,
        None => {
            return (StatusCode::BAD_REQUEST, "no package file provided").into_response();
        }
    };

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-upload-{}", Uuid::new_v4()));

    // Extract + parse on the blocking pool — a 100 MiB zip must not stall a worker.
    let meta = {
        let tmp = tmp_dir.clone();
        match run_blocking(move || {
            crate::build::routes::extract_zip_to_dir(&data, &tmp)?;
            read_agent_card(&tmp)
        })
        .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(%e, %owner_id, "import_upload: invalid package");
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                return (StatusCode::BAD_REQUEST, "invalid package").into_response();
            }
        }
    };

    let result = build_and_deploy(&tmp_dir, &meta, owner_id, &state).await;
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(r) => (StatusCode::CREATED, Json(r)).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

// ─── POST /import/github ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GithubImportRequest {
    repository: String,
}

async fn import_github(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<GithubImportRequest>
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Validate repository name: must be "owner/repo" with safe characters only.
    // Prevents path traversal and ensures git-clone-equivalent safety.
    if !is_valid_repo_name(&req.repository) {
        return (
            StatusCode::BAD_REQUEST,
            "invalid repository format — expected 'owner/repo'",
        ).into_response();
    }

    // Load and decrypt the user's stored GitHub access token.
    let access_token = match crate::github::load_github_token(&state.db, owner_id).await {
        Some(t) => t,
        None => {
            return (
                StatusCode::FORBIDDEN,
                "GitHub not connected — visit /add-agent.html to connect",
            ).into_response();
        }
    };

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-github-{}", Uuid::new_v4()));

    let tarball_bytes = match
        download_repo_tarball(&state.http_client, &access_token, &req.repository).await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(%e, %owner_id, repository = %req.repository, "import_github: download tarball failed");
            return (StatusCode::BAD_GATEWAY, "failed to download repository archive").into_response();
        }
    };

    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        tracing::warn!(%e, "failed to create extraction directory");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    {
        let bytes = tarball_bytes;
        let tmp = tmp_dir.clone();
        if let Err(e) = run_blocking(move || extract_tar_gzip(&bytes, &tmp)).await {
            tracing::warn!(%e, %owner_id, "import_github: failed to extract repository archive");
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            return (
                StatusCode::BAD_REQUEST,
                "failed to extract repository archive",
            ).into_response();
        }
    }
    let actual_root = (
        match tokio::fs::read_dir(&tmp_dir).await {
            Ok(mut rd) =>
                rd
                    .next_entry().await
                    .ok()
                    .flatten()
                    .map(|e| e.path()),
            Err(_) => None,
        }
    ).unwrap_or_else(|| tmp_dir.clone());

    let meta = {
        let root = actual_root.clone();
        match run_blocking(move || read_agent_card(&root)).await {
            Ok(m) => m,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                return (StatusCode::BAD_REQUEST, e).into_response();
            }
        }
    };

    let result = build_and_deploy(&actual_root, &meta, owner_id, &state).await;
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(r) => (StatusCode::CREATED, Json(r)).into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}

// ─── POST /import/registry ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct RegistryImportRequest {
    reference: String,
}

const SOURCE_MEDIA_TYPE: &str = "application/vnd.nasiko.agent.v1.tar+gzip";

fn validate_registry_host(
    host: &str,
    allowed: &[String],
) -> Result<(), (StatusCode, String)> {
    if allowed.is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            "registry import is disabled — set REGISTRY_IMPORT_ALLOWED_HOSTS to enable it".to_string(),
        ));
    }
    // Strip port before comparing (ghcr.io:443 → ghcr.io)
    let host_no_port = host.split(':').next().unwrap_or(host);
    if !allowed.iter().any(|h| h.split(':').next().unwrap_or(h) == host_no_port) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("registry host '{host_no_port}' is not in the allowed list"),
        ));
    }
    Ok(())
}

async fn import_registry(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<RegistryImportRequest>
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Parse OCI reference: "registry.host/owner/name:tag"
    let (repo_with_host, tag) = match req.reference.rsplit_once(':') {
        Some((r, t)) => (r.to_string(), t.to_string()),
        None => (req.reference.clone(), "latest".to_string()),
    };

    // Split host from repo path: "registry.nasiko.dev/nasiko/agent" → ("registry.nasiko.dev", "nasiko/agent")
    let (host, repo) = match repo_with_host.split_once('/') {
        Some((h, r)) => (h, r.to_string()),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid reference: expected registry.host/owner/name[:tag]".to_string(),
            ).into_response();
        }
    };

    if let Err((code, msg)) = validate_registry_host(host, &state.config.registry_import_allowed_hosts) {
        return (code, msg).into_response();
    }

    let registry_url = format!("https://{}", host);

    // Use a no-redirect client for registry fetches: `validate_registry_host`
    // only vets the initial host, so following a 3xx to an internal address would
    // reopen SSRF. With redirects disabled a 3xx fails the `is_success()` guards
    // below and is reported as an error rather than silently followed.
    let registry_client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%e, "import_registry: failed to build http client");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    // Fetch manifest from artifact registry
    let manifest_url = format!("{}/v2/{}/manifests/{}", registry_url, repo, tag);
    let manifest_res = registry_client
        .get(&manifest_url)
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .send().await;

    let manifest_res = match manifest_res {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            return (
                StatusCode::BAD_REQUEST,
                format!("registry returned {status}: {body}"),
            ).into_response();
        }
        Err(e) => {
            tracing::error!(%e, %registry_url, "import_registry: cannot reach registry");
            return (StatusCode::BAD_GATEWAY, "cannot reach registry").into_response();
        }
    };

    let manifest: serde_json::Value = match manifest_res.json().await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, %registry_url, "import_registry: invalid manifest response");
            return (StatusCode::BAD_GATEWAY, "registry returned an invalid manifest").into_response();
        }
    };

    // Check if this is a source artifact or a container image
    let layers = manifest.get("layers").and_then(|l| l.as_array());
    let is_source = layers
        .and_then(|l| l.first())
        .and_then(|layer| layer.get("mediaType"))
        .and_then(|mt| mt.as_str())
        .map(|mt| mt == SOURCE_MEDIA_TYPE)
        .unwrap_or(false);

    if is_source {
        // Source artifact: download, extract, build, deploy
        let blob_digest = match
            layers
                .and_then(|l| l.first())
                .and_then(|layer| layer.get("digest"))
                .and_then(|d| d.as_str())
        {
            Some(d) => d.to_string(),
            None => {
                return (
                    StatusCode::BAD_GATEWAY,
                    "manifest has no layer digest".to_string(),
                ).into_response();
            }
        };

        let blob_url = format!("{}/v2/{}/blobs/{}", registry_url, repo, blob_digest);
        let blob_res = match registry_client.get(&blob_url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("blob fetch failed: {}", r.status()),
                ).into_response();
            }
            Err(e) => {
                tracing::error!(%e, %blob_url, "import_registry: blob fetch error");
                return (StatusCode::BAD_GATEWAY, "failed to fetch registry blob").into_response();
            }
        };

        let blob_data = {
            use futures::StreamExt;
            use bytes::BufMut;
            const MAX_BLOB_BYTES: usize = 100 * 1024 * 1024;
            let mut buf = bytes::BytesMut::with_capacity(64 * 1024);
            let mut stream = blob_res.bytes_stream();
            loop {
                match stream.next().await {
                    None => {
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::error!(%e, %blob_url, "import_registry: blob read error");
                        return (
                            StatusCode::BAD_GATEWAY,
                            "failed to read registry blob",
                        ).into_response();
                    }
                    Some(Ok(chunk)) => {
                        if buf.len() + chunk.len() > MAX_BLOB_BYTES {
                            return (
                                StatusCode::BAD_REQUEST,
                                "registry blob exceeds 100 MB limit",
                            ).into_response();
                        }
                        buf.put(chunk);
                    }
                }
            }
            buf.freeze()
        };

        // Decompress gzip + extract tar + parse on the blocking pool.
        let tmp_dir = std::env::temp_dir().join(format!("nasiko-registry-{}", Uuid::new_v4()));
        let meta = {
            let bytes = blob_data;
            let tmp = tmp_dir.clone();
            match run_blocking(move || {
                extract_tar_gzip(&bytes, &tmp)?;
                read_agent_card(&tmp)
            })
            .await
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(%e, %owner_id, "import_registry: extract source failed");
                    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                    return (StatusCode::BAD_REQUEST, "invalid source artifact").into_response();
                }
            }
        };

        let result = build_and_deploy(&tmp_dir, &meta, owner_id, &state).await;
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

        match result {
            Ok(r) => (StatusCode::CREATED, Json(r)).into_response(),
            Err((code, msg)) => (code, msg).into_response(),
        }
    } else {
        // Container image: pull via docker and deploy directly
        let image_ref = format!(
            "{}/{}",
            registry_url.trim_start_matches("https://").trim_start_matches("http://"),
            repo
        );
        let image_with_tag = format!("{}:{}", image_ref, tag);

        // Use docker pull to fetch the image, bounded by a timeout so a hung/slow
        // registry can't block the handler indefinitely (CAT-5; mirrors the
        // git-clone path which already wraps in tokio::time::timeout).
        const PULL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
        let pull_fut = tokio::process::Command::new("docker")
            .args(["pull", &image_with_tag])
            .output();

        match tokio::time::timeout(PULL_TIMEOUT, pull_fut).await {
            Err(_) => {
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    "docker pull timed out",
                ).into_response();
            }
            Ok(Ok(output)) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::error!(%stderr, %image_with_tag, "import_registry: docker pull failed");
                return (
                    StatusCode::BAD_GATEWAY,
                    "docker pull failed",
                ).into_response();
            }
            Ok(Err(e)) => {
                tracing::error!(%e, "import_registry: docker pull spawn error");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error",
                ).into_response();
            }
            Ok(Ok(_)) => {}
        }

        // Derive agent name from repo
        let agent_name = repo.rsplit('/').next().unwrap_or("agent").to_string();

        // Register agent in catalog — only update if this caller owns the existing entry.
        let agent_id: Uuid = match
            sqlx
                ::query_scalar(
                    // Conflict target is the (owner_id, name) partial unique index
                    // (migration 015); the owner is part of the key, so a conflict
                    // only ever updates the same owner's row (no cross-owner takeover).
                    r#"INSERT INTO agents (name, display_name, owner_id, version, image)
               VALUES ($1, $1, $2, $3, $4)
               ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO UPDATE
                 SET version = EXCLUDED.version, image = EXCLUDED.image, updated_at = now()
               RETURNING id"#
                )
                .bind(&agent_name)
                .bind(owner_id)
                // Store the logical version with a single leading "v" stripped
                // (parity with read_agent_card / Python), while the image ref
                // below keeps the original OCI tag for pulls.
                .bind(tag.strip_prefix('v').unwrap_or(&tag))
                .bind(&image_with_tag)
                .fetch_optional(&state.db).await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                return (
                    StatusCode::CONFLICT,
                    "agent name already in use by another owner",
                ).into_response();
            }
            Err(e) => {
                tracing::error!(%e, %agent_name, %owner_id, "import_registry: register agent");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error",
                ).into_response();
            }
        };

        // Deploy — UUID-keyed (see build_agent_spec).
        let spec = crate::agents::build_agent_spec(
            agent_id,
            &agent_name,
            image_with_tag,
            vec![],
            std::collections::HashMap::new(),
            None,
        );

        let container_name = match state.runtime.deploy(&spec).await {
            Ok(status) => Some(status.container_id.to_string()),
            Err(e) => {
                tracing::warn!(agent_id = %agent_id, %e, "deploy after pull failed");
                None
            }
        };

        (
            StatusCode::CREATED,
            Json(ImportResult {
                agent_id,
                build_id: None,
                container_name,
                status: "success".into(),
            }),
        ).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{find_owned_agent, read_agent_card};

    /// `find_owned_agent` must never see another owner's agent, even when the
    /// name collides — otherwise `build_and_deploy`'s subsequent UPDATE would
    /// silently rewrite a different owner's agent (cross-owner takeover).
    ///
    /// Requires a live Postgres reachable via `DATABASE_URL` (same convention
    /// as `oss/server/tests/*`); skipped if unset so `cargo test --lib` still
    /// runs everywhere else in this module without infra.
    #[tokio::test]
    async fn find_owned_agent_never_matches_a_different_owner() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = sqlx::PgPool::connect(&database_url).await.expect("connect to test DB");

        let owner_a = uuid::Uuid::new_v4();
        let owner_b = uuid::Uuid::new_v4();
        let shared_name = format!("takeover-test-{}", uuid::Uuid::new_v4());

        for owner in [owner_a, owner_b] {
            sqlx::query(
                "INSERT INTO users (id, username, email, is_superuser) VALUES ($1, $2, $3, false)",
            )
            .bind(owner)
            .bind(format!("takeover-test-user-{owner}"))
            .bind(format!("takeover-test-{owner}@example.com"))
            .execute(&pool)
            .await
            .expect("insert test user");
        }

        let agent_a: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO agents (name, owner_id, version, image) VALUES ($1, $2, '1.0.0', 'img:1') RETURNING id",
        )
        .bind(&shared_name)
        .bind(owner_a)
        .fetch_one(&pool)
        .await
        .expect("insert owner A's agent");

        // Owner B imports the SAME name — must find nothing of Owner A's.
        let found_by_b = find_owned_agent(&pool, &shared_name, owner_b).await.unwrap();
        assert_eq!(found_by_b, None, "owner B must not see owner A's agent by name collision");

        // Owner A re-importing the same name must still find their own row.
        let found_by_a = find_owned_agent(&pool, &shared_name, owner_a).await.unwrap();
        assert_eq!(found_by_a, Some(agent_a), "owner A must find their own existing agent");

        let _ = sqlx::query("DELETE FROM agents WHERE id = $1").bind(agent_a).execute(&pool).await;
        for owner in [owner_a, owner_b] {
            let _ = sqlx::query("DELETE FROM users WHERE id = $1").bind(owner).execute(&pool).await;
        }
    }

    fn write_card(dir: &std::path::Path, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let body = format!(
            r#"{{"name":"demo","description":"d","version":"{version}","skills":[]}}"#
        );
        std::fs::write(dir.join("AgentCard.json"), body).unwrap();
    }

    #[test]
    fn strips_single_leading_v_from_version() {
        let dir = std::env::temp_dir().join(format!("nasiko-card-test-{}", uuid::Uuid::new_v4()));
        write_card(&dir, "v2.0.0");
        let meta = read_agent_card(&dir).unwrap();
        assert_eq!(meta.version, "2.0.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_plain_version_untouched() {
        let dir = std::env::temp_dir().join(format!("nasiko-card-test-{}", uuid::Uuid::new_v4()));
        write_card(&dir, "2.0.0");
        let meta = read_agent_card(&dir).unwrap();
        assert_eq!(meta.version, "2.0.0");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
