use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::Claims;
use crate::mcp::ApiResponse;
use crate::state::AppState;

use nasiko_secrets::SecretsCrypto;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/secrets", get(list_secrets).post(create_secret))
        .route(
            "/secrets/{name}",
            get(get_secret).put(update_secret).delete(delete_secret),
        )
}

/// Validate a secret name against the POSIX environment-variable-name
/// convention and reject reserved/dangerous names.
///
/// Secret names become container environment-variable KEYS at agent deploy
/// time (see `agent_secrets::resolve_agent_env` and `build_agent_spec`), so an
/// attacker-chosen name like `LD_PRELOAD` or `PATH` could inject a malicious
/// env var that changes how the agent's container runtime behaves. Modeled on
/// `validate_version_tag` in `build/routes.rs`.
pub(crate) fn validate_secret_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 128 {
        return Err("secret name must be 1-128 characters".into());
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_uppercase() && first != '_' {
        return Err("secret name must start with [A-Z_]".into());
    }
    if !chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
        return Err("secret name may only contain [A-Z0-9_]".into());
    }

    const RESERVED: &[&str] = &[
        "PATH",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "LD_AUDIT",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "IFS",
        "HOME",
        "SHELL",
        "BASH_ENV",
        "ENV",
        "PYTHONPATH",
        "NODE_OPTIONS",
        "PERL5LIB",
        "GIT_SSH_COMMAND",
    ];
    if RESERVED.contains(&name) {
        return Err(format!("'{name}' is a reserved environment variable name"));
    }

    Ok(())
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub(crate) struct SecretEntry {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateSecret {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateSecret {
    value: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SecretValue {
    name: String,
    value: String,
}

/// List the caller's secrets (names + metadata only — never decrypted values).
#[utoipa::path(
    get,
    path = "/api/secrets",
    tag = "secrets",
    responses(
        (status = 200, description = "The caller's secrets", body = [SecretEntry]),
        (status = 401, description = "Missing or invalid session"),
    ),
)]
pub(crate) async fn list_secrets(
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
        Ok(secrets) => {
            ApiResponse::ok(json!(secrets), "Secrets retrieved successfully").into_response()
        }
        Err(e) => {
            tracing::error!(%e, %user_id, "list_secrets: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Create a secret, or overwrite the value if one with this name already exists.
#[utoipa::path(
    post,
    path = "/api/secrets",
    tag = "secrets",
    request_body = CreateSecret,
    responses(
        (status = 201, description = "Secret created (or overwritten)", body = SecretEntry),
        (status = 422, description = "Invalid secret name — see `oss/docs` for the naming rule"),
    ),
)]
pub(crate) async fn create_secret(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateSecret>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    if let Err(msg) = validate_secret_name(&body.name) {
        return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }

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
        Ok(entry) => {
            ApiResponse::created(json!(entry), "Secret created successfully").into_response()
        }
        Err(e) => {
            tracing::error!(%e, %user_id, "create_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Fetch and decrypt a single secret's value.
#[utoipa::path(
    get,
    path = "/api/secrets/{name}",
    tag = "secrets",
    params(
        ("name" = String, Path, description = "Secret name"),
    ),
    responses(
        (status = 200, description = "Decrypted secret value", body = SecretValue),
        (status = 404, description = "No secret with this name"),
    ),
)]
pub(crate) async fn get_secret(
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
                Ok(value) => ApiResponse::ok(
                    json!(SecretValue { name, value }),
                    "Secret retrieved successfully",
                )
                .into_response(),
                Err(e) => {
                    tracing::error!(%e, %user_id, "get_secret: decrypt failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
                }
            }
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Overwrite an existing secret's value.
#[utoipa::path(
    put,
    path = "/api/secrets/{name}",
    tag = "secrets",
    params(
        ("name" = String, Path, description = "Secret name"),
    ),
    request_body = UpdateSecret,
    responses(
        (status = 204, description = "Secret updated"),
        (status = 404, description = "No secret with this name"),
    ),
)]
pub(crate) async fn update_secret(
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
        Ok(r) if r.rows_affected() > 0 => {
            ApiResponse::ok(json!(null), "Secret updated successfully").into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %name, "update_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Delete a secret.
#[utoipa::path(
    delete,
    path = "/api/secrets/{name}",
    tag = "secrets",
    params(
        ("name" = String, Path, description = "Secret name"),
    ),
    responses(
        (status = 204, description = "Secret deleted"),
        (status = 404, description = "No secret with this name"),
    ),
)]
pub(crate) async fn delete_secret(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query("DELETE FROM user_secrets WHERE user_id = $1 AND name = $2")
        .bind(user_id)
        .bind(&name)
        .execute(&state.db)
        .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => {
            ApiResponse::ok(json!(null), "Secret deleted successfully").into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %name, "delete_secret: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
