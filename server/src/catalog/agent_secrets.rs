use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::Claims;
use crate::secrets::crypto::SecretsCrypto;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/secrets", get(list_secrets).post(set_secret))
        .route("/agents/{agent_id}/secrets/import", axum::routing::post(import_secrets))
        .route("/agents/{agent_id}/secrets/{name}", axum::routing::delete(delete_secret))
}

#[derive(Serialize)]
struct SecretListEntry {
    name: String,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct SetSecretRequest {
    name: String,
    value: String,
}

async fn list_secrets(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    if !can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let secrets_env: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT secrets_env FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let names: Vec<SecretListEntry> = match secrets_env {
        Some(obj) => obj
            .as_object()
            .map(|m| m.keys().map(|k| SecretListEntry { name: k.clone(), updated_at: None }).collect())
            .unwrap_or_default(),
        None => vec![],
    };

    Json(names).into_response()
}

async fn set_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<SetSecretRequest>,
) -> impl IntoResponse {
    if !can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let crypto = SecretsCrypto::from_env();
    let encrypted = crypto.encrypt(&body.value);

    let result = sqlx::query(
        r#"UPDATE agents
           SET secrets_env = jsonb_set(COALESCE(secrets_env, '{}'), array[$2], to_jsonb($3::text)),
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
    .bind(&body.name)
    .bind(&encrypted)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::CREATED.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path((agent_id, name)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    if !can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let result = sqlx::query(
        r#"UPDATE agents
           SET secrets_env = secrets_env - $2,
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
    .bind(&name)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct ImportSecretsRequest {
    secret_names: Vec<String>,
}

async fn import_secrets(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
    Json(body): Json<ImportSecretsRequest>,
) -> impl IntoResponse {
    if !can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }

    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match import_user_secrets(&state.db, user_id, agent_id, &body.secret_names).await {
        Ok(count) => Json(serde_json::json!({"imported": count})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Resolve agent secrets_env into a decrypted HashMap for container deployment.
pub async fn resolve_agent_env(db: &sqlx::PgPool, agent_id: Uuid) -> HashMap<String, String> {
    let secrets_env: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT secrets_env FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    let Some(obj) = secrets_env.and_then(|v| v.as_object().cloned()) else {
        return HashMap::new();
    };

    if obj.is_empty() {
        return HashMap::new();
    }

    let crypto = SecretsCrypto::from_env();
    obj.into_iter()
        .filter_map(|(k, v)| {
            let encrypted = v.as_str()?;
            let decrypted = crypto.decrypt(encrypted).ok()?;
            Some((k, decrypted))
        })
        .collect()
}

/// Import user secrets into an agent's secrets_env.
pub async fn import_user_secrets(
    db: &sqlx::PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    secret_names: &[String],
) -> Result<usize, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, encrypted_value FROM user_secrets WHERE user_id = $1 AND name = ANY($2)",
    )
    .bind(user_id)
    .bind(secret_names)
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    for (name, encrypted_value) in &rows {
        sqlx::query(
            r#"UPDATE agents
               SET secrets_env = jsonb_set(COALESCE(secrets_env, '{}'), array[$2], to_jsonb($3::text)),
                   updated_at = now()
               WHERE id = $1"#,
        )
        .bind(agent_id)
        .bind(name)
        .bind(encrypted_value)
        .execute(db)
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(rows.len())
}

async fn can_manage_agent(state: &AppState, claims: &Claims, agent_id: Uuid) -> bool {
    if claims.is_superuser {
        return true;
    }
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return false,
    };
    crate::acl::user_can_access_agent(&state.db, user_id, agent_id).await
}
