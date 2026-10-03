//! Shared types for the compact-tools library.
//!
//! Independent of `nasiko-llm-router` IR. The router converts at the seam.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition (name + optional description + JSON Schema parameters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for arguments (`type: "object"`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call. `arguments` is a parsed JSON value (object).
///
/// The router converts to OpenAI IR by serializing `arguments` to a JSON string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Result of [`crate::encode_tools`]: prompt text plus structured schemas for round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Text injected into the model prompt (definitions + call-format instructions).
    pub prompt: String,
    /// Structured schemas preserved for [`crate::decode_tools`] / validation.
    ///
    /// Tool `name` fields are the **model-facing compact aliases** (not necessarily
    /// the original gateway / OpenAI tool names).
    pub(crate) tools: Vec<ToolDef>,
    /// Exact `compact_alias → original_tool_name` map for execution identity.
    /// Identity aliases (`ping → ping`) are included so resolve is total.
    pub(crate) original_by_alias: HashMap<String, String>,
}

impl CompactTools {
    /// The prompt fragment to inject (definitions + call-format instructions).
    pub fn as_str(&self) -> &str {
        &self.prompt
    }

    /// Tools that were successfully compacted (names are compact aliases).
    pub fn tools(&self) -> &[ToolDef] {
        &self.tools
    }

    /// Resolve a model-facing compact alias to the original tool name.
    ///
    /// Returns `None` for unknown aliases (fail closed — never guess).
    pub fn resolve_original_name(&self, alias: &str) -> Option<&str> {
        self.original_by_alias.get(alias).map(String::as_str)
    }

    /// Exact alias → original map retained for the request.
    pub fn original_by_alias(&self) -> &HashMap<String, String> {
        &self.original_by_alias
    }
}
