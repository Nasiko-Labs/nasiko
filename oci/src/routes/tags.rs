use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::OciState;
use crate::authz::{check_repo_access, CallerIdentity};
use crate::error::Result;
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

pub async fn list_tags(
    State(state): State<OciState>,
    caller: CallerIdentity,
    Path((owner, repo)): Path<(String, String)>,
    Query(params): Query<TagListParams>,
) -> Result<Json<TagList>> {
    check_repo_access(&state, &caller, &repo).await?;
    let name = format!("{owner}/{repo}");
    let limit = params.n.unwrap_or(100);
    let tags = ops::list_tags(&state, &name, params.last.as_deref(), limit).await?;
    Ok(Json(TagList { name, tags }))
}
