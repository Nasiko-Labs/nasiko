use axum::{
    Json, Router,
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::build::{download_repo_tarball, extract_tar_gzip, is_valid_repo_name};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload", post(import_upload))
        .route("/github", post(import_github))
        .route("/registry", post(import_registry))
}

// ─── Response ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ImportResult {
    agent_id: Uuid,
    build_id: Option<Uuid>,
    container_name: Option<String>,
    status: String,
}

// ─── Shared Pipeline ────────────────────────────────────────────────────────

struct AgentMetadata {
    name: String,
    display_name: Option<String>,
    description: Option<String>,
    version: String,
    skills: serde_json::Value,
    capabilities: serde_json::Value,
}

fn read_agent_card(dir: &std::path::Path) -> Result<AgentMetadata, String> {
    let card_path = dir.join("AgentCard.json");
    let content = std::fs::read_to_string(&card_path)
        .map_err(|e| format!("cannot read AgentCard.json: {e}"))?;
    let card: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("invalid AgentCard.json: {e}"))?;

    Ok(AgentMetadata {
        name: card.get("name").and_then(|v| v.as_str()).unwrap_or("agent").to_string(),
        display_name: card.get("name").and_then(|v| v.as_str()).map(String::from),
        description: card.get("description").and_then(|v| v.as_str()).map(String::from),
        version: card.get("version").and_then(|v| v.as_str()).unwrap_or("1.0.0").to_string(),
        skills: card.get("skills").cloned().unwrap_or(serde_json::json!([])),
        capabilities: card.get("capabilities").cloned().unwrap_or(serde_json::json!({
            "streaming": false,
            "pushNotifications": false,
            "stateTransitionHistory": false,
        })),
    })
}

async fn build_and_deploy(
    source_dir: &std::path::Path,
    meta: &AgentMetadata,
    owner_id: Uuid,
    state: &AppState,
) -> Result<ImportResult, (StatusCode, String)> {
    let image_tag = format!("{}:{}", meta.name, meta.version);

    // Verify Dockerfile exists
    if !source_dir.join("Dockerfile").exists() {
        return Err((StatusCode::BAD_REQUEST, "no Dockerfile found in source".into()));
    }

    // Register agent in catalog and sync skills projection atomically.
    let mut tx = state.db.begin().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("begin tx: {e}")))?;
    let existing_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM agents WHERE name = $1")
        .bind(&meta.name)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("lookup agent: {e}")))?;

    let agent_id: Uuid = if let Some(id) = existing_id {
        sqlx::query("UPDATE agents SET version = $1, image = $2, updated_at = now() WHERE id = $3")
            .bind(&meta.version)
            .bind(&image_tag)
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("update agent: {e}")))?;
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("register agent: {e}")))?
        .ok_or_else(|| (StatusCode::CONFLICT, "agent name already in use by another owner".into()))?
    };


    let skills: Vec<crate::catalog::models::Skill> =
        serde_json::from_value(meta.skills.clone())
            .map_err(|_| (StatusCode::BAD_REQUEST, "skills must be an array of skill objects".into()))?;
    crate::catalog::skills::sync_agent_skills(&mut tx, agent_id, &skills).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("sync skills: {e}")))?;
    // FIX: create the build record inside the same transaction so the agent row
    // and its first build record are always committed together.

    // Create build record
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
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("create build record: {e}")))?;

    tx.commit().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("commit: {e}")))?;

    // Build image
    // TODO: migrate to new runtime API — build() now takes tar bytes, not a directory path.
    // For now, read the directory into a tar archive in-memory.
    let tar_bytes = crate::build::tar_directory(source_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tar source: {e}")))?;
    if let Err(e) = state.runtime.build(&tar_bytes, &image_tag).await {
        let _ = sqlx::query("UPDATE agent_builds SET status = 'failed' WHERE id = $1")
            .bind(build_id)
            .execute(&state.db)
            .await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("docker build failed: {e}")));
    }

    // Mark build successful
    let _ = sqlx::query("UPDATE agent_builds SET status = 'success', updated_at = now() WHERE id = $1")
        .bind(build_id)
        .execute(&state.db)
        .await;

    // Deploy container
    let spec = nasiko_runtime::DeploymentSpec {
        container_id: nasiko_runtime::ContainerId::new(meta.name.clone()),
        name: meta.name.clone(),
        image: image_tag,
        min_replicas: 1,
        max_replicas: 1,
        env_vars: std::collections::HashMap::new(),
        ports: vec![8000],
        resources: None,
    };

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
    mut multipart: Multipart,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;
    let mut package_data: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("package") {
            let data = match field.bytes().await {
                Ok(d) if !d.is_empty() => d,
                _ => continue,
            };
            if data.len() > MAX_UPLOAD_BYTES {
                return (StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds 200 MB limit").into_response();
            }
            package_data = Some(data.to_vec());
        }
    }

    let data = match package_data {
        Some(d) => d,
        None => return (StatusCode::BAD_REQUEST, "no package file provided").into_response(),
    };

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-upload-{}", Uuid::new_v4()));

    if let Err(e) = crate::build::routes::extract_zip_to_dir(&data, &tmp_dir) {
        return (StatusCode::BAD_REQUEST, format!("invalid zip: {e}")).into_response();
    }

    let meta = match read_agent_card(&tmp_dir) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
    };

    let result = build_and_deploy(&tmp_dir, &meta, owner_id, &state).await;
    let _ = std::fs::remove_dir_all(&tmp_dir);

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
    Json(req): Json<GithubImportRequest>,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Validate repository name: must be "owner/repo" with safe characters only.
    // Prevents path traversal and ensures git-clone-equivalent safety.
    if !is_valid_repo_name(&req.repository) {
        return (StatusCode::BAD_REQUEST, "invalid repository format — expected 'owner/repo'").into_response();
    }

    // Load and decrypt the user's stored GitHub access token.
    let access_token = match crate::github::load_github_token(&state.db, owner_id).await {
        Some(t) => t,
        None => return (StatusCode::FORBIDDEN, "GitHub not connected — visit /add-agent.html to connect").into_response(),
    };

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-github-{}", Uuid::new_v4()));

    let tarball_bytes = match download_repo_tarball(&state.http_client, &access_token, &req.repository).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };

    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        tracing::warn!(%e, "failed to create extraction directory");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Err(e) = extract_tar_gzip(&tarball_bytes, &tmp_dir) {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return (StatusCode::BAD_REQUEST, format!("failed to extract repository archive: {e}")).into_response();
    }
    let actual_root = match tokio::fs::read_dir(&tmp_dir).await {
        Ok(mut rd) => rd.next_entry().await.ok().flatten().map(|e| e.path()),
        Err(_) => None,
    }
    .unwrap_or_else(|| tmp_dir.clone());

    let meta = match read_agent_card(&actual_root) {
        Ok(m) => m,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            return (StatusCode::BAD_REQUEST, e).into_response();
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

async fn import_registry(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<RegistryImportRequest>,
) -> impl IntoResponse {
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Parse OCI reference: "registry.host/owner/name:tag"
    let (repo_with_host, tag) = match req.reference.rsplit_once(':') {
        Some((r, t)) => (r.to_string(), t.to_string()),
        None => (req.reference.clone(), "latest".to_string()),
    };

    // Split host from repo path: "registry.nasiko.dev/nasiko/agent" → ("registry.nasiko.dev", "nasiko/agent")
    let (host, repo) = match repo_with_host.split_once('/') {
        Some((h, r)) => (h, r.to_string()),
        None => return (StatusCode::BAD_REQUEST, "invalid reference: expected registry.host/owner/name[:tag]".to_string()).into_response(),
    };

    let registry_url = format!("https://{}", host);

    // Fetch manifest from artifact registry
    let manifest_url = format!("{}/v2/{}/manifests/{}", registry_url, repo, tag);
    let manifest_res = state.http_client
        .get(&manifest_url)
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .send()
        .await;

    let manifest_res = match manifest_res {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            return (StatusCode::BAD_REQUEST, format!("registry returned {status}: {body}")).into_response();
        }
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("cannot reach registry: {e}")).into_response(),
    };

    let manifest: serde_json::Value = match manifest_res.json().await {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("invalid manifest: {e}")).into_response(),
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
        let blob_digest = match layers
            .and_then(|l| l.first())
            .and_then(|layer| layer.get("digest"))
            .and_then(|d| d.as_str())
        {
            Some(d) => d.to_string(),
            None => return (StatusCode::BAD_GATEWAY, "manifest has no layer digest".to_string()).into_response(),
        };

        let blob_url = format!("{}/v2/{}/blobs/{}", registry_url, repo, blob_digest);
        let blob_res = match state.http_client.get(&blob_url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => return (StatusCode::BAD_GATEWAY, format!("blob fetch failed: {}", r.status())).into_response(),
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("blob fetch error: {e}")).into_response(),
        };

        let blob_data = {
            use futures::StreamExt;
            use bytes::BufMut;
            const MAX_BLOB_BYTES: usize = 100 * 1024 * 1024;
            let mut buf = bytes::BytesMut::with_capacity(64 * 1024);
            let mut stream = blob_res.bytes_stream();
            loop {
                match stream.next().await {
                    None => break,
                    Some(Err(e)) => return (StatusCode::BAD_GATEWAY, format!("blob read error: {e}")).into_response(),
                    Some(Ok(chunk)) => {
                        if buf.len() + chunk.len() > MAX_BLOB_BYTES {
                            return (StatusCode::BAD_REQUEST, "registry blob exceeds 100 MB limit").into_response();
                        }
                        buf.put(chunk);
                    }
                }
            }
            buf.freeze()
        };

        // Decompress gzip then extract tar
        let tmp_dir = std::env::temp_dir().join(format!("nasiko-registry-{}", Uuid::new_v4()));
        if let Err(e) = extract_tar_gzip(&blob_data, &tmp_dir) {
            return (StatusCode::BAD_GATEWAY, format!("extract source: {e}")).into_response();
        }

        let meta = match read_agent_card(&tmp_dir) {
            Ok(m) => m,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                return (StatusCode::BAD_REQUEST, e).into_response();
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
        let image_ref = format!("{}/{}", registry_url.trim_start_matches("https://").trim_start_matches("http://"), repo);
        let image_with_tag = format!("{}:{}", image_ref, tag);

        // Use docker pull to fetch the image
        let pull_result = tokio::process::Command::new("docker")
            .args(["pull", &image_with_tag])
            .output()
            .await;

        match pull_result {
            Ok(output) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return (StatusCode::BAD_GATEWAY, format!("docker pull failed: {stderr}")).into_response();
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("docker pull error: {e}")).into_response();
            }
            _ => {}
        }

        // Derive agent name from repo
        let agent_name = repo.rsplit('/').next().unwrap_or("agent").to_string();

        // Register agent in catalog — only update if this caller owns the existing entry.
        let agent_id: Uuid = match sqlx::query_scalar(
            r#"INSERT INTO agents (name, display_name, owner_id, version, image)
               VALUES ($1, $1, $2, $3, $4)
               ON CONFLICT (name) DO UPDATE
                 SET version = EXCLUDED.version, image = EXCLUDED.image, updated_at = now()
                 WHERE agents.owner_id = EXCLUDED.owner_id
               RETURNING id"#,
        )
        .bind(&agent_name)
        .bind(owner_id)
        .bind(&tag)
        .bind(&image_with_tag)
        .fetch_optional(&state.db)
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => return (StatusCode::CONFLICT, "agent name already in use by another owner").into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("register agent: {e}")).into_response(),
        };

        // Deploy
        let spec = nasiko_runtime::DeploymentSpec {
            container_id: nasiko_runtime::ContainerId::new(agent_name.clone()),
            name: agent_name,
            image: image_with_tag,
            min_replicas: 1,
            max_replicas: 1,
            env_vars: std::collections::HashMap::new(),
            ports: vec![8000],
            resources: None,
        };

        let container_name = match state.runtime.deploy(&spec).await {
            Ok(status) => Some(status.container_id.to_string()),
            Err(e) => {
                tracing::warn!(agent_id = %agent_id, %e, "deploy after pull failed");
                None
            }
        };

        (StatusCode::CREATED, Json(ImportResult {
            agent_id,
            build_id: None,
            container_name,
            status: "success".into(),
        })).into_response()
    }
}

