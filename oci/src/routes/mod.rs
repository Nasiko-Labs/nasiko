pub mod blobs;
pub mod catalog;
pub mod manifests;
pub mod tags;

use axum::{
    Json, Router,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post, put},
};

use crate::OciState;

pub fn router(state: OciState) -> Router {
    Router::new()
        .route("/v2/", get(version_check))
        .route("/v2/_catalog", get(catalog::catalog))
        .route("/v2/{owner}/{repo}/manifests/{reference}", get(manifests::get_manifest))
        .route("/v2/{owner}/{repo}/manifests/{reference}", put(manifests::put_manifest))
        .route("/v2/{owner}/{repo}/manifests/{reference}", axum::routing::delete(manifests::delete_manifest))
        .route("/v2/{owner}/{repo}/referrers/{digest}", get(manifests::get_referrers))
        .route("/v2/{owner}/{repo}/blobs/{digest}", get(blobs::get_blob).head(blobs::head_blob))
        .route("/v2/{owner}/{repo}/blobs/{digest}", axum::routing::delete(blobs::delete_blob))
        .route("/v2/{owner}/{repo}/blobs/uploads/", post(blobs::initiate_upload))
        .route("/v2/{owner}/{repo}/blobs/uploads/{uuid}", patch(blobs::patch_upload))
        .route("/v2/{owner}/{repo}/blobs/uploads/{uuid}", put(blobs::complete_upload))
        .route("/v2/{owner}/{repo}/tags/list", get(tags::list_tags))
        .with_state(state)
}

async fn version_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("OCI-Distribution-Spec-Version", "1.1.0")],
        Json(serde_json::json!({})),
    )
}
