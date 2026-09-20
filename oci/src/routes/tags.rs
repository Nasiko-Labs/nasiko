use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::OciState;
use crate::authz::{Caller, check_pull_access};
use crate::error::{OciError, Result};
use crate::ops;

#[derive(Deserialize)]
pub struct TagListParams {
    last: Option<String>,
    n: Option<i64>,
}

#[derive(Serialize)]
pub struct TagList {
    name: String,
    tags: Vec<String>,
}

/// Ceiling on `?n=`, so one request cannot ask for an unbounded page.
const MAX_PAGE: i64 = 1000;
const DEFAULT_PAGE: i64 = 100;

/// GET /v2/:name/tags/list
///
/// Answers `404 NAME_UNKNOWN` for a repository that holds nothing. An empty tag
/// list is a different, meaningful answer — a repository whose manifests are all
/// digest-addressed — so conflating the two leaves a client unable to tell "no
/// such repository" from "nothing tagged yet".
pub async fn list_tags(
    State(state): State<OciState>,
    caller: Caller,
    Path((owner, repo)): Path<(String, String)>,
    Query(params): Query<TagListParams>,
) -> Result<Response> {
    check_pull_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let limit = params.n.unwrap_or(DEFAULT_PAGE).clamp(0, MAX_PAGE);

    if !ops::manifests::repository_exists(&state, &name).await? {
        return Err(OciError::name_unknown(format!(
            "repository {name} not found"
        )));
    }

    let tags = ops::list_tags(&state, &name, params.last.as_deref(), limit).await?;

    // `Link` is how a client discovers there is another page; without it a caller
    // has to guess whether a full page means "more to come".
    let mut headers = HeaderMap::new();
    if limit > 0
        && tags.len() as i64 == limit
        && let Some(last) = tags.last()
        && let Ok(value) = HeaderValue::from_str(&format!(
            "</v2/{name}/tags/list?n={limit}&last={last}>; rel=\"next\""
        ))
    {
        headers.insert("Link", value);
    }

    Ok((StatusCode::OK, headers, Json(TagList { name, tags })).into_response())
}
