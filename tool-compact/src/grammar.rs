use crate::types::ToolCall;

pub const OPEN: &str = "<<call ";
pub const CLOSE: &str = ">>";

pub fn render_call(call: &ToolCall) -> String {
    let args_str = if call.arguments.is_object() {
        serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_string())
    } else {
        "{}".to_string()
    };
    format!("{OPEN}{} {args_str}{CLOSE}", call.name)
}
