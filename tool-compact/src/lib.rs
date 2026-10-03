#![forbid(unsafe_code)]

//! `nasiko-tool-compact` provides pure-Rust compact tool definitions and streaming decoders
//! for LLM function calling without token bloat.

pub mod decode;
pub mod encode;
pub mod error;
pub mod stream;
pub mod types;
pub mod validate;

pub use decode::{decode_calls, decode_tools, strip_calls};
pub use encode::{encode_tools, INSTRUCTION};
pub use error::ToolCompactError;
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};
pub use validate::validate_tool_call;
