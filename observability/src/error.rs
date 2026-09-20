use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObservabilityError {
    #[error("Tempo error: {0}")]
    TempoError(String),

    #[error("Loki error: {0}")]
    LokiError(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("deserialization error: {0}")]
    Deserialization(String),

    #[error("internal error: {0}")]
    Internal(String),
}