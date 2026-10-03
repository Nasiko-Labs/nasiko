use nasiko_tool_compact::{CompactError, CompactTools, ToolCall, ToolDef};
use serde_json::Value;

#[derive(Debug, Default, cucumber::World)]
pub struct CompactWorld {
    pub tools: Vec<ToolDef>,
    pub sample: Option<Value>,
    pub compact: Option<CompactTools>,
    pub encode_error: Option<CompactError>,
    pub schemas: Option<Result<Vec<ToolDef>, CompactError>>,
    pub calls: Option<Result<Vec<ToolCall>, CompactError>>,
    pub reply: String,
    pub chunks: Vec<String>,
    pub chunk_emitted: Vec<usize>,
    pub stream: Option<Result<Vec<ToolCall>, CompactError>>,
    pub property_held: bool,
}
