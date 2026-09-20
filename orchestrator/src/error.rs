use thiserror::Error;

#[derive(Debug, Error)]
pub enum RouterError {
    #[error("no agents available")]
    NoAgentsAvailable,
    #[error("agent not found: {0}")]
    AgentNotFound(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("selection failed: {0}")]
    Selection(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<crate::selector::SelectorError> for RouterError {
    fn from(e: crate::selector::SelectorError) -> Self {
        RouterError::Selection(e.to_string())
    }
}
