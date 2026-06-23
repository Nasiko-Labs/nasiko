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
    routing::{get, post},
};
use serde::Serialize;
use std::convert::Infallible;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::build::BuildStatus;
use crate::build::routes::extract_zip_to_dir;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload-and-deploy", post(upload_and_deploy))
        .route("/deploy-status/{build_id}", get(deploy_status_sse))
}

// ─── Response ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct UploadAndDeployResponse {
    pub build_id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub image_tag: String,
    pub status: &'static str,
}

// ─── POST /upload-and-deploy ─────────────────────────────────────────────────

async fn upload_and_deploy(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // TODO: restore claims extraction once auth middleware is re-enabled
    // let claims: Claims = ...;
    // let owner_id: Uuid = claims.sub.parse()...;
    let _owner_id: Uuid = Uuid::nil();

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

    // TODO: restore catalog upsert + build record once DB is wired back in
    // let agent_id = sqlx::query_scalar!(...).fetch_one(&state.db).await?;
    // let build_id = sqlx::query_scalar!(...).fetch_one(&state.db).await?;
    // let source_key = format!("sources/{agent_id}/{version_tag}.zip");
    // state.oci_storage.put_blob(&source_key, ...).await?;
    let agent_id = Uuid::new_v4();
    let build_id = Uuid::new_v4();
    let image_tag = format!("{name}:{version_tag}");

    let runtime = state.runtime.clone();
    let name_clone = name.clone();
    let image_tag_clone = image_tag.clone();
    let ports_clone = if ports.is_empty() { vec![8000] } else { ports };

    tokio::spawn(async move {
        execute_upload_and_deploy(
            runtime,
            build_id,
            agent_id,
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
    build_id: Uuid,
    agent_id: Uuid,
    name: String,
    source_data: Vec<u8>,
    image_tag: String,
    ports: Vec<u16>,
    env: HashMap<String, String>,
) {
    // TODO: restore DB status tracking once wired back in
    // set_build_status(&db, build_id, "building").await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-agent-{build_id}"));

    let result: Result<(), String> = async {
        // Extract zip.
        extract_zip_to_dir(&source_data, &tmp_dir)?;

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

        Ok(())
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match result {
        Ok(()) => {
            tracing::info!(build_id = %build_id, agent_id = %agent_id, "upload-and-deploy succeeded");
            // TODO: update agent status to 'running' and record agent_version in DB
        }
        Err(e) => {
            tracing::error!(build_id = %build_id, %e, "upload-and-deploy failed");
            // TODO: set build status to 'failed' in DB
        }
    }
}

// TODO: restore once DB is wired back in
// async fn set_build_status(db: &sqlx::PgPool, build_id: Uuid, status: &str) {
//     let _ = sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
//         .bind(build_id)
//         .bind(status)
//         .execute(db)
//         .await;
// }

// ─── GET /deploy-status/{build_id} (SSE) ─────────────────────────────────────
// TODO: restore once DB is wired back in — currently returns not_found immediately
// since build records are not persisted without the DB calls above.

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