//! Compact Tool Schemas (Track P1) for Nasiko.
//!
//! Provides schema compaction and fail-closed streaming call decoding for LLM tool calling.
//! Achieves >30% token reduction (typically ~50%) on JSON Schema definitions by rendering
//! concise type signatures while preserving strict validation and streaming parity.

pub mod decode;
pub mod encode;
pub mod error;
pub mod stream;
pub mod types;
pub mod validate;

pub use decode::{decode_calls, strip_call_markers};
pub use encode::{INSTRUCTION_PROMPT, encode_tools};
pub use error::{CompactError, CompactToolError, DecodeError};
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};
pub use validate::validate_call;
