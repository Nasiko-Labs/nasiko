//! Pure, deterministic tool-schema compaction and validated call decoding.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod error;
mod schema;
mod types;

pub use error::{CompactError, Result};
pub use schema::{CanonicalTool, Property, SchemaKind, SchemaNode, analyze_tools};
pub use types::{CompactTools, FunctionDef, ToolCall, ToolDef};

/// Compile all tools, or return an error so the caller can use native tools.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let canonical = analyze_tools(tools)?;
    Ok(CompactTools {
        rendered: schema::render(&canonical)?,
    })
}

/// Validate a call without changing its name, arguments, or unknown keys.
pub fn validate_call(call: &ToolCall, tools: &[ToolDef]) -> Result<()> {
    let canonical = analyze_tools(tools)?;
    let tool = canonical
        .iter()
        .find(|tool| tool.name == call.name)
        .ok_or_else(|| CompactError::UnknownTool(call.name.clone()))?;
    schema::validate(tool, &call.arguments)
}
