//! A deterministic, schema-aware compact protocol for function tools.
//!
//! Tool declarations use `name(field!:type, optional?:type)~"description"`.
//! Types are `str`, `int`, `num`, `bool`, `null`, `[type]`, and
//! `obj{field!:type}`. `str@date-time` retains a format hint; `str{"a","b"}`
//! retains an enum. `|null` denotes a nullable type. `obj!{...}` means
//! `additionalProperties: false`; `obj+{...}` means explicit `true`.
//! Descriptions follow a type or declaration as `~"JSON-escaped text"`.
//! Calls are `<<call name {"argument":"value"}>>`. Arguments remain JSON.

mod decoder;
mod schema;

pub use decoder::{StreamDecoder, decode_calls, render_call, strip_calls};
pub use schema::{decode_tools, encode_tools, validate_arguments};

use serde_json::Value;

/// Router-independent function definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// A decoded call, with parsed arguments and no generated router-specific ID.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// The exact text sent to a model. No hidden schema copy is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompactError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("malformed call: {0}")]
    MalformedCall(String),
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),
    #[error("invalid definition: {0}")]
    InvalidDefinition(String),
}

pub type Result<T> = std::result::Result<T, CompactError>;
