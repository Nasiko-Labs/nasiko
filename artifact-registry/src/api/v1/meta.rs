use axum::{extract::State, Json};
use serde_json::json;

use crate::{db::queries, error::Result, AppState};

pub async fn frameworks(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let frameworks = queries::distinct_frameworks(&state.pool).await?;
    Ok(Json(json!({ "data": frameworks })))
}

pub async fn owners(State(state): State<AppState>) -> Result<Json<serde_json::Value>> {
    let owners = queries::distinct_owners(&state.pool).await?;
    Ok(Json(json!({ "data": owners })))
}
