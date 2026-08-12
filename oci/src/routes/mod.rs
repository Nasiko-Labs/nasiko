pub mod blobs;
pub mod catalog;
pub mod manifests;
pub mod tags;

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, patch, post, put},
};

use crate::OciState;

/// Normalise OCI image names so that single-segment names (e.g. `my-agent`)
/// are rewritten to two-segment names (`nasiko/my-agent`) before routing.
///
/// The OCI Distribution spec allows repository names with any number of
/// `/`-separated path components. Our routes use `{owner}/{repo}`, which
/// requires at least two components. Agents built without an explicit owner
/// prefix push to paths like `/v2/my-agent/blobs/uploads/` — one component —
/// which Axum cannot match against the two-component pattern, yielding a 404.
///
/// This middleware intercepts those requests and inserts `nasiko/` as the
/// owner before the existing routes see the URI, making both `my-agent` and
/// `nasiko/my-agent` work transparently.
async fn normalize_image_name(
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let (mut parts, body) = req.into_parts();

    if let Some(normalized) = rewrite_single_segment(parts.uri.path()) {
        let query = parts
            .uri
            .path_and_query()
            .and_then(|pq| pq.query())
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let new_path_and_query = format!("{normalized}{query}");
        if let Ok(new_pq) = new_path_and_query.parse() {
            let mut uri_parts = parts.uri.clone().into_parts();
            uri_parts.path_and_query = Some(new_pq);
            if let Ok(new_uri) = axum::http::Uri::from_parts(uri_parts) {
                parts.uri = new_uri;
            }
        }
    }

    next.run(axum::extract::Request::from_parts(parts, body))
        .await
}

/// Returns a rewritten path if `path` is `/v2/{name}/{oci-verb}/...` where
/// `name` is a single path component (no `/`). Returns `None` when the path
/// already has two or more name segments or is not a repository path.
fn rewrite_single_segment(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/v2/")?;
    // Skip the version endpoint and catalog — those are not repository paths.
    if rest.is_empty() || rest.starts_with('_') {
        return None;
    }
    let slash = rest.find('/')?;
    let first = &rest[..slash];
    let after = &rest[slash + 1..];

    // If `after` immediately starts with a known OCI operation keyword it
    // means `first` was the ONLY name segment — rewrite to nasiko/{first}.
    // Otherwise a second name segment precedes the keyword and no rewrite
    // is needed.
    let oci_ops = ["blobs/", "manifests/", "tags/", "referrers/", "uploads/"];
    if oci_ops.iter().any(|op| after.starts_with(op)) {
        return Some(format!("/v2/nasiko/{first}/{after}"));
    }
    None
}

pub fn router(state: OciState) -> Router {
    Router::new()
        .route("/v2/", get(version_check))
        .route("/v2/_catalog", get(catalog::catalog))
        .route(
            "/v2/{owner}/{repo}/manifests/{reference}",
            get(manifests::get_manifest),
        )
        .route(
            "/v2/{owner}/{repo}/manifests/{reference}",
            put(manifests::put_manifest),
        )
        .route(
            "/v2/{owner}/{repo}/manifests/{reference}",
            axum::routing::delete(manifests::delete_manifest),
        )
        .route(
            "/v2/{owner}/{repo}/referrers/{digest}",
            get(manifests::get_referrers),
        )
        .route(
            "/v2/{owner}/{repo}/blobs/{digest}",
            get(blobs::get_blob).head(blobs::head_blob),
        )
        .route(
            "/v2/{owner}/{repo}/blobs/{digest}",
            axum::routing::delete(blobs::delete_blob),
        )
        .route(
            "/v2/{owner}/{repo}/blobs/uploads/",
            post(blobs::initiate_upload),
        )
        .route(
            "/v2/{owner}/{repo}/blobs/uploads/{uuid}",
            patch(blobs::patch_upload)
                .put(blobs::complete_upload)
                // end-13: resume a partial upload after losing local bookkeeping.
                .get(blobs::upload_status)
                // Abandon a session and free its buffer immediately.
                .delete(blobs::cancel_upload),
        )
        .route("/v2/{owner}/{repo}/tags/list", get(tags::list_tags))
        // Defense-in-depth: reject oversized request bodies at the router/
        // middleware layer, before a handler (and its manual
        // `axum::body::to_bytes(.., MAX_CHUNK_BYTES)` call) even runs. Mirrors
        // the same per-chunk cap used inside `blobs::patch_upload` /
        // `blobs::complete_upload` so both layers agree.
        .layer(DefaultBodyLimit::max(blobs::MAX_CHUNK_BYTES))
        .layer(middleware::from_fn(normalize_image_name))
        .with_state(state)
}

async fn version_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("OCI-Distribution-Spec-Version", "1.1.0")],
        Json(serde_json::json!({})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_blob_upload_rewritten() {
        assert_eq!(
            rewrite_single_segment("/v2/my-agent/blobs/uploads/"),
            Some("/v2/nasiko/my-agent/blobs/uploads/".to_string())
        );
    }

    #[test]
    fn single_segment_manifests_rewritten() {
        assert_eq!(
            rewrite_single_segment("/v2/my-agent/manifests/latest"),
            Some("/v2/nasiko/my-agent/manifests/latest".to_string())
        );
    }

    #[test]
    fn two_segment_not_rewritten() {
        assert_eq!(
            rewrite_single_segment("/v2/nasiko/my-agent/blobs/uploads/"),
            None
        );
    }

    #[test]
    fn version_endpoint_not_rewritten() {
        assert_eq!(rewrite_single_segment("/v2/"), None);
    }

    #[test]
    fn catalog_not_rewritten() {
        assert_eq!(rewrite_single_segment("/v2/_catalog"), None);
    }
}
