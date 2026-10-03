//! Lossless tool-schema encoding and fail-closed compact call decoding.
//!
//! No IO, environment access, provider code or router dependency. See README.md
//! for the supported JSON Schema subset, wire grammar and resource limits.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

mod schema;
mod stream;
mod strict_json;
mod wire;

pub use stream::StreamDecoder;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TOOLS: usize = 128;
pub const MAX_CALLS: usize = 128;

fn function_kind() -> String {
    "function".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

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

/// Validated call. The router assigns its own id and serializes arguments to a
/// JSON string at the IR seam; the eval uses this name/arguments object directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Transmitted definitions, without a hidden copy of the original schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    pub definitions: String,
}

pub const INSTRUCTIONS: &str =
    "Tools (!required). Call with <<call NAME {JSON arguments}>>, or answer normally.";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unsupported_schema: {0}")]
    UnsupportedSchema(String),
    #[error("unknown_tool")]
    UnknownTool,
    #[error("invalid_arguments")]
    InvalidArguments,
    #[error("malformed_output")]
    MalformedOutput,
    #[error("resource_limit")]
    ResourceLimit,
    #[error("decoder_finished")]
    Finished,
}
impl CompactError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedOutput => "malformed_output",
            Self::ResourceLimit => "resource_limit",
            Self::Finished => "decoder_finished",
        }
    }
}
pub type Result<T> = std::result::Result<T, CompactError>;

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

pub(crate) fn check_tools(tools: &[ToolDef]) -> Result<()> {
    if tools.len() > MAX_TOOLS {
        return Err(CompactError::ResourceLimit);
    }
    let mut seen = std::collections::BTreeSet::new();
    for tool in tools {
        if tool.kind != "function"
            || !tool.extra.is_empty()
            || !tool.function.extra.is_empty()
            || !valid_name(&tool.function.name)
            || !seen.insert(&tool.function.name)
        {
            return Err(CompactError::UnsupportedSchema("tool definition".into()));
        }
        if let Some(schema) = &tool.function.parameters {
            schema::check(schema, 0)?;
            if schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err(CompactError::UnsupportedSchema(
                    "arguments must be object".into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    check_tools(tools)?;
    let mut definitions = String::new();
    for t in tools {
        let desc = serde_json::to_string(&t.function.description)
            .map_err(|_| CompactError::MalformedOutput)?;
        let shape = t
            .function
            .parameters
            .as_ref()
            .map(wire::encode)
            .unwrap_or_else(|| "none".into());
        definitions.push_str(&format!("{} {} {}\n", t.function.name, desc, shape));
    }
    if definitions.len() > MAX_BYTES {
        return Err(CompactError::ResourceLimit);
    }
    Ok(CompactTools { definitions })
}

/// Reconstruct schemas from the actual wire definitions, not saved originals.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    if compact.definitions.len() > MAX_BYTES {
        return Err(CompactError::ResourceLimit);
    }
    let mut parser = wire::Parser::new(&compact.definitions);
    let mut tools = Vec::new();
    loop {
        parser.space();
        if parser.end() {
            break;
        }
        let name = parser.atom()?;
        parser.space();
        let desc = parser.json()?;
        let description = match desc {
            Value::Null => None,
            Value::String(s) => Some(s),
            _ => return Err(CompactError::MalformedOutput),
        };
        parser.space();
        let parameters = if parser.text[parser.pos..].starts_with("none") {
            parser.pos += 4;
            None
        } else {
            Some(parser.schema(0)?)
        };
        tools.push(ToolDef {
            kind: function_kind(),
            function: FunctionDef {
                name,
                description,
                parameters,
                extra: Map::new(),
            },
            extra: Map::new(),
        });
        if tools.len() > MAX_TOOLS {
            return Err(CompactError::ResourceLimit);
        }
        if !parser.end() && !parser.text.as_bytes()[parser.pos].is_ascii_whitespace() {
            return Err(CompactError::MalformedOutput);
        }
    }
    check_tools(&tools)?;
    Ok(tools)
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish()
}

/// Render typed calls for offline round-trip evaluation; never consults labels
/// or user text. Validation uses the same original schemas as model decoding.
pub fn render_calls(calls: &[ToolCall], tools: &[ToolDef]) -> Result<String> {
    check_tools(tools)?;
    if calls.len() > MAX_CALLS {
        return Err(CompactError::ResourceLimit);
    }
    let mut rendered = String::new();
    for call in calls {
        validate_call(call, tools)?;
        let args =
            serde_json::to_string(&call.arguments).map_err(|_| CompactError::InvalidArguments)?;
        rendered.push_str(&format!("<<call {} {}>>\n", call.name, args));
        if rendered.len() > MAX_BYTES {
            return Err(CompactError::ResourceLimit);
        }
    }
    Ok(rendered)
}

pub(crate) fn validate_call(call: &ToolCall, tools: &[ToolDef]) -> Result<()> {
    let tool = tools
        .iter()
        .find(|t| t.function.name == call.name)
        .ok_or(CompactError::UnknownTool)?;
    if !call.arguments.is_object() {
        return Err(CompactError::InvalidArguments);
    }
    if let Some(schema) = &tool.function.parameters {
        schema::validate(schema, &call.arguments, 0)?;
    }
    Ok(())
}
