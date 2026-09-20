use crate::a2a::A2aClientError;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("no agents available in registry")]
    NoAgents,

    #[error("registry discovery failed: {0}")]
    Registry(String),

    #[error("LLM configuration error: {0}")]
    LlmConfig(String),

    #[error("LLM completion error: {0}")]
    Completion(String),

    #[error("tool execution failed: {tool} — {message}")]
    ToolExecution { tool: String, message: String },

    #[error("A2A agent call failed: {0}")]
    A2a(#[from] A2aClientError),

    #[error("max turns ({0}) exceeded without resolution")]
    MaxTurnsExceeded(usize),

    #[error("context serialization error: {0}")]
    Serialization(String),
}
