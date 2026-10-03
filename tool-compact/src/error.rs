/// Typed failures; no argument payloads appear in error messages.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactError {
    /// Caller must use native tools for the entire request.
    #[error("unsupported schema keyword {keyword} at {path} for {tool}")]
    UnsupportedSchema {
        tool: String,
        path: String,
        keyword: String,
    },
    /// Invalid schema shape or duplicate tool name.
    #[error("invalid schema for {tool}: {reason}")]
    InvalidSchema { tool: String, reason: String },
    /// A detected call does not follow the grammar.
    #[error("malformed compact call")]
    MalformedCall,
    /// Stream ended during a detected call.
    #[error("incomplete compact call")]
    IncompleteCall,
    /// A function was not in the supplied tool set.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// Schema validation failed at an argument path.
    #[error("invalid arguments for {tool} at {path}: {reason}")]
    InvalidArguments {
        tool: String,
        path: String,
        reason: String,
    },
    /// Input exceeded a documented resource bound.
    #[error("resource limit exceeded: {0}")]
    LimitExceeded(&'static str),
    /// Compact schema text could not be reconstructed.
    #[error("invalid compact schema grammar")]
    InvalidGrammar,
}

/// Result type used throughout the library.
pub type Result<T> = std::result::Result<T, CompactError>;
