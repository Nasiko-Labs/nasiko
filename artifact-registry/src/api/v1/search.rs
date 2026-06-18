use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::json;

use crate::{
    db::queries,
    embeddings,
    error::Result,
    models::search::SearchParams,
    AppState,
};

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<serde_json::Value>> {
    let query_embedding = match (&state.config.openai_api_key, &params.q) {
        (Some(key), Some(q)) if !q.is_empty() => {
            embeddings::generate(key, q).await.ok()
        }
        _ => None,
    };

    let result = queries::search_artifacts(&state.pool, &params, query_embedding).await?;
    Ok(Json(json!({
        "data": result.items,
        "total": result.total,
        "limit": result.limit,
        "offset": result.offset,
    })))
}
