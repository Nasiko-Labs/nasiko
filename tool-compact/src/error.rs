//! Error types for compact tool encoding, decoding, validation, and streaming.

use thiserror::Error;

/// Errors that can occur during tool definition encoding, invocation decoding, or validation.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum CompactToolError {
    /// A tool schema contains a JSON Schema feature that cannot be represented losslessly.
    #[error("Tool '{tool}' uses unsupported JSON schema feature: {feature}")]
    UnsupportedSchemaFeature { tool: String, feature: &'static str },

    /// A syntax error occurred in the input text stream/document at the specified byte offset.
    #[error("Syntax error at offset {offset}: {reason}")]
    SyntaxError { offset: usize, reason: String },

    /// The JSON arguments payload inside a tool call is malformed or invalid JSON.
    #[error("Malformed JSON in tool call '{tool_name}': {details}")]
    MalformedJson { tool_name: String, details: String },

    /// A tool invocation referenced a tool name that is not in the schema registry.
    #[error("Unknown tool '{tool_name}' invoked")]
    UnknownTool { tool_name: String },

    /// A required argument was missing from the tool invocation arguments.
    #[error("Missing required argument '{field}' for tool '{tool_name}'")]
    MissingRequiredField { tool_name: String, field: String },

    /// An argument was provided with an incompatible type.
    #[error(
        "Invalid type for field '{field}' on tool '{tool_name}': expected {expected}, found {found}"
    )]
    InvalidFieldType {
        tool_name: String,
        field: String,
        expected: &'static str,
        found: String,
    },

    /// An argument with an enum constraint received a value not in the allowed variants list.
    #[error(
        "Invalid enum value for field '{field}' on tool '{tool_name}': expected one of {expected:?}, found '{found}'"
    )]
    InvalidEnumValue {
        tool_name: String,
        field: String,
        expected: Vec<String>,
        found: String,
    },

    /// An unexpected argument was provided to a tool with `additionalProperties: false`.
    #[error("Unexpected argument '{field}' for tool '{tool_name}'")]
    UnexpectedField { tool_name: String, field: String },

    /// The input text or stream ended in the middle of an unclosed tool call marker.
    #[error("Stream finished with unclosed tool call marker")]
    UnclosedCallMarker,
}
