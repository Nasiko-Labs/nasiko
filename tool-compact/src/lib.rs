pub mod error;
pub mod grammar;
pub mod types;

// Group A modules (Schema & Encoder)
pub mod decode_tools;
pub mod encode;
pub mod schema;

// Group B modules (Decoder & Validator)
pub mod decode;
pub mod stream;
pub mod validate;

pub use error::{CompactError, Result};
pub use grammar::{CLOSE, OPEN, render_call};
pub use types::{CompactTools, ToolCall, ToolDef};

pub use decode_tools::decode_tools;
pub use encode::encode_tools;
pub use schema::is_schema_supported;

pub use decode::decode_calls;
pub use stream::{StreamDecoder, StreamEvent};
pub use validate::validate_arguments;
