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
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::auth::Claims;
use crate::build::BuildStatus;
use crate::build::routes::extract_zip_to_dir;
use crate::state::AppState;

/// Outbound providers the LLM router can translate to, and the inbound SDK formats it
/// can parse — used to validate `llm-config` writes.
const SUPPORTED_PROVIDERS: [&str; 3] = ["openai", "anthropic", "gemini"];
const SUPPORTED_INBOUND_FORMATS: [&str; 3] = ["openai", "anthropic", "gemini"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/upload-and-deploy", post(upload_and_deploy))
        .route("/deploy-status/{build_id}", get(deploy_status_sse))
        .route(
            "/{id}/llm-config",
            get(get_llm_config).patch(update_llm_config),
        )
}

/// Resolve the agent's owner, enforcing owner-only (superuser override) access for
/// llm-config read/write. `Err` is a ready-to-return response (404 unknown / 403 not owner).
async fn agent_owner_or_reject(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    user_id: Uuid,
    is_superuser: bool,
) -> Result<Uuid, axum::response::Response> {
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    match owner {
        None => Err((StatusCode::NOT_FOUND, "agent not found").into_response()),
        Some(o) if o != user_id && !is_superuser => {
            Err((StatusCode::FORBIDDEN, "not the agent owner").into_response())
        }
        Some(o) => Ok(o),
    }
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

// ─── PATCH /{id}/llm-config ──────────────────────────────────────────────────

/// Self-service LLM routing config for an agent (P2.6). Sets the `agents.llm_config`
/// JSONB (provider/model/fallbacks/tuning/secret) and, optionally, `inbound_format`.
/// Owner-only (or superuser); the gateway routes off this on the next request (≤ cache TTL).
#[derive(Debug, Deserialize)]
pub struct UpdateLlmConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<i64>,
    /// Name of the caller's `user_secrets` row holding the provider API key. None ⇒ the
    /// platform-key path (see resolver §6.5).
    #[serde(default)]
    pub api_key_secret_name: Option<String>,
    /// Optionally also change which SDK the agent's code speaks (drives deploy injection).
    #[serde(default)]
    pub inbound_format: Option<String>,
}

/// Validate the provider/model/inbound_format fields (everything that doesn't need the DB).
fn validate_llm_config(req: &UpdateLlmConfigRequest) -> Result<(), String> {
    if !SUPPORTED_PROVIDERS.contains(&req.provider.as_str()) {
        return Err(format!(
            "unsupported provider '{}' (expected one of: {})",
            req.provider,
            SUPPORTED_PROVIDERS.join(", ")
        ));
    }
    if req.model.trim().is_empty() {
        return Err("model must not be empty".to_string());
    }
    if let Some(fmt) = &req.inbound_format
        && !SUPPORTED_INBOUND_FORMATS.contains(&fmt.as_str())
    {
        return Err(format!(
            "unsupported inbound_format '{fmt}' (expected one of: {})",
            SUPPORTED_INBOUND_FORMATS.join(", ")
        ));
    }
    Ok(())
}

/// `GET /{id}/llm-config` — current routing config + inbound format (owner/superuser).
async fn get_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };
    if let Err(resp) = agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        return resp;
    }

    let row: Option<(Option<serde_json::Value>, String)> = sqlx::query_as(
        "SELECT llm_config, inbound_format FROM agents WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match row {
        Some((llm_config, inbound_format)) => (
            StatusCode::OK,
            Json(json!({
                "agent_id": agent_id,
                "llm_config": llm_config,           // null ⇒ backward-compat defaults apply
                "inbound_format": inbound_format,
            })),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "agent not found").into_response(),
    }
}

async fn update_llm_config(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(req): Json<UpdateLlmConfigRequest>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user id").into_response(),
    };

    // Owner-only mutation (superuser may override). Read access (public/grant) is NOT
    // enough to edit routing config.
    let owner = match agent_owner_or_reject(&state.db, agent_id, user_id, claims.is_superuser).await
    {
        Ok(owner) => owner,
        Err(resp) => return resp,
    };

    if let Err(msg) = validate_llm_config(&req) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    // A referenced secret must exist for this owner (resolver would otherwise 400 at call
    // time). Validate against the agent owner's secrets, not the (possibly superuser) caller.
    if let Some(name) = req.api_key_secret_name.as_deref().filter(|s| !s.is_empty()) {
        let exists: bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM user_secrets WHERE user_id = $1 AND name = $2)",
        )
        .bind(owner)
        .bind(name)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
        if !exists {
            return (
                StatusCode::BAD_REQUEST,
                format!("secret '{name}' not found for the agent owner"),
            )
                .into_response();
        }
    }

    // Build the llm_config JSONB exactly as the resolver's LLMConfig deserializes it.
    let llm_config = json!({
        "provider": req.provider,
        "model": req.model,
        "fallback_models": req.fallback_models,
        "temperature": req.temperature,
        "max_tokens": req.max_tokens,
        "api_key_secret_name": req.api_key_secret_name,
    });

    let result = if let Some(fmt) = &req.inbound_format {
        sqlx::query(
            "UPDATE agents SET llm_config = $2, inbound_format = $3, updated_at = now() WHERE id = $1",
        )
        .bind(agent_id)
        .bind(&llm_config)
        .bind(fmt)
        .execute(&state.db)
        .await
    } else {
        sqlx::query("UPDATE agents SET llm_config = $2, updated_at = now() WHERE id = $1")
            .bind(agent_id)
            .bind(&llm_config)
            .execute(&state.db)
            .await
    };

    match result {
        Ok(_) => {
            let mut body = json!({ "agent_id": agent_id, "llm_config": llm_config });
            if let Some(fmt) = &req.inbound_format {
                body["inbound_format"] = json!(fmt);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to update llm_config: {e}"),
        )
            .into_response(),
    }
}

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
    let mut source_data: Option<Vec<u8>> = None;
    let mut ports: Vec<u16> = vec![];
    let mut env: HashMap<String, String> = HashMap::new();
    // Which LLM SDK the agent's code speaks (drives gateway env injection). Default openai.
    let mut inbound_format: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "name" => name = field.text().await.ok(),
            "version_tag" => version_tag = field.text().await.ok(),
            "inbound_format" => inbound_format = field.text().await.ok(),
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
    // Accept only the supported SDK formats; anything else falls back to openai.
    let inbound_format = match inbound_format.as_deref() {
        Some("anthropic") => "anthropic",
        Some("gemini") => "gemini",
        _ => "openai",
    };
    let source_data = match source_data {
        Some(d) => d,
        None => return (StatusCode::BAD_REQUEST, "source zip is required").into_response(),
    };

    let image_tag = format!("{name}:{version_tag}");

    // Upsert the agent. Agent names are not globally unique (migration 006 dropped
    // the unique constraint), so scope the lookup to this owner instead of
    // relying on ON CONFLICT (name).
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
            "UPDATE agents SET version = $2, image = $3, status = 'deploying', inbound_format = $4, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&version_tag)
        .bind(&image_tag)
        .bind(inbound_format)
        .execute(&state.db)
        .await;
        id
    } else {
        match sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agents (name, owner_id, version, image, status, inbound_format) \
             VALUES ($1, $2, $3, $4, 'deploying', $5) RETURNING id",
        )
        .bind(&name)
        .bind(owner_id)
        .bind(&version_tag)
        .bind(&image_tag)
        .bind(inbound_format)
        .fetch_one(&state.db)
        .await
        {
            Ok(id) => id,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("register agent: {e}"))
                    .into_response();
            }
        }
    };

    // Persist a build record (status defaults to 'queued').
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
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("create build record: {e}"))
                .into_response();
        }
    };

    // Wire the agent's LLM SDK through the gateway (mint JWT + inject base-URL/key per the
    // agent's inbound_format). Best-effort; skipped (with a warning) if the gateway isn't
    // configured.
    let mut env = env;
    crate::llm_wiring::inject_agent_llm_env(&state.db, &mut env, agent_id, Some(owner_id)).await;

    let runtime = state.runtime.clone();
    let db = state.db.clone();
    let name_clone = name.clone();
    let image_tag_clone = image_tag.clone();
    let ports_clone = if ports.is_empty() { vec![8000] } else { ports };

    tokio::spawn(async move {
        execute_upload_and_deploy(
            runtime,
            db,
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
    db: sqlx::PgPool,
    build_id: Uuid,
    agent_id: Uuid,
    name: String,
    source_data: Vec<u8>,
    image_tag: String,
    ports: Vec<u16>,
    env: HashMap<String, String>,
) {
    set_build_status(&db, build_id, BuildStatus::Building).await;

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
            set_build_status(&db, build_id, BuildStatus::Success).await;
            // Record the built version (idempotent on agent_id+version).
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
            tracing::info!(build_id = %build_id, agent_id = %agent_id, "upload-and-deploy succeeded");
        }
        Err(e) => {
            set_build_status(&db, build_id, BuildStatus::Failed).await;
            let _ = sqlx::query("UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .execute(&db)
                .await;
            tracing::error!(build_id = %build_id, %e, "upload-and-deploy failed");
        }
    }
}

async fn set_build_status(db: &sqlx::PgPool, build_id: Uuid, status: BuildStatus) {
    if let Err(e) =
        sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
            .bind(build_id)
            .bind(status)
            .execute(db)
            .await
    {
        tracing::error!(build_id = %build_id, ?status, %e, "failed to update build status");
    }
}

// ─── GET /deploy-status/{build_id} (SSE) ─────────────────────────────────────
// Streams the build's status (persisted by execute_upload_and_deploy) until it
// reaches a terminal state. Emits {"status":"not_found"} for unknown build ids.

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
#[cfg(test)]
mod tests {
    use super::*;

    fn req(provider: &str, model: &str, inbound: Option<&str>) -> UpdateLlmConfigRequest {
        UpdateLlmConfigRequest {
            provider: provider.into(),
            model: model.into(),
            fallback_models: vec![],
            temperature: None,
            max_tokens: None,
            api_key_secret_name: None,
            inbound_format: inbound.map(str::to_string),
        }
    }

    #[test]
    fn accepts_supported_provider_and_format() {
        assert!(validate_llm_config(&req("anthropic", "claude-3-5-sonnet-20241022", Some("gemini"))).is_ok());
        assert!(validate_llm_config(&req("openai", "gpt-4o-mini", None)).is_ok());
    }

    #[test]
    fn rejects_unsupported_provider() {
        let err = validate_llm_config(&req("cohere", "command-r", None)).unwrap_err();
        assert!(err.contains("unsupported provider"));
    }

    #[test]
    fn rejects_empty_model() {
        let err = validate_llm_config(&req("openai", "   ", None)).unwrap_err();
        assert!(err.contains("model must not be empty"));
    }

    #[test]
    fn rejects_unsupported_inbound_format() {
        let err = validate_llm_config(&req("openai", "gpt-4o", Some("crewai"))).unwrap_err();
        assert!(err.contains("unsupported inbound_format"));
    }
}
