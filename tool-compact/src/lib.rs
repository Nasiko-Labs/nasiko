mod decode;
mod encode;
mod error;
mod model;
mod stream;

pub use decode::decode_calls;
pub use encode::{encode_tools, render_compact_tools};
pub use error::Error;
pub use model::{CompactTools, FunctionDef, ToolCall, ToolDef};
pub use stream::StreamDecoder;
