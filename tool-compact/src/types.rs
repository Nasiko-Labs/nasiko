use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::instructions::{HEADER, INSTRUCTIONS};

/// One function tool as the caller sees it: the OpenAI `function` object without the wrapper.
///
/// `parameters` is the JSON Schema exactly as received. `None` (key absent) and `Some({})`
/// are different inputs and round-trip differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded, schema-validated call. `arguments` is always a JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// The compact form of a catalog: one line per tool plus the fixed call instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub(crate) definitions: String,
    pub(crate) tools: Vec<CompactTool>,
}

/// One tool's compact line, kept alongside its name so callers can attribute sizes per tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTool {
    pub name: String,
    pub line: String,
}

impl CompactTools {
    /// All tool lines, LF-joined, no trailing newline. This is what [`crate::decode_tools`]
    /// parses.
    pub fn definitions(&self) -> &str {
        &self.definitions
    }

    /// The fixed call-format instructions ([`INSTRUCTIONS`]), placed after the definitions.
    pub fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    /// The fixed header line ([`HEADER`]), placed before the definitions.
    pub fn header(&self) -> &'static str {
        HEADER
    }

    /// Per-tool lines in input order.
    pub fn tools(&self) -> &[CompactTool] {
        &self.tools
    }

    /// Header, definitions, then the call instructions: the text a caller puts in a system
    /// message.
    pub fn prompt(&self) -> String {
        let mut out =
            String::with_capacity(HEADER.len() + INSTRUCTIONS.len() + 2 + self.definitions.len());
        out.push_str(HEADER);
        out.push('\n');
        out.push_str(&self.definitions);
        out.push('\n');
        out.push_str(INSTRUCTIONS);
        out
    }
}

/// Result of decoding one reply: the prose outside the calls, exactly as written, and the calls.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Decoded {
    pub content: String,
    pub calls: Vec<ToolCall>,
}
