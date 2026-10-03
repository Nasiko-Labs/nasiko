//! `nasiko-tool-compact` — Compact tool schema format for nasiko-llm-router.
//!
//! Converts verbose JSON tool definitions into a compact single-line signature
//! format that uses far fewer tokens, then decodes the model's `<<call...>>`
//! response back into a standard OpenAI tool call.
//!
//! # Invariants (matching nasiko-compress)
//! - **Fail-closed** — any decode error returns `Err`; no guessed calls.
//! - **Deterministic** — same input → same compact output, always.
//! - **Never grows** — compact form is always shorter than the original JSON.
//! - **Streaming-safe** — `<<call...>>` markers may arrive split across chunks.
//!
//! # Quick start
//! ```rust
//! use nasiko_tool_compact::{compact_tools, ToolDecoder, KnownTool};
//! use std::collections::HashMap;
//!
//! // 1. Compact the tool schemas
//! let tools_json = serde_json::json!([{
//!     "type": "function",
//!     "function": {
//!         "name": "send_email",
//!         "description": "Send an email.",
//!         "parameters": {
//!             "type": "object",
//!             "properties": {
//!                 "to":      {"type": "string"},
//!                 "subject": {"type": "string"}
//!             },
//!             "required": ["to", "subject"]
//!         }
//!     }
//! }]);
//!
//! let compact = compact_tools(tools_json.as_array().unwrap());
//! println!("{}", compact.prompt_block());
//! // send_email(to:str, subject:str) - Send an email.
//! // To call a tool, emit EXACTLY this format ...
//!
//! // 2. Decode the model's response
//! let known = vec![KnownTool {
//!     name: "send_email".into(),
//!     required: vec!["to".into(), "subject".into()],
//!     enum_fields: HashMap::new(),
//! }];
//!
//! let mut decoder = ToolDecoder::new();
//! let result = decoder.push(
//!     r#"<<call send_email {"to":"a@b.com","subject":"Hello"}>>"#,
//!     &known,
//! ).unwrap();
//! assert!(result.is_some());
//! ```

pub mod compactor;
pub mod decoder;
pub mod error;
pub mod schema;

pub use compactor::{compact_tools, CompactTools, CALL_INSTRUCTION};
pub use decoder::{DecodedCall, KnownTool, ToolDecoder};
pub use error::CompactToolError;
pub use schema::render_type;
