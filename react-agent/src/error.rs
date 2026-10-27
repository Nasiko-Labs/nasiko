use crate::a2a::{A2aClientError, PauseInfo};

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

    /// The call guard refused the next model call (`CallGuard::before_completion`).
    #[error("model call refused: {0}")]
    Refused(String),

    /// Not a real failure — a called agent needs a human before this run can continue. Used only
    /// by the non-streaming loop (`Orchestrator::run`); its `Ok` type has no room for "paused"
    /// any more than `rig`'s own erased types did, so this is routed through `Err` for the same
    /// reason `A2aToolError::AwaitingHuman` is. `pause` is boxed: `PauseInfo` is large enough
    /// (several `String`s plus a `serde_json::Value`) that inlining it here would bloat every
    /// `Result<_, OrchestratorError>` in this crate, not just this one variant's callers.
    #[error("agent '{agent}' is awaiting a human")]
    AwaitingHuman {
        agent: String,
        agent_id: String,
        pause: Box<PauseInfo>,
    },
}
