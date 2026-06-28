//! The canonical internal representation (IR) — OpenAI shape — that every inbound
//! format normalizes to and every provider renders from. See `RUST_PLAN_V1.md` §1.

pub mod chat;
pub mod embeddings;

pub use chat::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, Delta, FunctionCall,
    FunctionCallDelta, FunctionDef, Message, ToolCall, ToolCallDelta, ToolDef, Usage,
};
pub use embeddings::{Embedding, EmbeddingsRequest, EmbeddingsResponse};
