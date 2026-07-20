use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OciError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("forbidden: {0}")]
    Forbidden(String),
}

impl IntoResponse for OciError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            OciError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            OciError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            OciError::Database(e) => {
                tracing::error!("oci registry db error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "database error".into())
            }
            OciError::Storage(m) => {
                tracing::error!("oci registry storage error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, "storage error".into())
            }
            OciError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
        };
        (
            status,
            Json(json!({"errors": [{"code": "UNKNOWN", "message": message}]})),
        )
            .into_response()
    }
}

pub type Result<T> = std::result::Result<T, OciError>;
