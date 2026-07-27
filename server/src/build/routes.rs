use std::convert::Infallible;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use uuid::Uuid;

use super::BuildStatus;
use crate::agents::upload::BuildJobPayload;
use crate::agents::utils::set_build_status;
use crate::auth::Claims;
use crate::state::AppState;

// ─── Input validation ────────────────────────────────────────────────────────

/// Validate a git clone URL.
/// Only HTTPS scheme is allowed. Host must be in the provided allowlist.
/// Two-layer: called at HTTP handler (before DB insert) and in execute_build
/// (defence-in-depth for jobs that bypassed the handler via direct DB writes).
pub(crate) fn validate_github_url(url: &str, allowed_hosts: &[String]) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("only https:// URLs are allowed".into());
    }
    let host = parsed.host_str().ok_or_else(|| "missing host".to_string())?;
    if !allowed_hosts.iter().any(|h| h == host) {
        return Err(format!("host '{host}' is not in the allowed list"));
    }
    Ok(())
}

/// Shallow-clone `url` into `dest`. Re-validates against `allowed_hosts` as
/// defence-in-depth (jobs inserted directly into `build_jobs` — admin tooling,
/// migrations — bypass the HTTP handler's own check). Shared by the agent
/// build path (`execute_build`) and the MCP-server-upload build path
/// (`crate::mcp::build::execute_mcp_server_build`) — do not duplicate.
pub(crate) async fn clone_repo(url: &str, allowed_hosts: &[String], dest: &std::path::Path) -> Result<(), String> {
    validate_github_url(url, allowed_hosts).map_err(|e| format!("invalid github_url: {e}"))?;

    tokio::fs::create_dir_all(dest).await.map_err(|e| format!("create tmp dir: {e}"))?;

    // dest.to_str() fails only on non-UTF8 paths (exotic OS configs). Falling
    // back to "." is dangerous: tar_directory would then package the server's
    // working directory (binaries, env files) into the build context.
    let dest_str = dest.to_str().ok_or_else(|| "temp path contains non-UTF8 characters".to_string())?;

    let clone_status = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new("git")
            // Restrict git to HTTPS only — prevents protocol-redirect attacks.
            .env("GIT_ALLOW_PROTOCOL", "https")
            // Prevent git from blocking the worker waiting for terminal credentials.
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["clone", "--depth=1", url, dest_str])
            .status(),
    )
    .await
    .map_err(|_| "git clone timed out after 5 minutes".to_string())?
    .map_err(|e| format!("git clone: {e}"))?;

    if !clone_status.success() {
        return Err(format!("git clone failed with exit code {}", clone_status.code().unwrap_or(1)));
    }
    Ok(())
}

/// Download a GitHub repo as a tarball via the API (no `git` binary needed).
/// Uses `token` for private repos; public repos work without one.
/// URL format: `https://github.com/{owner}/{repo}.git` or `https://github.com/{owner}/{repo}`
pub(crate) async fn download_github_tarball(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    token: Option<&str>,
) -> Result<(), String> {
    // Parse owner/repo from the URL.
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let segments: Vec<&str> = parsed.path().trim_matches('/').split('/').collect();
    if segments.len() < 2 {
        return Err("expected github.com/{owner}/{repo} URL".into());
    }
    let owner = segments[0];
    let repo = segments[1].trim_end_matches(".git");

    // GitHub's tarball endpoint (public repos, no auth needed).
    let tarball_url = format!("https://api.github.com/repos/{owner}/{repo}/tarball");

    let mut req = client
        .get(&tarball_url)
        .header("User-Agent", "nasiko-server")
        .timeout(Duration::from_secs(300));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("github tarball download: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "github tarball download failed: HTTP {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("github tarball read: {e}"))?;

    tokio::fs::create_dir_all(dest)
        .await
        .map_err(|e| format!("create tmp dir: {e}"))?;

    // Extract the tarball. GitHub tarballs have a top-level directory
    // ({owner}-{repo}-{sha}/), so we strip the first path component.
    let dest_path = dest.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().map_err(|e| format!("tar entries: {e}"))? {
            let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;
            let path = entry.path().map_err(|e| format!("tar path: {e}"))?.into_owned();
            // Strip the top-level directory (e.g. "owner-repo-sha7/src/..." → "src/...")
            let stripped: std::path::PathBuf = path.components().skip(1).collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            let full_path = dest_path.join(&stripped);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
            }
            entry
                .unpack(&full_path)
                .map_err(|e| format!("unpack {}: {e}", stripped.display()))?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("spawn_blocking tar extract: {e}"))??;

    Ok(())
}

/// Validate an OCI image tag segment.
/// OCI spec: tag must start with [a-zA-Z0-9_] and contain only [a-zA-Z0-9._-].
pub(crate) fn validate_version_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag.len() > 128 {
        return Err("version_tag must be 1–128 characters".into());
    }
    let mut chars = tag.chars();
    let first = chars.next().unwrap(); // non-empty guaranteed above
    if !first.is_ascii_alphanumeric() && first != '_' {
        return Err("version_tag must start with [a-zA-Z0-9_]".into());
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
        return Err("version_tag may only contain [a-zA-Z0-9._-]".into());
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/builds", post(create_build))
}

/// The GET routes here are mounted separately from `router()`'s
/// `require_deployer`-gated mutation, under `require_auth` only — each
/// handler checks `can_deploy` itself and returns `crate::unavailable()`
/// (200) instead of what would otherwise be a blanket 403, replicating
/// exactly who could see this data before (deployer+, same scoping the
/// queries already apply for non-superusers), just without an error status.
pub fn degradable_router() -> Router<AppState> {
    Router::new()
        .route("/builds", get(list_all_builds))
        .route("/builds/{id}", get(get_build))
        .route("/builds/{id}/progress", get(build_progress_sse))
        .route("/builds/{id}/logs", get(get_build_logs))
        .route("/builds/agent/{agent_id}", get(list_builds))
}

// ─── Models ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct BuildRecord {
    id: Uuid,
    agent_id: Uuid,
    github_url: Option<String>,
    commit_hash: Option<String>,
    version_tag: String,
    image_reference: String,
    status: BuildStatus,
    logs_url: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn create_build(
    State(state): State<AppState>,
    claims: Claims,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let mut agent_id: Option<Uuid> = None;
    let mut version_tag: Option<String> = None;
    let mut github_url: Option<String> = None;
    let mut source_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().map(|n| n.to_string()).unwrap_or_default();
        match name.as_str() {
            "agent_id" => {
                let text: String = field.text().await.unwrap_or_default();
                agent_id = text.parse().ok();
            }
            "version_tag" => {
                let text: String = field.text().await.unwrap_or_default();
                version_tag = Some(text);
            }
            "github_url" => {
                let text: String = field.text().await.unwrap_or_default();
                github_url = Some(text);
            }
            "source" => {
                let data: bytes::Bytes = field.bytes().await.unwrap_or_default();
                if !data.is_empty() {
                    source_data = Some(data.to_vec());
                }
            }
            _ => {}
        }
    }

    let agent_id = match agent_id {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "agent_id is required").into_response(),
    };
    let version_tag = match version_tag {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "version_tag is required").into_response(),
    };

    // Verify agent exists and caller owns it
    let agent_name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM agents WHERE id = $1 AND owner_id = $2",
    )
    .bind(agent_id)
    .bind(owner_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let agent_name = match agent_name {
        Some(name) => name,
        None => return (StatusCode::NOT_FOUND, "agent not found or not owned by you").into_response(),
    };

    // Validate user-supplied inputs before any side effects.
    if let Err(e) = validate_version_tag(&version_tag) {
        return (StatusCode::UNPROCESSABLE_ENTITY, e).into_response();
    }
    if let Some(ref url) = github_url
        && let Err(e) = validate_github_url(url, &state.config.git_clone_allowed_hosts)
    {
        return (StatusCode::UNPROCESSABLE_ENTITY, e).into_response();
    }

    let image_reference = format!("{agent_name}:{version_tag}");

    // If source ZIP provided, upload to S3
    let source_key = if let Some(data) = &source_data {
        let key = format!("sources/{agent_id}/{version_tag}.zip");
        if let Err(e) = state.oci_storage.put_blob(&key, bytes::Bytes::from(data.clone())).await {
            tracing::error!(%e, %agent_id, %key, "create_build: failed to upload source");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
        Some(key)
    } else {
        None
    };

    // Create build record
    let result = sqlx::query_as::<_, BuildRecord>(
        r#"INSERT INTO agent_builds (agent_id, github_url, version_tag, image_reference, status)
           VALUES ($1, $2, $3, $4, 'queued')
           RETURNING *"#,
    )
    .bind(agent_id)
    .bind(&github_url)
    .bind(&version_tag)
    .bind(&image_reference)
    .fetch_one(&state.db)
    .await;

    let build = match result {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(%e, %agent_id, "create_build: failed to create build record");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    let payload = BuildJobPayload::StandaloneBuild {
        build_id: build.id,
        agent_id,
        agent_name,
        github_url,
        source_key,
        version_tag,
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO build_jobs (agent_id, owner_id, payload) VALUES ($1, $2, $3)",
    )
    .bind(agent_id)
    .bind(owner_id)
    .bind(serde_json::to_value(&payload).expect("serialize build payload"))
    .execute(&state.db)
    .await
    {
        tracing::error!(%e, %agent_id, build_id = %build.id, "create_build: failed to queue build job");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }
    let _ = state.build_tx.send(()).await;

    (StatusCode::CREATED, Json(build)).into_response()
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_build(
    runtime: std::sync::Arc<dyn nasiko_runtime::ContainerRuntime>,
    db: sqlx::PgPool,
    build_id: Uuid,
    agent_name: String,
    github_url: Option<String>,
    source_key: Option<String>,
    version_tag: String,
    oci_storage: nasiko_oci::storage::S3Storage,
    http_client: reqwest::Client,
    allowed_hosts: Vec<String>,
    capability_generator_model: String,
) {
    set_build_status(&db, build_id, BuildStatus::Building).await;

    let image_tag = format!("{agent_name}:{version_tag}");
    let tmp_dir = std::env::temp_dir().join(format!("nasiko-build-{build_id}"));

    let result: Result<(), String> = async {
        // Acquire source into tmp_dir
        if let Some(ref url) = github_url {
            clone_repo(url, &allowed_hosts, &tmp_dir).await?;
        } else if let Some(key) = &source_key {
            let data = oci_storage.get_blob(key).await
                .map_err(|e| format!("fetch source from S3: {e}"))?;
            extract_zip_to_dir(&data, &tmp_dir).map_err(|e| format!("extract zip: {e}"))?;
        } else {
            return Err("no source provided (neither github_url nor source zip)".into());
        }

        // Verify Dockerfile exists
        let dockerfile_path = tmp_dir.join("Dockerfile");
        if !dockerfile_path.exists() {
            return Err("no Dockerfile found in source".into());
        }

        // Patch Dockerfile to inject OTel auto-instrumentation (zero-code change for the agent).
        let original = tokio::fs::read_to_string(&dockerfile_path)
            .await
            .map_err(|e| format!("read Dockerfile: {e}"))?;
        let patched = nasiko_observability::patch_dockerfile_for_otel(&original);
        if patched != original {
            tokio::fs::write(&dockerfile_path, &patched).await
                .map_err(|e| format!("write patched Dockerfile: {e}"))?;
            tracing::info!(build_id = %build_id, "patched Dockerfile with OTel instrumentation");

            // Write the Python sitecustomize file into the build context so the
            // COPY instruction in the patched Dockerfile can include it in the image.
            // This file wraps AgentExecutor.execute() to set session.id on every
            // request span — the key attribute for session grouping in the dashboard.
            nasiko_observability::write_otel_patch_file(&tmp_dir)
                .map_err(|e| format!("write OTel patch file to build context: {e}"))?;
            tracing::info!(build_id = %build_id, "wrote .nasiko_otel_patch.py to build context");
        }

        // Build image
        let tar_bytes = crate::build::tar_directory(&tmp_dir)
            .map_err(|e| format!("tar source: {e}"))?;
        runtime.build(&tar_bytes, &image_tag).await
            .map_err(|e| format!("docker build: {e}"))?;

        Ok(())
    }.await;

    // Clean up temp dir
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(()) => {
            set_build_status(&db, build_id, BuildStatus::Success).await;

            if let Err(e) = sqlx::query(
                r#"INSERT INTO agent_versions (agent_id, build_id, version, image_tag, is_active)
                   SELECT agent_id, $1, version_tag, image_reference, false
                   FROM agent_builds WHERE id = $1"#,
            )
            .bind(build_id)
            .execute(&db)
            .await
            {
                tracing::error!(build_id = %build_id, %e, "failed to create agent version");
            }

            if let Some(ref key) = source_key {
                auto_generate_capabilities_pub(
                    &db, &oci_storage, &http_client, key, &agent_name, &capability_generator_model,
                ).await;
            }
        }
        Err(e) => {
            tracing::error!(build_id = %build_id, %e, "build failed");
            set_build_status(&db, build_id, BuildStatus::Failed).await;
        }
    }
}

// Zip-slip/zip-bomb-safe extraction lives in `nasiko_utils::zip` — moved out of
// this crate so `oss/mcp-gateway` (which cannot depend on `oss/server`) can reuse
// the exact same logic for MCP-server-upload validation instead of duplicating it.
pub use nasiko_utils::zip::{extract_zip_from_file, extract_zip_to_dir};

// ─── GET /builds/{id} ───────────────────────────────────────────────────────

async fn get_build(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    match sqlx::query_as::<_, BuildRecord>("SELECT * FROM agent_builds WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(build)) => Json(build).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, build_id = %id, "get_build: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── GET /builds/{id}/progress (SSE) ────────────────────────────────────────

async fn build_progress_sse(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let db = state.db.clone();

    let stream = async_stream::stream! {
        let mut last_status: Option<BuildStatus> = None;

        loop {
            let record: Option<BuildRecord> = sqlx::query_as(
                "SELECT * FROM agent_builds WHERE id = $1"
            )
            .bind(id)
            .fetch_optional(&db)
            .await
            .ok()
            .flatten();

            let Some(record) = record else {
                yield Ok::<_, Infallible>(Event::default().data(
                    serde_json::json!({"status": "not_found"}).to_string()
                ));
                break;
            };

            if Some(record.status) != last_status {
                last_status = Some(record.status);
                yield Ok(Event::default().data(
                    serde_json::json!({
                        "status": record.status,
                        "build_id": id,
                    }).to_string()
                ));
            }

            if matches!(record.status, BuildStatus::Success | BuildStatus::Failed) {
                break;
            }

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

// ─── GET /builds/{id}/logs ──────────────────────────────────────────────────

async fn get_build_logs(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let url: Option<String> =
        sqlx::query_scalar("SELECT logs_url FROM agent_builds WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    match url {
        Some(u) => u.into_response(),
        None => (StatusCode::NOT_FOUND, "no logs available").into_response(),
    }
}

// ─── GET /builds (list all) ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct ListAllQuery {
    #[serde(default = "default_list_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    status: Option<String>,
    q: Option<String>,
}
fn default_list_limit() -> i64 { 20 }

async fn list_all_builds(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListAllQuery>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Superusers see all builds; others see only builds for their own agents
    let rows = if claims.is_superuser {
        sqlx::query_as::<_, BuildRecord>(
            r#"SELECT b.* FROM agent_builds b
               LEFT JOIN agents a ON a.id = b.agent_id
               WHERE ($1::text IS NULL OR b.status::text = $1)
                 AND ($2::text IS NULL OR a.name ILIKE '%' || $2 || '%' OR b.version_tag ILIKE '%' || $2 || '%')
               ORDER BY b.created_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(&q.status)
        .bind(&q.q)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, BuildRecord>(
            r#"SELECT b.* FROM agent_builds b
               JOIN agents a ON a.id = b.agent_id
               WHERE a.owner_id = $5
                 AND ($1::text IS NULL OR b.status::text = $1)
                 AND ($2::text IS NULL OR a.name ILIKE '%' || $2 || '%' OR b.version_tag ILIKE '%' || $2 || '%')
               ORDER BY b.created_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(&q.status)
        .bind(&q.q)
        .bind(q.limit)
        .bind(q.offset)
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(builds) => Json(crate::Paginated::new(builds)).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_all_builds: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── GET /builds/agent/{agent_id} ───────────────────────────────────────────

async fn list_builds(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Superusers see all; others must own the agent.
    if !claims.is_superuser {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND owner_id = $2)",
        )
        .bind(agent_id)
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

        if !owned {
            return crate::unavailable();
        }
    }

    match sqlx::query_as::<_, BuildRecord>(
        "SELECT * FROM agent_builds WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 20",
    )
    .bind(agent_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(builds) => Json(builds).into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "list_builds: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── Auto-generate capabilities after build ─────────────────────────────────

pub async fn auto_generate_capabilities_pub(
    db: &sqlx::PgPool,
    oci_storage: &nasiko_oci::storage::S3Storage,
    http_client: &reqwest::Client,
    source_key: &str,
    agent_name: &str,
    capability_generator_model: &str,
) {
    use crate::capabilities::generator::CapabilityGenerator;
    use nasiko_orchestrator::providers::LLMProvider;

    let data = match oci_storage.get_blob(source_key).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("capability gen: failed to fetch source {source_key}: {e}");
            return;
        }
    };

    let source = match extract_source_text(&data) {
        Some(s) => s,
        None => return,
    };

    let provider = LLMProvider::from_env(http_client.clone());
    // Caller passes `state.config.capability_generator_model` (already
    // resolved from `CAPABILITY_GENERATOR_MODEL`, default "gpt-4o-mini") —
    // see `capabilities/routes.rs::make_generator` for the same fix; this
    // function has no `AppState` of its own so the resolved value is threaded
    // through `execute_build` instead of re-reading the env var here with a
    // hardcoded placeholder.
    let generator = CapabilityGenerator::new(provider, capability_generator_model.to_string());

    match generator.generate(&source, agent_name).await {
        Ok((card, _)) => {
            let skills_json = serde_json::to_value(&card.skills).unwrap_or_default();
            let caps_json = serde_json::to_value(&card.capabilities).unwrap_or_default();
            let input_modes = serde_json::to_value(&card.default_input_modes).unwrap_or_default();
            let output_modes = serde_json::to_value(&card.default_output_modes).unwrap_or_default();

            let updated_ids: Vec<uuid::Uuid> = match sqlx::query_scalar::<_, uuid::Uuid>(
                r#"UPDATE agents
                   SET description = COALESCE(NULLIF(description, ''), $2),
                       skills = $3,
                       tags = $4,
                       capabilities = $5,
                       default_input_modes = $6,
                       default_output_modes = $7,
                       updated_at = now()
                   WHERE name = $1
                   RETURNING id"#,
            )
            .bind(agent_name)
            .bind(&card.description)
            .bind(&skills_json)
            .bind(&card.tags)
            .bind(&caps_json)
            .bind(&input_modes)
            .bind(&output_modes)
            .fetch_all(db)
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!("capability gen: failed to update agent '{agent_name}': {e}");
                    return;
                }
            };

            // Keep the normalized agent_skills projection in sync (best-effort).
            // agents.name is non-unique (migration 006); warn if more than one matched.
            if updated_ids.len() > 1 {
                tracing::warn!(
                    "capability gen: name '{agent_name}' matched {} agents — capabilities overwritten for all",
                    updated_ids.len()
                );
            }
            for id in updated_ids {
                crate::catalog::skills::sync_agent_skills_json(db, id, &skills_json).await;
            }

            tracing::info!("capability gen: updated card for agent '{agent_name}'");
        }
        Err(e) => {
            tracing::warn!("capability gen: LLM generation failed for '{agent_name}': {e}");
        }
    }
}

fn extract_source_text(data: &[u8]) -> Option<String> {
    use std::io::Read;

    let cursor = std::io::Cursor::new(data);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return None,
    };

    let code_extensions = [
        "py", "rs", "ts", "js", "go", "java", "rb", "ex", "exs", "toml", "yaml", "yml", "json",
        "md", "txt", "dockerfile", "sh",
    ];

    let mut combined = String::new();
    for i in 0..archive.len() {
        let file = match archive.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if file.is_dir() || file.size() > 50_000 {
            continue;
        }
        let name = file.name().to_string();
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if !code_extensions.contains(&ext.as_str()) && !name.to_lowercase().contains("dockerfile") {
            continue;
        }
        // `file.size()` above is the DECLARED size from the zip's central
        // directory — a crafted entry can under-report it while its deflate
        // stream actually decompresses to far more (zip bomb). Bound the
        // real read too, not just the declared-size check.
        let mut contents = String::new();
        if file.take(50_000).read_to_string(&mut contents).is_ok() {
            combined.push_str(&format!("\n--- {name} ---\n"));
            combined.push_str(&contents);
        }
    }

    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}

#[cfg(test)]
mod extract_source_text_tests {
    use super::extract_source_text;
    use std::io::Write;

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(content).unwrap();
        }
        zw.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn reads_small_source_file_content_in_full() {
        let zip = make_zip(&[("main.py", b"print('hello')\n")]);
        let text = extract_source_text(&zip).expect("should extract source text");
        assert!(text.contains("print('hello')"), "small file content must round-trip untruncated");
    }

    #[test]
    fn ignores_non_code_extensions() {
        let zip = make_zip(&[("data.bin", b"\x00\x01\x02\x03")]);
        assert!(extract_source_text(&zip).is_none(), "non-code extensions should be skipped");
    }

    #[test]
    fn reads_a_file_exactly_at_the_size_cap_in_full() {
        // The declared-size guard is `file.size() > 50_000` (strictly greater),
        // so a 50,000-byte file passes it and reaches the `.take(50_000)` read.
        // This proves that bound doesn't off-by-one truncate legitimate content
        // sitting right at the boundary. Uses 'q' as filler (not present in the
        // filename or the "--- name ---" header) so a plain byte-count of the
        // combined output isn't polluted by boilerplate text.
        let content = vec![b'q'; 50_000];
        let zip = make_zip(&[("boundary.md", &content)]);
        let text = extract_source_text(&zip).expect("should extract source text");
        let q_count = text.bytes().filter(|&b| b == b'q').count();
        assert_eq!(q_count, 50_000, "a file exactly at the cap must be read in full, not truncated early");
    }
}
