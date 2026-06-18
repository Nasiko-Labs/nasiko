mod api;
mod auth;
mod config;
mod db;
mod embeddings;
mod error;
mod models;
mod storage;

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::{IntoResponse, Json},
    routing::{delete, get, patch, post, put},
    Router,
};
use bytes::BytesMut;
use dashmap::DashMap;
use rust_embed::Embed;
use serde_json::json;
use sqlx::PgPool;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

#[derive(Embed)]
#[folder = "../ui/registry/"]
struct RegistryAssets;

#[derive(Embed)]
#[folder = "../ui/common/"]
#[prefix = "common/"]
struct CommonAssets;

use crate::{
    api::{
        v1::{artifacts as v1_artifacts, meta as v1_meta, search as v1_search},
        v2::{blobs as v2_blobs, catalog as v2_catalog, manifests as v2_manifests, tags as v2_tags},
    },
    config::Config,
    storage::S3Storage,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub storage: S3Storage,
    pub config: Config,
    /// In-memory buffers for chunked blob uploads (docker push PATCH protocol).
    /// Each entry is keyed by upload UUID and accumulates chunks until PUT finalizes.
    pub upload_buffers: Arc<DashMap<Uuid, BytesMut>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nasiko_registry=info".parse()?)
                .add_directive("tower_http=debug".parse()?),
        )
        .init();

    let config = Config::from_env()?;

    tracing::info!("connecting to database…");
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    tracing::info!("connecting to S3 storage…");
    let storage = S3Storage::new(
        config.s3_endpoint.clone(),
        config.s3_region.clone(),
        config.aws_access_key_id.clone(),
        config.aws_secret_access_key.clone(),
        config.s3_bucket.clone(),
        config.s3_force_path_style,
    )
    .await?;
    storage.ensure_bucket(config.s3_skip_bucket_create).await?;

    let state = AppState {
        pool,
        storage,
        config: config.clone(),
        upload_buffers: Arc::new(DashMap::new()),
    };

    let app = Router::new()
        // Health
        .route("/health", get(health))
        // ── v1 metadata API ─────────────────────────────────────────────────
        .route("/v1/artifacts", post(v1_artifacts::publish))
        .route("/v1/artifacts/{owner}", get(v1_artifacts::list_by_owner))
        .route("/v1/artifacts/{owner}/{name}", get(v1_artifacts::get_latest))
        .route("/v1/artifacts/{owner}/{name}/versions", get(v1_artifacts::list_versions))
        .route("/v1/artifacts/{owner}/{name}/{version}", get(v1_artifacts::get_version))
        .route("/v1/artifacts/{owner}/{name}/{version}", delete(v1_artifacts::yank))
        .route("/v1/artifacts/{owner}/{name}/{version}/download", get(v1_artifacts::download))
        .route("/v1/search", get(v1_search::search))
        .route("/v1/meta/frameworks", get(v1_meta::frameworks))
        .route("/v1/meta/owners", get(v1_meta::owners))
        .route("/v1/agents/{owner}/{name}/.well-known/agent.json", get(v1_artifacts::agent_card))
        // ── v2 OCI Distribution Spec ────────────────────────────────────────
        .route("/v2/", get(oci_version_check))
        .route("/v2/_catalog", get(v2_catalog::catalog))
        .route("/v2/{owner}/{repo}/manifests/{reference}", get(v2_manifests::get_manifest))
        .route("/v2/{owner}/{repo}/manifests/{reference}", put(v2_manifests::put_manifest))
        .route("/v2/{owner}/{repo}/manifests/{reference}", delete(v2_manifests::delete_manifest))
        .route("/v2/{owner}/{repo}/blobs/{digest}", get(v2_blobs::get_blob).head(v2_blobs::head_blob))
        .route("/v2/{owner}/{repo}/blobs/{digest}", delete(v2_blobs::delete_blob))
        .route("/v2/{owner}/{repo}/blobs/uploads/", post(v2_blobs::initiate_upload))
        .route("/v2/{owner}/{repo}/blobs/uploads/{uuid}", patch(v2_blobs::patch_upload))
        .route("/v2/{owner}/{repo}/blobs/uploads/{uuid}", put(v2_blobs::complete_upload))
        .route("/v2/{owner}/{repo}/referrers/{digest}", get(v2_manifests::get_referrers))
        .route("/v2/{owner}/{repo}/tags/list", get(v2_tags::list_tags))
        // Static UI
        .fallback(static_handler)
        // Middleware
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port).parse()?;
    tracing::info!("artifact registry listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "service": "artifact-registry"}))
}

async fn oci_version_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("OCI-Distribution-Spec-Version", "1.1.0")],
        Json(json!({})),
    )
}

async fn static_handler(req: Request<Body>) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = RegistryAssets::get(path).or_else(|| CommonAssets::get(path)) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (StatusCode::OK, [(header::CONTENT_TYPE, mime.as_ref().to_string())], file.data.to_vec()).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}
