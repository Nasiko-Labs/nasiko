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
}

impl CompactTools {
    pub(crate) fn new(definitions: String) -> Self {
        Self { definitions }
    }

    /// The minified compact JSON tool array.
    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    /// Definitions plus the shared schema legend and exact call grammar.
    pub fn prompt(&self) -> String {
        format!(
            "Tools (each is [name,description|null,schema|null]):\n{}\n{}",
            self.definitions, Self::instructions()
        )
    }

    /// Shared legend and call instructions used by every compact request.
    pub const fn instructions() -> &'static str {
        "Schema legend: t=type,p=properties,r=required,i=items,e=enum,d=description,\
f=format,a=additionalProperties; type values o=object,a=array,s=string,i=integer,\
n=number,b=boolean,0=null. Other schema keys are written as ~KEY. To call a tool, \
emit exactly <<call TOOL_NAME {JSON_OBJECT}>>. Emit one block per call and never alter \
tool or argument names. If no tool is needed, answer normally without a call block."
    }
}

