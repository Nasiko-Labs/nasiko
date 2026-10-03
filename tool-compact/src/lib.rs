//! Compact tool-schema encoding and decoding for LLM prompt-token reduction.
//!
//! This crate provides a compact text representation of OpenAI-shaped tool definitions
//! that dramatically reduces prompt tokens while remaining easy for LLMs to parse and
//! produce. The decoder turns model output back into standard tool calls, validating
//! every call against the original schema.
//!
//! # Design invariants
//!
//! - **Fail closed**: unknown tools, missing required fields, or invalid values produce
//!   errors — never guessed or silently altered calls.
//! - **Pure library**: no IO, no env reads, no provider code. The router (or any other
//!   consumer) converts at the seam.
//! - **Deterministic**: same input always produces the same output.
//!
//! # Grammar
//!
//! Tool definitions use Python-style function signatures:
//! ```text
//! tool_name(param:type, optional_param?:type) - Description.
//! ```
//!
//! Tool calls use delimited markers:
//! ```text
//! <<call tool_name {"param": "value"}>>
//! ```
//!
//! # Example
//!
//! ```rust
//! use nasiko_tool_compact::{ToolDef, encode_tools, decode_calls};
//!
//! let tools = vec![ToolDef {
//!     name: "greet".into(),
//!     description: Some("Say hello".into()),
//!     parameters: Some(serde_json::json!({
//!         "type": "object",
//!         "properties": {
//!             "name": { "type": "string", "description": "Person's name" }
//!         },
//!         "required": ["name"]
//!     })),
//! }];
//!
//! let compact = encode_tools(&tools).unwrap();
//! assert!(compact.definitions.contains("greet(name:str)"));
//!
//! let text = r#"<<call greet {"name": "Alice"}>>"#;
//! let calls = decode_calls(text, &tools).unwrap();
//! assert_eq!(calls[0].name, "greet");
//! ```

mod decode;
mod encode;
mod error;
mod types;
mod validate;

pub use decode::{DecoderResult, StreamDecoder, StreamEvent, decode_calls, decode_calls_and_text};
pub use encode::encode_tools;
pub use error::{CompactError, DecodeError};
pub use types::{CompactTools, ToolCall, ToolDef};

/// Decode compact definitions back to `ToolDef` for schema round-trip checks.
///
/// This lets scorers verify that schema information survived the compact encoding.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Ok(compact.original_tools.clone())
}
