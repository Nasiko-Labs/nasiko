use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A function tool: this crate's own mirror of the router's `ToolDef`, so the crate never
/// depends on the router. The router converts at the seam.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded, schema-validated tool call. The router assigns the call `id`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    /// Arguments as a JSON string, the shape OpenAI's `function.arguments` carries.
    pub fn arguments_json(&self) -> String {
        Value::Object(self.arguments.clone()).to_string()
    }
}

/// The compact rendering of a tool list.
///
/// `definitions` holds one signature line per tool; `instructions` tells the model how to call
/// them. They are kept apart so [`crate::decode_tools`] can parse `definitions` back without the
/// prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub definitions: String,
    pub instructions: String,
}

impl CompactTools {
    /// The text injected into the request in place of the native `tools` field.
    pub fn prompt(&self) -> String {
        format!("{}\n{}", self.instructions, self.definitions)
    }
}

/// One unit of decoded model output, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Plain text outside any call. Adjacent text may arrive split across several events.
    Text(String),
    Call(ToolCall),
}

/// A fully decoded model reply.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decoded {
    /// All text outside calls, concatenated in order.
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl Decoded {
    pub(crate) fn from_events(events: impl IntoIterator<Item = Event>) -> Self {
        let mut out = Decoded::default();
        for event in events {
            match event {
                Event::Text(t) => out.text.push_str(&t),
                Event::Call(c) => out.calls.push(c),
            }
        }
        out
    }
}
