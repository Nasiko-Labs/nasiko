use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}

/// A function tool definition in OpenAI-compatible shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The function portion of a tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A decoded call. Call ids belong to the router, not this codec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// A deterministic compact representation of a set of tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactTools {
    definitions: String,
    reconstruction: String,
}

impl CompactTools {
    pub(crate) fn new(definitions: String, reconstruction: String) -> Self {
        Self {
            definitions,
            reconstruction,
        }
    }

    /// The model-visible signature definitions.
    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    pub(crate) fn reconstruction(&self) -> &str {
        &self.reconstruction
    }

    /// Definitions plus the small signature legend and exact call grammar.
    pub fn prompt(&self) -> String {
        format!(
            "Tools ({}):\n{}\n{}",
            Self::legend(),
            self.definitions,
            Self::instructions(),
        )
    }

    /// The only notation in signatures that is not self-explanatory.
    pub const fn legend() -> &'static str {
        "? optional; \"...\" desc"
    }

    /// Call instructions used by every compact request.
    pub const fn instructions() -> &'static str {
        "Call <<call NAME {JSON}>>; repeat/omit; names exact."
    }
}
