use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    http::header,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use crate::{auth::AdminAuth, db::queries, embeddings, error::Result, models::artifact::{PublishRequest, PublishResponse}, AppState};

#[derive(Deserialize)]
pub struct ListByOwnerParams {
    #[serde(rename = "type")]
    pub artifact_type: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 { 20 }

pub async fn publish(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Json(req): Json<PublishRequest>,
) -> Result<(StatusCode, Json<PublishResponse>)> {
    // Validate semver loosely
    if req.version.is_empty() {
        return Err(crate::error::AppError::BadRequest("version is required".into()));
    }
    if req.artifact_type.is_empty() {
        return Err(crate::error::AppError::BadRequest("artifact_type is required".into()));
    }

    let artifact = queries::insert_artifact(&state.pool, &req).await?;

    if let Some(api_key) = state.config.openai_api_key.clone() {
        let pool = state.pool.clone();
        let id = artifact.id;
        let text = format!(
            "{} {} {}",
            artifact.name,
            artifact.description.as_deref().unwrap_or(""),
            artifact.tags.join(" ")
        );
        tokio::spawn(async move {
            match embeddings::generate(&api_key, &text).await {
                Ok(vec) => {
                    if let Err(e) = queries::update_artifact_embedding(&pool, id, vec).await {
                        tracing::warn!("failed to store embedding for {id}: {e}");
                    }
                }
                Err(e) => tracing::warn!("embedding generation failed for {id}: {e}"),
            }
        });
    }

    let upload_url = format!(
        "{}/v2/{}/{}/blobs/uploads/",
        state.config.public_base_url, artifact.owner, artifact.name
    );

    Ok((
        StatusCode::CREATED,
        Json(PublishResponse { artifact, upload_url }),
    ))
}

pub async fn list_by_owner(
    State(state): State<AppState>,
    Path(owner): Path<String>,
    Query(params): Query<ListByOwnerParams>,
) -> Result<Json<serde_json::Value>> {
    let (items, total) = queries::list_artifacts_by_owner(
        &state.pool,
        &owner,
        params.artifact_type.as_deref(),
        params.limit,
        params.offset,
    )
    .await?;
    Ok(Json(json!({
        "data": items,
        "total": total,
        "limit": params.limit.min(100),
        "offset": params.offset,
    })))
}

pub async fn get_latest(
    State(state): State<AppState>,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let artifact = queries::get_artifact_latest(&state.pool, &owner, &name).await?;
    Ok(Json(json!({"data": artifact})))
}

pub async fn get_version(
    State(state): State<AppState>,
    Path((owner, name, version)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>> {
    let artifact = queries::get_artifact_version(&state.pool, &owner, &name, &version).await?;
    Ok(Json(json!({"data": artifact})))
}

pub async fn list_versions(
    State(state): State<AppState>,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let versions = queries::list_artifact_versions(&state.pool, &owner, &name).await?;
    Ok(Json(json!({"data": versions})))
}

pub async fn yank(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((owner, name, version)): Path<(String, String, String)>,
) -> Result<StatusCode> {
    queries::yank_artifact(&state.pool, &owner, &name, &version).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn download(
    State(state): State<AppState>,
    Path((owner, name, version)): Path<(String, String, String)>,
) -> Result<impl IntoResponse> {
    let repo = format!("{owner}/{name}");

    // Look up the manifest to find the actual layer blob digest
    let row = sqlx::query(
        "SELECT content::text FROM oci_manifests WHERE repository = $1 AND reference = $2 ORDER BY created_at DESC LIMIT 1"
    )
    .bind(&repo)
    .bind(&version)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| crate::error::AppError::NotFound("manifest not found".into()))?;

    let manifest_json: String = row.try_get("content")?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_json)
        .map_err(|e| crate::error::AppError::Storage(format!("invalid manifest JSON: {e}")))?;

    let layer_digest = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .and_then(|layers| layers.first())
        .and_then(|l| l.get("digest"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| crate::error::AppError::NotFound("no layer in manifest".into()))?;

    let data = state.storage.get_blob(layer_digest).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/gzip")],
        data,
    ))
}

pub async fn agent_card(
    State(state): State<AppState>,
    Path((owner, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let artifact = queries::get_artifact_latest(&state.pool, &owner, &name).await?;

    if artifact.artifact_type != "agent" {
        return Err(crate::error::AppError::NotFound(format!(
            "{owner}/{name} is not an agent artifact"
        )));
    }

    // Return the embedded AgentCard from metadata, or synthesize a minimal one
    let card = if let Some(card) = artifact.metadata.get("agentCard") {
        card.clone()
    } else {
        json!({
            "name": artifact.name,
            "description": artifact.description,
            "version": artifact.version,
        })
    };

    Ok(Json(card))
}
