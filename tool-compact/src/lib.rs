//! Pure, deterministic tool-schema compaction and validated call decoding.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod decode;
mod error;
mod report;
mod schema;
mod scope;
mod types;

pub use decode::StreamDecoder;
pub use error::{CompactError, Result};
pub use report::{
    BypassReason, OptimizationContext, OptimizationOutcome, OptimizationPlan, OptimizationReport,
    optimize_tools,
};
pub use schema::{CanonicalTool, Property, SchemaKind, SchemaNode, analyze_tools};
pub use scope::{ScopeDecision, ScopeInput, ScopeReason, select_tools};
pub use types::{CompactTools, FunctionDef, ToolCall, ToolDef};

/// Decode atomically: any malformed or invalid detected call fails the response.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish()
}

/// Render calls generically with JSON arguments, without implying validation.
pub fn render_calls(calls: &[ToolCall]) -> Result<String> {
    calls
        .iter()
        .map(|call| {
            if !schema::safe_identifier(&call.name) || !call.arguments.is_object() {
                return Err(CompactError::MalformedCall);
            }
            let arguments =
                serde_json::to_string(&call.arguments).map_err(|_| CompactError::MalformedCall)?;
            Ok(format!("<<call {} {arguments}>>", call.name))
        })
        .collect::<Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

/// Compile all tools, or return an error so the caller can use native tools.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let canonical = analyze_tools(tools)?;
    Ok(CompactTools {
        rendered: schema::render(&canonical)?,
    })
}

/// Reconstruct schemas from model-visible grammar, without original-schema storage.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    schema::parse_tools(&compact.rendered)
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
