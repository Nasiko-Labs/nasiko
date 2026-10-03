//! A pure, deterministic tool codec. The host owns I/O, provider calls, and call IDs.
mod codec;
mod decode;
mod schema;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use codec::{decode_tools, encode_tools};
pub use decode::{DecodedResponse, StreamDecoder, decode_calls, decode_response};

/// Limits bound parsing work and request-scoped memory.
pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_DEPTH: usize = 32;
pub const MAX_CALLS: usize = 64;
pub const MAX_TOOLS: usize = 128;
pub const MAX_ITEMS: usize = 4096;

/// Original schema is authoritative; no argument coercion or repair is performed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Self-contained, reversible definitions; no hidden copy of the original schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    pub definitions: String,
}

impl CompactTools {
    /// Model-facing definitions without the storage-version line. The version is
    /// metadata, not an executable tool; removing it preserves every schema field.
    pub fn catalog(&self) -> Result<&str, CompactError> {
        self.definitions.strip_prefix("ct1\n").ok_or_else(|| {
            CompactError::MalformedOutput("unsupported compact schema version".into())
        })
    }
}

/// Kept short because these instructions are part of the measured request.
pub const CALL_INSTRUCTIONS: &str = "For each action emit <<call NAME {\"arg\":\"value\"}>>; omit unspecified optional args (?). Otherwise answer normally.\n";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactError {
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),
    #[error("invalid tool definitions: {0}")]
    InvalidTools(String),
    #[error("malformed output: {0}")]
    MalformedOutput(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("codec resource limit exceeded")]
    LimitExceeded,
}

impl CompactError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::InvalidTools(_) => "invalid_tools",
            Self::MalformedOutput(_) => "malformed_output",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::LimitExceeded => "limit_exceeded",
        }
    }
}

pub(crate) fn name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

pub(crate) fn check_tools(tools: &[ToolDef]) -> Result<(), CompactError> {
    if tools.len() > MAX_TOOLS {
        return Err(CompactError::LimitExceeded);
    }
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        // Bound arbitrary annotation values too, before recursive serialization.
        let mut work = vec![(&tool.parameters, 0usize)];
        let mut nodes = 0usize;
        while let Some((value, depth)) = work.pop() {
            nodes += 1;
            if depth > MAX_DEPTH || nodes > MAX_BYTES {
                return Err(CompactError::LimitExceeded);
            }
            match value {
                Value::Array(a) => {
                    if a.len() > MAX_ITEMS {
                        return Err(CompactError::LimitExceeded);
                    }
                    work.extend(a.iter().map(|v| (v, depth + 1)));
                }
                Value::Object(m) => work.extend(m.values().map(|v| (v, depth + 1))),
                _ => {}
            }
        }
        if tool.name.is_empty()
            || tool.name.len() > 128
            || !tool.name.bytes().all(name_char)
            || !names.insert(&tool.name)
        {
            return Err(CompactError::InvalidTools(
                "invalid or duplicate tool name".into(),
            ));
        }
        schema::check(&tool.parameters, 0)?;
        if tool.parameters.get("type").and_then(Value::as_str) != Some("object") {
            return Err(CompactError::UnsupportedSchema(
                "function arguments must be an object".into(),
            ));
        }
    }
    if serde_json::to_vec(tools)
        .map_err(|e| CompactError::InvalidTools(e.to_string()))?
        .len()
        > MAX_BYTES
    {
        return Err(CompactError::LimitExceeded);
    }
    Ok(())
}

pub(crate) fn validate_call(call: &ToolCall, tools: &[ToolDef]) -> Result<(), CompactError> {
    let tool = tools
        .iter()
        .find(|t| t.name == call.name)
        .ok_or_else(|| CompactError::UnknownTool(call.name.clone()))?;
    schema::validate(&tool.parameters, &call.arguments, 0)
        .map_err(|reason| CompactError::InvalidArguments(format!("{}: {reason}", call.name)))
}

/// Render expected calls for offline round-trip evaluation, never for live inference.
pub fn render_calls(calls: &[ToolCall], tools: &[ToolDef]) -> Result<String, CompactError> {
    check_tools(tools)?;
    if calls.len() > MAX_CALLS {
        return Err(CompactError::LimitExceeded);
    }
    let mut out = String::new();
    for call in calls {
        validate_call(call, tools)?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("<<");
        out.push_str(&call.name);
        out.push(' ');
        out.push_str(
            &serde_json::to_string(&call.arguments)
                .map_err(|e| CompactError::InvalidArguments(e.to_string()))?,
        );
        out.push_str(">>");
    }
    if out.len() > MAX_BYTES {
        return Err(CompactError::LimitExceeded);
    }
    Ok(out)
}
