use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// Original schemas are retained for exact argument validation.
    pub tools: Vec<ToolDef>,

    /// Fast name lookup used by the decoder.
    pub lookup: HashMap<String, usize>,

    /// Compact representation sent to the model.
    pub rendered: String,
}
