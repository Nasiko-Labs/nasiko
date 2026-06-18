use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{error::Result, AppState};

#[derive(Deserialize)]
pub struct CatalogParams {
    last: Option<String>,
    n: Option<i64>,
}

#[derive(Serialize)]
pub struct Catalog {
    repositories: Vec<String>,
}

/// GET /v2/_catalog — list all repositories in the registry
pub async fn catalog(
    State(state): State<AppState>,
    Query(params): Query<CatalogParams>,
) -> Result<Json<Catalog>> {
    let limit = params.n.unwrap_or(100).min(1000);

    let rows = sqlx::query(
        r#"
        SELECT DISTINCT repository
        FROM oci_manifests
        WHERE ($1::text IS NULL OR repository > $1)
        ORDER BY repository
        LIMIT $2
        "#,
    )
    .bind(&params.last)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    let repositories: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("repository").ok())
        .collect();

    Ok(Json(Catalog { repositories }))
}
