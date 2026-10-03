//! # nasiko-tool-compact
//!
//! Compact tool schema encoding and model call decoding for LLM tool invocation.
//!
//! This crate provides:
//! 1. **Tool Encoding**: Compacting canonical tool definitions into concise signatures
//!    with explicit calling instructions (`<<call name {json args}>>`), reducing prompt tokens.
//! 2. **Batch Call Decoding**: Parsing model text containing compact calls into canonical
//!    OpenAI-shaped `ToolCall` objects, validating arguments against original JSON Schemas.
//! 3. **Streaming Decoding**: Incremental stream decoding that handles split chunks and
//!    delimiters across arbitrary SSE token boundaries.
//!
//! Designed as an independent, zero-I/O library crate.

#![forbid(unsafe_code)]

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod stream;
pub mod types;
pub mod validator;

pub use decoder::decode_calls;
pub use encoder::{CALL_INSTRUCTION, decode_tools, encode_tools};
pub use error::ToolCompactError;
pub use stream::{StreamChunkResult, StreamDecoder, StreamFinished};
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};
pub use validator::validate_call_arguments;
