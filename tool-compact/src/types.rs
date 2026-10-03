//! Plain tool types. They mirror the router's IR shapes but are owned here, so this crate does
//! not depend on `nasiko-llm-router`; the router converts at its seam.

use serde_json::{Map, Value};

/// A function tool as a client declares it (OpenAI `tools[].function`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// Function name, `[A-Za-z0-9_-]{1,64}`.
    pub name: String,
    /// Free-text description shown to the model.
    pub description: Option<String>,
    /// JSON Schema of the arguments object. `None` means the tool takes no arguments.
    pub parameters: Option<Value>,
}

/// A decoded, schema-valid tool call. The router assigns the call `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Name of a tool from the request.
    pub name: String,
    /// The arguments object, exactly as the model wrote it.
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    /// Arguments as the JSON string OpenAI puts in `function.arguments`.
    pub fn arguments_json(&self) -> String {
        Value::Object(self.arguments.clone()).to_string()
    }
}
