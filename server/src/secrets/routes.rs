use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

use super::crypto::SecretsCrypto;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/secrets", get(list_secrets).post(create_secret))
        .route("/secrets/{name}", get(get_secret).put(update_secret).delete(delete_secret))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct SecretEntry {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
struct CreateSecret {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct UpdateSecret {
    value: String,
}

#[derive(Debug, Serialize)]
struct SecretValue {
    name: String,
    value: String,
}

async fn list_secrets(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    match sqlx::query_as::<_, SecretEntry>(
        "SELECT id, name, created_at, updated_at FROM user_secrets WHERE user_id = $1 ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(secrets) => Json(secrets).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "list_secrets: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn create_secret(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateSecret>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let crypto = SecretsCrypto::for_user(user_id);
    let encrypted = crypto.encrypt(&body.value);

    let result = sqlx::query_as::<_, SecretEntry>(
        r#"INSERT INTO user_secrets (user_id, name, encrypted_value)
           VALUES ($1, $2, $3)
           ON CONFLICT (user_id, name) DO UPDATE SET encrypted_value = $3, updated_at = now()
           RETURNING id, name, created_at, updated_at"#,
    )
    .bind(user_id)
    .bind(&body.name)
    .bind(&encrypted)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(entry) => (StatusCode::CREATED, Json(entry)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "create_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn get_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let encrypted: Option<String> = sqlx::query_scalar(
        "SELECT encrypted_value FROM user_secrets WHERE user_id = $1 AND name = $2",
    )
    .bind(user_id)
    .bind(&name)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    match encrypted {
        Some(enc) => {
            let crypto = SecretsCrypto::for_user(user_id);
            match crypto.decrypt(&enc) {
                Ok(value) => Json(SecretValue { name, value }).into_response(),
                Err(e) => {
                    tracing::error!(%e, %user_id, "get_secret: decrypt failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
                }
            }
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn update_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
    Json(body): Json<UpdateSecret>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let crypto = SecretsCrypto::for_user(user_id);
    let encrypted = crypto.encrypt(&body.value);

    let result = sqlx::query(
        "UPDATE user_secrets SET encrypted_value = $3, updated_at = now() WHERE user_id = $1 AND name = $2",
    )
    .bind(user_id)
    .bind(&name)
    .bind(&encrypted)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %name, "update_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn delete_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query(
        "DELETE FROM user_secrets WHERE user_id = $1 AND name = $2",
    )
    .bind(user_id)
    .bind(&name)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %name, "delete_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
