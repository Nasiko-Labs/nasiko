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

/// Validate an OCI image tag segment.
/// OCI spec: tag must start with [a-zA-Z0-9_] and contain only [a-zA-Z0-9._-].
fn validate_version_tag(tag: &str) -> Result<(), String> {
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
    Router::new()
        .route("/builds", post(create_build).get(list_all_builds))
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
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to upload source: {e}")).into_response();
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
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("queue build: {e}")).into_response();
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
) {
    set_build_status(&db, build_id, BuildStatus::Building).await;

    let image_tag = format!("{agent_name}:{version_tag}");
    let tmp_dir = std::env::temp_dir().join(format!("nasiko-build-{build_id}"));

    let result = async {
        // Acquire source into tmp_dir
        if let Some(ref url) = github_url {
            // Defence-in-depth: re-validate even if the HTTP handler already checked.
            // Jobs inserted directly into build_jobs (e.g. admin tooling, migrations)
            // bypass the handler — this ensures the subprocess never runs an arbitrary URL.
            validate_github_url(url, &allowed_hosts)
                .map_err(|e| format!("invalid github_url: {e}"))?;

            tokio::fs::create_dir_all(&tmp_dir).await
                .map_err(|e| format!("create tmp dir: {e}"))?;

            // tmp_dir.to_str() fails only on non-UTF8 paths (exotic OS configs).
            // Falling back to "." is dangerous: tar_directory would then package the
            // server's working directory (binaries, env files) into the build context.
            let tmp_path = tmp_dir.to_str()
                .ok_or_else(|| "temp path contains non-UTF8 characters".to_string())?;

            let clone_status = tokio::time::timeout(
                Duration::from_secs(300),
                tokio::process::Command::new("git")
                    // Restrict git to HTTPS only — prevents protocol-redirect attacks.
                    .env("GIT_ALLOW_PROTOCOL", "https")
                    // Prevent git from blocking the worker waiting for terminal credentials.
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .args(["clone", "--depth=1", url, tmp_path])
                    .status(),
            )
            .await
            .map_err(|_| "git clone timed out after 5 minutes".to_string())?
            .map_err(|e| format!("git clone: {e}"))?;

            if !clone_status.success() {
                return Err(format!("git clone failed with exit code {}", clone_status.code().unwrap_or(1)));
            }
        } else if let Some(key) = &source_key {
            let data = oci_storage.get_blob(key).await
                .map_err(|e| format!("fetch source from S3: {e}"))?;
            extract_zip_to_dir(&data, &tmp_dir)
                .map_err(|e| format!("extract zip: {e}"))?;
        } else {
            return Err("no source provided (neither github_url nor source zip)".into());
        }

        // Verify Dockerfile exists
        let dockerfile_path = tmp_dir.join("Dockerfile");
        if !dockerfile_path.exists() {
            return Err("no Dockerfile found in source".into());
        }

        // Patch Dockerfile to inject OTel auto-instrumentation (zero-code change for the agent).
        let original = tokio::fs::read_to_string(&dockerfile_path).await
            .map_err(|e| format!("read Dockerfile: {e}"))?;
        let patched = nasiko_observability::patch_dockerfile_for_otel(&original);
        if patched != original {
            tokio::fs::write(&dockerfile_path, &patched).await
                .map_err(|e| format!("write patched Dockerfile: {e}"))?;
            tracing::info!(build_id = %build_id, "patched Dockerfile with OTel instrumentation");
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
                    &db, &oci_storage, &http_client, key, &agent_name,
                ).await;
            }
        }
        Err(e) => {
            tracing::error!(build_id = %build_id, %e, "build failed");
            set_build_status(&db, build_id, BuildStatus::Failed).await;
        }
    }
}

const MAX_ZIP_FILES: usize = 1_000;
const MAX_ZIP_UNCOMPRESSED: u64 = 200 * 1024 * 1024; // 200 MiB

/// Extract a zip archive from a byte slice into `dest`.
/// Kept for the build/S3 path which already has data in memory.
pub fn extract_zip_to_dir(data: &[u8], dest: &std::path::Path) -> std::result::Result<(), String> {
    extract_zip_reader(std::io::Cursor::new(data), dest)
}

/// Extract a zip archive from a file on disk into `dest`.
/// Used by the upload path after streaming the zip to disk.
pub fn extract_zip_from_file(zip_path: &std::path::Path, dest: &std::path::Path) -> std::result::Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("open zip: {e}"))?;
    extract_zip_reader(std::io::BufReader::new(f), dest)
}

fn extract_zip_reader<R: std::io::Read + std::io::Seek>(
    reader: R,
    dest: &std::path::Path,
) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;

    if archive.len() > MAX_ZIP_FILES {
        return Err(format!("zip contains {} files, limit is {MAX_ZIP_FILES}", archive.len()));
    }

    let mut uncompressed_total: u64 = 0;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;

        // Path traversal guard: `enclosed_name()` returns None for any entry whose stored
        // path contains `..` or an absolute root — zip 2.x `mangled_name()` strips those
        // components silently, so the Component::ParentDir check would never fire on it.
        let safe_path = match file.enclosed_name() {
            Some(p) => p,
            None => {
                return Err(format!("zip traversal attempt: {:?}", file.name()));
            }
        };

        let path = dest.join(&safe_path);

        // Belt-and-suspenders: verify the resolved path stays inside dest
        if !path.starts_with(dest) {
            return Err(format!("zip traversal attempt (join escaped dest): {}", safe_path.display()));
        }

        if file.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
            // Zip-bomb guard: bound the ACTUAL bytes written, not the declared
            // `file.size()` (a bomb declares 0 while inflating to gigabytes).
            // Read at most `remaining + 1` so an over-limit entry is detected.
            let remaining = MAX_ZIP_UNCOMPRESSED.saturating_sub(uncompressed_total);
            let written = std::io::copy(&mut std::io::Read::take(&mut file, remaining + 1), &mut out)
                .map_err(|e| e.to_string())?;
            uncompressed_total = uncompressed_total.saturating_add(written);
            if uncompressed_total > MAX_ZIP_UNCOMPRESSED {
                return Err(format!(
                    "zip uncompressed size exceeds {MAX_ZIP_UNCOMPRESSED} bytes — possible zip bomb"
                ));
            }
        }
    }
    Ok(())
}

// ─── GET /builds/{id} ───────────────────────────────────────────────────────

async fn get_build(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match sqlx::query_as::<_, BuildRecord>("SELECT * FROM agent_builds WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(build)) => Json(build).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /builds/{id}/progress (SSE) ────────────────────────────────────────

async fn build_progress_sse(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
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

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ─── GET /builds/{id}/logs ──────────────────────────────────────────────────

async fn get_build_logs(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /builds/agent/{agent_id} ───────────────────────────────────────────

async fn list_builds(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
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
            return StatusCode::FORBIDDEN.into_response();
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── Auto-generate capabilities after build ─────────────────────────────────

pub async fn auto_generate_capabilities_pub(
    db: &sqlx::PgPool,
    oci_storage: &nasiko_oci::storage::S3Storage,
    http_client: &reqwest::Client,
    source_key: &str,
    agent_name: &str,
) {
    use crate::capabilities::generator::CapabilityGenerator;
    use nasiko_router::providers::LLMProvider;

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
    let model = std::env::var("CAPABILITY_GENERATOR_MODEL")
        .unwrap_or_else(|_| "deepseek-v4-flash".into());
    let generator = CapabilityGenerator::new(provider, model);

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
        let mut file = match archive.by_index(i) {
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
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
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
