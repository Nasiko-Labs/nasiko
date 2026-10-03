//! Calls → canonical `<<call …>>` text.

use crate::types::ToolCall;

/// Render calls in the canonical call language: `<<call NAME {compact JSON}>>`, one per line.
///
/// Arguments that parse as JSON are re-emitted compactly (no extra whitespace); anything else is
/// written as given, so a bad call renders visibly bad rather than being "fixed" here.
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| {
            let args = serde_json::from_str::<serde_json::Value>(&c.arguments)
                .ok()
                .and_then(|v| serde_json::to_string(&v).ok())
                .unwrap_or_else(|| c.arguments.clone());
            format!("<<call {} {args}>>", c.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
