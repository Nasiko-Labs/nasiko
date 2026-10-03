//! Decode model response text into OpenAI-shaped `ToolCall` structs.

use serde_json::Value;

use crate::error::CompactError;
use crate::stream::StreamDecoder;
use crate::types::{FunctionCall, ToolCall, ToolDef};
use crate::validate::validate_call;

/// Decode compact call markers (`<<call name {json}>>`) from text into standard
/// OpenAI-shaped `ToolCall` structs, validated against `tools`.
///
/// Returns typed errors (`UnknownTool`, `InvalidArguments`, `MalformedOutput`).
/// Tool call IDs are assigned sequentially as `call_1`, `call_2`, ...
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new();
    decoder.push(text);
    let raw_calls = decoder.finish()?;

    let mut tool_calls = Vec::with_capacity(raw_calls.len());

    for (idx, raw) in raw_calls.into_iter().enumerate() {
        let parsed_args: Value =
            serde_json::from_str(&raw.args).map_err(|e| CompactError::InvalidArguments {
                tool: raw.name.clone(),
                details: format!("JSON parse error: {e}"),
            })?;

        validate_call(&raw.name, &parsed_args, tools)?;

        // Standardize arguments back to compact JSON string
        let args_str =
            serde_json::to_string(&parsed_args).map_err(|e| CompactError::InvalidArguments {
                tool: raw.name.clone(),
                details: format!("JSON serialization error: {e}"),
            })?;

        tool_calls.push(ToolCall {
            id: format!("call_{}", idx + 1),
            kind: "function".to_string(),
            function: FunctionCall {
                name: raw.name,
                arguments: args_str,
            },
            extra: serde_json::Map::new(),
        });
    }

    Ok(tool_calls)
}
