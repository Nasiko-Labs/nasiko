use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::OciState;
use crate::error::Result;
use crate::ops;

#[derive(Deserialize)]
pub struct CatalogParams {
    last: Option<String>,
    n: Option<i64>,
}

#[derive(Serialize)]
pub struct Catalog {
    repositories: Vec<String>,
}

pub async fn catalog(
    State(state): State<OciState>,
    Query(params): Query<CatalogParams>,
) -> Result<Json<Catalog>> {
    let limit = params.n.unwrap_or(100);
    let repositories = ops::list_repositories(&state, params.last.as_deref(), limit).await?;
    Ok(Json(Catalog { repositories }))
}
