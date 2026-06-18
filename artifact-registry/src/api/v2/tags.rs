use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{error::Result, AppState};

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

/// GET /v2/:name/tags/list
pub async fn list_tags(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    Query(params): Query<TagListParams>,
) -> Result<Json<TagList>> {
    let name = format!("{owner}/{repo}");
    let limit = params.n.unwrap_or(100).min(1000);

    let rows = sqlx::query(
        r#"
        SELECT DISTINCT reference
        FROM oci_manifests
        WHERE repository = $1
          AND reference IS NOT NULL
          AND reference NOT LIKE 'sha256:%'
          AND ($2::text IS NULL OR reference > $2)
        ORDER BY reference
        LIMIT $3
        "#,
    )
    .bind(&name)
    .bind(&params.last)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    let tags: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.try_get::<Option<String>, _>("reference").ok().flatten())
        .collect();

    Ok(Json(TagList { name, tags }))
}
