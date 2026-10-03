#![forbid(unsafe_code)]
#![allow(dead_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod definitions;
mod encode;
mod error;
mod format;
mod json;
mod render;
mod schema;
mod stream;
mod types;

pub use definitions::{decode, decode_calls, decode_definitions, decode_tools};
pub use encode::encode_tools;
pub use error::{DecodeError, DefinitionError, EncodeError};
pub use format::{legend_for_definitions, prompt_for_definitions};
pub use render::render_calls;
pub use stream::StreamDecoder;
pub use types::{CompactTools, Decoded, StreamEvent, ToolCall, ToolDef};

pub const MAX_CALL_BYTES: usize = 1 << 20;
