//! Compact tool schema encoding and response decoding integration (Phase 1).

use nasiko_tool_compact::{decode_calls, encode_tools, CompactTools, DecodeResult, ToolDef};
use serde_json::{json, Value};

/// Helper to optionally compact tool definitions and inject them into messages.
/// 
/// Returns `true` if tool definitions were successfully encoded and injected 
/// as a system message; `false` if compaction was bypassed or failed.
pub fn apply_compact_tools(
    tools: &[ToolDef],
    messages: &mut Vec<Value>,
) -> bool {
    match encode_tools(tools) {
        Ok(CompactTools::Encoded { text }) => {
            // Push the compact tools instruction/definition block as a system message
            messages.push(json!({
                "role": "system",
                "content": text
            }));
            true
        }
        Ok(CompactTools::Bypassed { .. }) => false,
        Err(e) => {
            eprintln!("[Warning] Failed to encode compact tools: {:?}", e);
            false
        }
    }
}

/// Helper to extract and decode tool calls from the model's text response.
pub fn parse_compact_response(
    response_text: &str,
    tools: &[ToolDef],
) -> Option<DecodeResult> {
    let result = decode_calls(response_text, tools);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_compact_tools_bypass_empty() {
        let tools = vec![];
        let mut messages = vec![json!({
            "role": "user",
            "content": "Hello, world!"
        })];

        let applied = apply_compact_tools(&tools, &mut messages);
        
        // Should bypass when no tools are present
        assert!(!applied);
        assert_eq!(messages.len(), 1);
    }
}