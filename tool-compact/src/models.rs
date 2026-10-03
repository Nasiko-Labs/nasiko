//! The crate's own tool and call types. Deliberately independent of `nasiko-llm-router`'s IR:
//! the router depends on this crate and converts at its seam, never the reverse.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::encode::INSTRUCTION;

/// A function tool as an OpenAI-compatible client declares it.
///
/// (De)serializes as the wire shape `{"type":"function","function":{…}}`. Function-level fields
/// this crate does not model are kept in [`ToolDef::extra`] rather than dropped, so
/// [`crate::encode_tools`] can refuse to compact a tool it would otherwise silently strip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WireTool", into = "WireTool")]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema for the arguments. `None` means the tool declares no parameters.
    pub parameters: Option<Value>,
    /// OpenAI's `function.strict`.
    pub strict: Option<bool>,
    /// Function-level fields not modelled above.
    pub extra: Map<String, Value>,
}

impl ToolDef {
    /// A plain function tool with no `strict` flag and no extra fields.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
            strict: None,
            extra: Map::new(),
        }
    }
}

/// One decoded tool call. `arguments` has already been validated against the tool's schema.
///
/// The OpenAI `id` is not part of this type: assigning ids is the router's job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    /// The arguments as the JSON string OpenAI's `function.arguments` carries.
    pub fn arguments_json(&self) -> String {
        Value::Object(self.arguments.clone()).to_string()
    }
}

/// Compact definitions for a set of tools, produced by [`crate::encode_tools`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    definitions: String,
}

impl CompactTools {
    /// Wrap definitions text, e.g. one read back from a transcript, for [`crate::decode_tools`].
    pub fn from_definitions(definitions: impl Into<String>) -> Self {
        Self {
            definitions: definitions.into(),
        }
    }

    /// The tool definitions alone, one tool per header line.
    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    /// The call-format instruction followed by the definitions: the text to place in a system
    /// message instead of the native `tools` array.
    pub fn system_prompt(&self) -> String {
        format!("{INSTRUCTION}\n{}", self.definitions)
    }
}

/// A fully decoded reply: the text the model wrote around its calls, and the calls in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decoded {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

fn function_kind() -> String {
    "function".to_string()
}

#[derive(Serialize, Deserialize)]
struct WireTool {
    #[serde(rename = "type", default = "function_kind")]
    kind: String,
    function: WireFunction,
}

#[derive(Serialize, Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl TryFrom<WireTool> for ToolDef {
    type Error = String;

    fn try_from(wire: WireTool) -> Result<Self, Self::Error> {
        if wire.kind != "function" {
            return Err(format!("unsupported tool type `{}`", wire.kind));
        }
        let WireFunction {
            name,
            description,
            parameters,
            strict,
            extra,
        } = wire.function;
        Ok(Self {
            name,
            description,
            parameters,
            strict,
            extra,
        })
    }
}

impl From<ToolDef> for WireTool {
    fn from(tool: ToolDef) -> Self {
        Self {
            kind: function_kind(),
            function: WireFunction {
                name: tool.name,
                description: tool.description,
                parameters: tool.parameters,
                strict: tool.strict,
                extra: tool.extra,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_def_serde_roundtrips_the_openai_wrapper() {
        let wire = json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Look a thing up.",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}},
                "strict": true,
                "x-vendor": 1
            }
        });

        let tool: ToolDef = serde_json::from_value(wire.clone()).unwrap();

        assert_eq!(tool.name, "lookup");
        assert_eq!(tool.strict, Some(true));
        assert_eq!(
            tool.extra.get("x-vendor"),
            Some(&json!(1)),
            "unknown fields are kept"
        );
        assert_eq!(serde_json::to_value(&tool).unwrap(), wire);
    }

    #[test]
    fn non_function_tool_types_are_rejected_not_coerced() {
        let err = serde_json::from_value::<ToolDef>(json!({
            "type": "web_search",
            "function": {"name": "x"}
        }))
        .unwrap_err();
        assert!(err.to_string().contains("unsupported tool type"));
    }

    #[test]
    fn arguments_json_is_the_openai_string_form() {
        let call = ToolCall {
            name: "f".into(),
            arguments: json!({"a": 1, "b": "x"}).as_object().unwrap().clone(),
        };
        assert_eq!(call.arguments_json(), r#"{"a":1,"b":"x"}"#);
    }
}
