use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum EncodeError {
    #[error("unsupported schema for tool {tool} at {path}: {reason}")]
    Unsupported {
        tool: String,
        path: String,
        reason: String,
    },
    #[error("invalid tool name: {0}")]
    InvalidName(String),
    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum DecodeError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("invalid arguments for tool {tool} at {path}: {reason}")]
    InvalidArguments {
        tool: String,
        path: String,
        reason: String,
    },
    #[error("malformed call: {reason}")]
    MalformedCall { reason: String },
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(#[from] EncodeError),
}

impl DecodeError {
    pub fn code(&self) -> &str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall { .. } => "malformed_call",
            Self::UnsupportedSchema(_) => "unsupported_schema",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("line {line}, column {column}: {reason}")]
pub struct DefinitionError {
    pub line: usize,
    pub column: usize,
    pub reason: String,
}
