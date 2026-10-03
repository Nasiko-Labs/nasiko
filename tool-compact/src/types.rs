use serde_json::Value;

/// A tool definition, equivalent to the router's `ToolDef.function`.
///
/// Defined here so this crate does not depend on `nasiko-llm-router`; the
/// router converts at the seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments object.
    pub parameters: Option<Value>,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self { name: name.into(), description, parameters }
    }
}

/// A decoded tool call. `arguments` is a JSON object serialized to a string,
/// matching the OpenAI wire shape. The router assigns the call `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// The compact rendering of a tool set, ready to inject into a system message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// The `TOOLS` block: one signature line per tool plus argument notes.
    pub tools_block: String,
    /// Instructions telling the model how to emit a call.
    pub instructions: String,
}

impl CompactTools {
    /// The full text to inject, tools block followed by call instructions.
    pub fn render(&self) -> String {
        format!("{}\n\n{}", self.tools_block, self.instructions)
    }
}
