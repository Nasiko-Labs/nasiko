use serde_json::{Map, Value, json};

use crate::error::{Error, Result};

/// A function tool: the parts of an OpenAI `{"type":"function","function":{..}}` entry this
/// crate can represent.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// Function name.
    pub name: String,
    /// What the tool does. Kept verbatim.
    pub description: Option<String>,
    /// JSON Schema for the arguments. `None` is treated as an object with no properties.
    pub parameters: Option<Value>,
}

impl ToolDef {
    /// Parse one OpenAI tool entry.
    ///
    /// Fails on any key this type cannot carry (for example `"strict": true`, or extra keys on
    /// the `function` object), so a lossy conversion can never happen silently. Treat an error as
    /// "send this tool list natively".
    pub fn from_openai(value: &Value) -> Result<Self> {
        let outer = value
            .as_object()
            .ok_or_else(|| Error::InvalidTools("tool entry is not an object".into()))?;
        for key in outer.keys() {
            if key != "type" && key != "function" {
                return Err(Error::InvalidTools(format!(
                    "unrepresentable tool key '{key}'"
                )));
            }
        }
        match outer.get("type") {
            None => {}
            Some(Value::String(kind)) if kind == "function" => {}
            Some(other) => {
                return Err(Error::InvalidTools(format!(
                    "tool type {other} is not \"function\""
                )));
            }
        }
        let function = outer
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::InvalidTools("missing 'function' object".into()))?;
        for key in function.keys() {
            if !matches!(key.as_str(), "name" | "description" | "parameters") {
                return Err(Error::InvalidTools(format!(
                    "unrepresentable function key '{key}'"
                )));
            }
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidTools("function.name must be a string".into()))?
            .to_string();
        let description = match function.get("description") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(Error::InvalidTools(format!(
                    "{name}: description must be a string"
                )));
            }
        };
        Ok(Self {
            name,
            description,
            parameters: function.get("parameters").cloned(),
        })
    }

    /// Render as an OpenAI tool entry (`{"type":"function","function":{..}}`).
    pub fn to_openai(&self) -> Value {
        let mut function = Map::new();
        function.insert("name".into(), Value::String(self.name.clone()));
        if let Some(description) = &self.description {
            function.insert("description".into(), Value::String(description.clone()));
        }
        if let Some(parameters) = &self.parameters {
            function.insert("parameters".into(), parameters.clone());
        }
        json!({ "type": "function", "function": function })
    }
}

/// A decoded, validated tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Name of a tool from the list the call was validated against.
    pub name: String,
    /// The arguments object, already validated against the tool's schema.
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    /// Arguments as a JSON string — the shape OpenAI's `function.arguments` expects.
    pub fn arguments_json(&self) -> String {
        Value::Object(self.arguments.clone()).to_string()
    }
}
