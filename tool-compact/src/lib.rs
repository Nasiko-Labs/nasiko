//! High-density Token-Oriented Object Notation (TOON) for LLM tool calling.

pub mod decode;
pub mod encode;
pub mod reverse;
pub mod stream;
pub mod types;
pub mod validate;

pub use decode::decode_calls;
pub use encode::encode_tools;
pub use reverse::decode_tools;
pub use stream::StreamDecoder;
pub use types::{
    CompactTools, DecodeError, EncodeError, FunctionCall, FunctionCallDelta, FunctionDef,
    ToolCall, ToolCallDelta, ToolDef,
};
pub use validate::{validate_call, validate_json_schema};
