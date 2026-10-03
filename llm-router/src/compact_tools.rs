use nasiko_tool_compact::{
    decode_calls,
    encode_tools,
    ToolCall as CompactToolCall,
    ToolDef as CompactToolDef,
};

use crate::ir::{FunctionCall, FunctionDef, ToolCall, ToolDef};

/// Convert router tool definitions into the compact-tools representation.
pub fn encode_router_tools(
    tools: &[ToolDef],
) -> nasiko_tool_compact::Result<String> {
    let compact = tools
        .iter()
        .map(|tool| CompactToolDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        })
        .collect::<Vec<_>>();

    Ok(encode_tools(&compact)?.text)
}

/// Decode compact tool calls and convert them back into router IR.
pub fn decode_router_calls(
    text: &str,
    tools: &[ToolDef],
) -> nasiko_tool_compact::Result<Vec<ToolCall>> {
    let compact_tools = tools
        .iter()
        .map(|tool| CompactToolDef {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            parameters: tool.function.parameters.clone(),
        })
        .collect::<Vec<_>>();

    let calls = decode_calls(text, &compact_tools)?;

    Ok(calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| router_tool_call(index, call))
        .collect())
}

fn router_tool_call(index: usize, call: CompactToolCall) -> ToolCall {
    ToolCall {
        id: format!("compact-call-{index}"),
        kind: "function".to_string(),
        function: FunctionCall {
            name: call.name,
            arguments: serde_json::to_string(&call.arguments)
                .expect("serde_json::Value always serializes"),
        },
        extra: Default::default(),
    }
}

/// Check whether opt-in compact tool mode is enabled.
pub fn compact_tools_enabled() -> bool {
    std::env::var("NASIKO_COMPACT_TOOLS")
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Encode the tools from a router request into compact format.
pub fn compact_request(
    request: &crate::ir::ChatRequest,
) -> nasiko_tool_compact::Result<String> {
    let tools = request.tools.as_deref().unwrap_or_default();

    encode_router_tools(tools)
}

/// Return the compact tool prompt when compact mode is enabled.
pub fn compact_tool_prompt(
    request: &crate::ir::ChatRequest,
) -> nasiko_tool_compact::Result<Option<String>> {
    if !compact_tools_enabled() {
        return Ok(None);
    }

    let Some(tools) = request.tools.as_deref() else {
        return Ok(None);
    };

    if tools.is_empty() {
        return Ok(None);
    }

    Ok(Some(encode_router_tools(tools)?))
}

/// Decode a complete compact-tool response back into router IR.
pub fn decode_compact_response(
    text: &str,
    tools: &[ToolDef],
) -> nasiko_tool_compact::Result<Vec<ToolCall>> {
    decode_router_calls(text, tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn weather_tool() -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "get_weather".to_string(),
                description: Some("Get current weather.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string"
                        }
                    },
                    "required": ["city"]
                })),
            },
            extra: Default::default(),
        }
    }

    #[test]
    fn router_tools_encode_to_compact_format() {
        let encoded = encode_router_tools(&[weather_tool()]).unwrap();

        assert!(encoded.contains("get_weather"));
        assert!(encoded.contains("city:str"));
    }

    #[test]
    fn compact_calls_decode_to_router_calls() {
        let tools = vec![weather_tool()];

        let calls =
            decode_router_calls(
                r#"<<call get_weather {"city":"Hyderabad"}>>"#,
                &tools,
            )
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(
            calls[0].function.arguments,
            r#"{"city":"Hyderabad"}"#
        );
        assert_eq!(calls[0].id, "compact-call-0");
    }
}