//! `nasiko-tool-compact` — a compact wire format for LLM tool definitions, plus a
//! fail-closed decoder that turns a model's compact replies back into standard tool calls.
//!
//! ## Why
//!
//! Native (OpenAI-shaped) tool definitions are verbose JSON Schema: every request re-sends
//! `{"type":"function","function":{"name":...,"parameters":{"type":"object","properties":
//! {...}}}}` for every tool. That is prompt tokens spent on structure the model does not need
//! spelled out in full. This crate renders the same tools as a terse one-line-per-tool
//! signature and teaches the model a small call grammar, then decodes what it writes back.
//!
//! ## Grammar (explicit, versioned)
//!
//! A compact tool definition is one line:
//!
//! ```text
//! name(arg:type, opt?:type, choice?:a|b) - description
//! ```
//!
//! * `?` after an argument name marks it **optional**; no `?` means **required**.
//! * `type` is one of `str`, `int`, `float`, `bool`, `datetime`, `[elem]` (array of `elem`),
//!   `obj` (nested object), or an inline enum `a|b|c`.
//! * The trailing ` - description` is present only when the tool has a description.
//!
//! A tool **call** is a marker the model emits inline, anywhere in its reply:
//!
//! ```text
//! <<call name {"arg":"value"}>>
//! ```
//!
//! The payload between the braces is a normal JSON object. Multiple markers = multiple calls.
//! Text before, between, or after markers is ignored. `>>` inside a JSON string argument is
//! safe: the decoder matches the JSON object by brace depth with full string/escape awareness,
//! so the closing `>>` is only recognised after the object is balanced.
//!
//! ## Fail-closed
//!
//! Decoding **validates every call against the original schema**. An unknown tool, a missing
//! required argument, a wrong type, or a value outside an `enum` returns an [`CompactError`],
//! never a guessed or silently-altered call.
//!
//! ## Boundaries (what this crate is not)
//!
//! Pure library: no IO, no env reads, no provider code, and **no dependency on the router**.
//! It defines its own [`ToolDef`] / [`ToolCall`] so the router can depend on it and convert at
//! the seam. Unsupported JSON Schema features are documented on [`encode_tools`]; the router is
//! expected to bypass compaction for tools that use them rather than lose meaning.

mod decode;
mod encode;
mod stream;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use decode::{decode_calls, decode_tools};
pub use encode::encode_tools;
pub use stream::StreamDecoder;

/// A tool the model may call — this crate's own type, intentionally decoupled from the
/// router's `ToolDef` so the dependency arrow points router → crate, never the reverse.
///
/// `parameters` is a JSON Schema object (the same shape OpenAI uses under
/// `function.parameters`). `None` means the tool takes no arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call. `arguments` is a JSON **object** (`Value::Object`); the router
/// stringifies it when it builds the OpenAI-shaped `function.arguments` at the seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// The compact rendering of a tool set: the per-tool signature block plus the call-format
/// instructions the model needs to write calls back. [`CompactTools::render`] joins them into
/// the single system-message string a caller injects in place of native tool definitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// One compact signature per tool, newline-separated, in input order.
    pub tools_block: String,
    /// Fixed grammar instructions telling the model how to emit `<<call ...>>` markers.
    pub instructions: String,
}

impl CompactTools {
    /// The full system-message text: the call instructions followed by a blank line and the
    /// tool signatures. This is what a caller injects instead of native `tools`.
    pub fn render(&self) -> String {
        format!("{}\n\n{}", self.instructions, self.tools_block)
    }
}

/// A `Result` whose error is always a [`CompactError`].
pub type Result<T> = std::result::Result<T, CompactError>;

/// Every way encoding or (fail-closed) decoding can refuse to produce a call.
///
/// The screening harness maps the two decode-time failures to stable wire strings:
/// [`CompactError::UnknownTool`] → `"unknown_tool"`, everything argument-related →
/// `"invalid_arguments"`. [`CompactError::as_wire`] performs that mapping.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactError {
    /// A `<<call name ...>>` named a tool not in the provided set.
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    /// The call's JSON payload would not parse as a JSON object.
    #[error("malformed arguments: {0}")]
    MalformedArguments(String),

    /// The arguments parsed but violated the tool's schema (missing required field, wrong
    /// type, or a value outside an `enum`).
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// A tool's `parameters` schema was itself malformed and could not be rendered/validated.
    #[error("invalid schema for {tool}: {reason}")]
    InvalidSchema { tool: String, reason: String },

    /// A `<<call ...>>` marker was structurally broken (no name, or no JSON object body).
    #[error("malformed call marker: {0}")]
    MalformedMarker(String),
}

impl CompactError {
    /// The stable wire label the screening harness expects for a decoder error: either
    /// `"unknown_tool"` or `"invalid_arguments"`. Structural/marker/schema errors all fold
    /// into `"invalid_arguments"` — from the client's perspective the model's output could not
    /// be turned into a valid call.
    pub fn as_wire(&self) -> &'static str {
        match self {
            CompactError::UnknownTool(_) => "unknown_tool",
            _ => "invalid_arguments",
        }
    }
}
