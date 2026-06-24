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

use crate::auth::Claims;
use crate::state::AppState;

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
    status: String,
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
    let owner_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
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

    // Spawn build task
    let build_id = build.id;
    let orch = state.runtime.clone();
    let db = state.db.clone();
    let oci_storage = state.oci_storage.clone();
    let http_client = state.http_client.clone();
    tokio::spawn(async move {
        execute_build(orch, db, build_id, agent_name, github_url, source_key, version_tag, oci_storage, http_client).await;
    });

    (StatusCode::CREATED, Json(build)).into_response()
}

#[allow(clippy::too_many_arguments)]
async fn execute_build(
    runtime: std::sync::Arc<dyn nasiko_runtime::ContainerRuntime>,
    db: sqlx::PgPool,
    build_id: Uuid,
    agent_name: String,
    github_url: Option<String>,
    source_key: Option<String>,
    version_tag: String,
    oci_storage: nasiko_oci::storage::S3Storage,
    http_client: reqwest::Client,
) {
    update_status(&db, build_id, "building").await;

    let image_tag = format!("{agent_name}:{version_tag}");
    let tmp_dir = std::env::temp_dir().join(format!("nasiko-build-{build_id}"));

    let result = async {
        // Acquire source into tmp_dir
        if let Some(url) = &github_url {
            tokio::fs::create_dir_all(&tmp_dir).await
                .map_err(|e| format!("create tmp dir: {e}"))?;
            let status = tokio::process::Command::new("git")
                .args(["clone", "--depth=1", url, tmp_dir.to_str().unwrap_or(".")])
                .status()
                .await
                .map_err(|e| format!("git clone: {e}"))?;
            if !status.success() {
                return Err(format!("git clone failed with exit code {}", status.code().unwrap_or(1)));
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
            update_status(&db, build_id, "success").await;

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
            update_status(&db, build_id, "failed").await;
        }
    }
}

pub fn extract_zip_to_dir(data: &[u8], dest: &std::path::Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = dest.join(file.mangled_name());
        if file.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn update_status(db: &sqlx::PgPool, build_id: Uuid, status: &str) {
    if let Err(e) = sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
        .bind(build_id)
        .bind(status)
        .execute(db)
        .await
    {
        tracing::error!(build_id = %build_id, %status, %e, "failed to update build status");
    }
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
        let mut last_status = String::new();

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

            if record.status != last_status {
                last_status = record.status.clone();
                yield Ok(Event::default().data(
                    serde_json::json!({
                        "status": record.status,
                        "build_id": id,
                    }).to_string()
                ));
            }

            if record.status == "success" || record.status == "failed" {
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
    let user_id: Uuid = claims.sub.parse().unwrap_or_default();

    // Superusers see all builds; others see only builds for their own agents
    let rows = if claims.is_superuser {
        sqlx::query_as::<_, BuildRecord>(
            r#"SELECT b.* FROM agent_builds b
               LEFT JOIN agents a ON a.id = b.agent_id
               WHERE ($1::text IS NULL OR b.status = $1)
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
                 AND ($1::text IS NULL OR b.status = $1)
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
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
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
    use crate::router::providers::LLMProvider;

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

            if let Err(e) = sqlx::query(
                r#"UPDATE agents
                   SET description = COALESCE(NULLIF(description, ''), $2),
                       skills = $3,
                       tags = $4,
                       capabilities = $5,
                       default_input_modes = $6,
                       default_output_modes = $7,
                       updated_at = now()
                   WHERE name = $1"#,
            )
            .bind(agent_name)
            .bind(&card.description)
            .bind(&skills_json)
            .bind(&card.tags)
            .bind(&caps_json)
            .bind(&input_modes)
            .bind(&output_modes)
            .execute(db)
            .await
            {
                tracing::error!("capability gen: failed to update agent '{agent_name}': {e}");
                return;
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
