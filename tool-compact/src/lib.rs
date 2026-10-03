//! Compact tool schemas and a strict decoder for the calls a model writes back.
//!
//! Pure library: no IO, no environment reads, no provider code. Decoding validates every call
//! against the original JSON Schema and fails closed.

mod decode;
mod decode_tools;
mod encode;
mod render;
mod stream;
mod strict_json;
mod types;
mod validate;

pub use decode::decode_calls;
pub use decode_tools::decode_tools;
pub use encode::encode_tools;
pub use render::render_calls;
pub use stream::StreamDecoder;
pub use types::{Bypass, CompactTools, DecodeError, EncodeError, ToolCall, ToolDef};
pub use validate::validate_call;
