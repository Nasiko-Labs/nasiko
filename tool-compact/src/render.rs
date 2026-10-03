use crate::ToolCall;
use serde_json::Value;

pub fn render_calls(calls: &[ToolCall]) -> String {
    let rendered = calls
        .iter()
        .map(|call| {
            format!(
                "<<call {} {}>>",
                call.name,
                compact_json_object(&call.arguments)
            )
        })
        .collect::<Vec<_>>();
    rendered.join("\n")
}

fn compact_json_object(map: &serde_json::Map<String, Value>) -> String {
    let value = Value::Object(map.clone());
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}
