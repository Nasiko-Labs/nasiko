use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    pub fn arguments_json(&self) -> String {
        let value = Value::Object(self.arguments.clone());
        crate::json::to_compact_json_value(&value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    definitions: String,
    prompt: String,
}

impl CompactTools {
    pub fn new(definitions: String, prompt: String) -> Self {
        Self {
            definitions,
            prompt,
        }
    }

    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl Decoded {
    pub fn extend(events: Vec<crate::StreamEvent>) -> Self {
        let mut text = String::new();
        let mut calls = Vec::new();
        for event in events {
            match event {
                crate::StreamEvent::Text(part) => text.push_str(&part),
                crate::StreamEvent::Call { call, .. } => calls.push(call),
            }
        }
        Self { text, calls }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Text(String),
    Call { index: usize, call: ToolCall },
}
