//! Render tool calls in the compact `<<call name {json}>>` format.

use serde_json::Value;

/// Render a single tool call in compact format.
pub fn render_call(name: &str, args: &Value) -> String {
    let args_str = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
    format!("<<call {name} {args_str}>>")
}

/// Render multiple tool calls, space-separated.
pub fn render_calls(calls: &[(String, Value)]) -> String {
    calls
        .iter()
        .map(|(name, args)| render_call(name, args))
        .collect::<Vec<_>>()
        .join(" ")
}
