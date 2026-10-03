//! Compact tool schemas for LLM prompts, plus a fail-closed decoder for the
//! compact call format.
//!
//! Pure library: no IO, no env reads, no provider code. It does not depend on
//! `nasiko-llm-router`; the router converts its own types at the seam.
//!
//! # Definition grammar (what the model reads)
//!
//! ```text
//! tool     := NAME "(" [param ("," param)*] ")" [" - " DESCRIPTION]
//! param    := PNAME ["?"] ":" type            ; "?" = optional
//! type     := "str" | "int" | "num" | "bool" | "null" | "datetime" | "date"
//!           | "obj"                           ; free-form object
//!           | "[" type "]"                    ; array
//!           | "{" [param ("," param)*] "}"    ; nested object
//!           | enumval ("|" enumval)+ | enumval ; enum (bare word, number, or JSON string)
//! ```
//! Per-parameter descriptions follow the signature as `  path: text` lines
//! (`a.b` for nested fields, `a[]` for array items).
//!
//! # Call grammar (what the model writes)
//!
//! ```text
//! call := "<<call" WS+ NAME [WS+ JSON_OBJECT] WS* ">>"
//! ```
//! `JSON_OBJECT` is parsed with a real JSON parser, so `>>`, braces and quotes
//! inside strings are safe. Everything outside calls is plain text.
//!
//! # Unsupported schema features (encoder returns `Err`; callers bypass compaction)
//! `anyOf/oneOf/allOf/$ref/not`, numeric/string constraints (`minimum`, `pattern`,
//! `maxLength`, ...), `default`, `format` other than `date-time`/`date`,
//! multi-type `type` arrays, arrays without `items`, `additionalProperties` other
//! than `false` (or `true` on a property-less nested object).
//!
//! # Fail-closed decoding
//! Unknown tool, invalid JSON, a missing required field, a wrong type, an enum
//! violation or an unknown field is an error; a call is never guessed or repaired.

mod decode;
mod encode;
mod validate;

pub use decode::{decode_calls, DecodeEvent, StreamDecoder, MAX_CALL_BYTES};
pub use encode::{encode_tools, encode_tools_with, CompactTools, EncodeOptions};

use serde_json::Value;
use thiserror::Error;

/// A tool definition (router-independent).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments (`type: object`). `None` means no arguments.
    pub parameters: Option<Value>,
}

/// A decoded tool call. The router assigns the call id.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    /// Arguments as a JSON string (OpenAI `function.arguments` shape).
    pub fn arguments_json(&self) -> String {
        self.arguments.to_string()
    }
}

/// Render a call in the compact call format (inverse of decoding).
pub fn render_call(call: &ToolCall) -> String {
    format!("<<call {} {}>>", call.name, call.arguments)
}

/// Encoding failures. Callers should bypass compaction on any of these.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum EncodeError {
    #[error("invalid tool name `{0}`")]
    InvalidToolName(String),
    #[error("duplicate tool name `{0}`")]
    DuplicateTool(String),
    #[error("unsupported schema feature at `{path}`: {reason}")]
    Unsupported { path: String, reason: String },
    #[error("malformed schema at `{path}`: {reason}")]
    MalformedSchema { path: String, reason: String },
}

/// Decoding failures.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum DecodeError {
    #[error("malformed call: {0}")]
    Malformed(String),
    #[error("arguments are not valid JSON: {0}")]
    InvalidJson(String),
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    #[error("invalid arguments for `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },
    #[error("incomplete stream: {0}")]
    IncompleteStream(String),
    #[error("tool call exceeds {0} bytes")]
    TooLarge(usize),
}

pub(crate) fn is_valid_tool_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(is_name_char)
}

pub(crate) fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::ToolDef;
    use serde_json::json;

    pub fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "ping".into(),
                description: None,
                parameters: None,
            },
        ]
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;
    use crate::testutil::tools;
    use serde_json::json;

    #[test]
    fn render_then_decode_is_semantically_identical() {
        let calls = vec![
            ToolCall {
                name: "create_calendar_event".into(),
                arguments: json!({"title":"Say \"hi\" }>> {x}","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"],"duration_min":30,"visibility":"private"}),
            },
            ToolCall {
                name: "ping".into(),
                arguments: json!({}),
            },
        ];
        let text = calls.iter().map(render_call).collect::<Vec<_>>().join("\n");
        assert_eq!(decode_calls(&text, &tools()).unwrap(), calls);
    }

    #[test]
    fn encode_is_deterministic() {
        assert_eq!(
            encode_tools(&tools()).unwrap(),
            encode_tools(&tools()).unwrap()
        );
    }
}
