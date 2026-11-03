//! The canonical internal representation (IR) — OpenAI shape — that every inbound
//! format normalizes to and every provider renders from. See `RUST_PLAN_V1.md` §1.

pub mod chat;
pub mod embeddings;
pub mod parts;

pub use chat::{
    CACHE_CONTROL, CacheCreationUsage, ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice,
    Delta, FunctionCall, FunctionCallDelta, FunctionDef, Message, ToolCall, ToolCallDelta, ToolDef,
    Usage,
};
pub use embeddings::{Embedding, EmbeddingsRequest, EmbeddingsResponse};
pub use parts::{
    ContentPart, ContentView, DataSource, UnsupportedPart, parts_to_content_value, split_data_url,
};
