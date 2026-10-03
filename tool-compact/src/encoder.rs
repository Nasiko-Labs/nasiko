use serde_json::Value;
use crate::error::{Result, ToolCompactError};
use crate::schema::ToolRegistry;

/// Encodes a tool name and JSON arguments into the compact tool call representation.
///
/// Example output:
/// `weather_get(location="New York", units="celsius")`
pub fn encode_call(tool_name: &str, args: &Value) -> Result<String> {
    let mut out = String::new();
    out.push_str(tool_name);
    out.push('(');

    if let Value::Object(map) = args {
        let mut first = true;
        for (k, v) in map {
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(k);
            out.push('=');
            out.push_str(&encode_value(v));
        }
    } else if !args.is_null() {
        return Err(ToolCompactError::SerializationError(
            "Tool call arguments must be a JSON Object".into(),
        ));
    }

    out.push(')');
    Ok(out)
}

/// Formats a serde_json::Value into compact representation.
pub fn encode_value(val: &Value) -> String {
    match val {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => escape_string(s),
        Value::Array(arr) => {
            let mut s = String::new();
            s.push('[');
            for (i, elem) in arr.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&encode_value(elem));
            }
            s.push(']');
            s
        }
        Value::Object(map) => {
            let mut s = String::new();
            s.push('{');
            let mut first = true;
            for (k, v) in map {
                if !first {
                    s.push_str(", ");
                }
                first = false;
                s.push_str(k);
                s.push(':');
                s.push_str(&encode_value(v));
            }
            s.push('}');
            s
        }
    }
}

/// Escapes a string for safe output inside quotes.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Encodes all tool definitions registered in a `ToolRegistry` into a token-optimized system prompt block.
///
/// Format per tool:
/// `@tool name(param:type[!=default]) "description"`
pub fn encode_schemas(registry: &ToolRegistry) -> String {
    let mut out = String::new();
    out.push_str("COMPACT TOOL DEFINITIONS:\n");
    for tool in registry.iter() {
        out.push_str("@tool ");
        out.push_str(&tool.name);
        out.push('(');
        for (i, param) in tool.parameters.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&param.name);
            out.push(':');
            out.push_str(&param.val_type.to_string());
            if param.required {
                out.push('!');
            }
            if let Some(ref def) = param.default {
                out.push('=');
                out.push_str(&encode_value(def));
            }
            if let Some(ref enum_vals) = param.enum_values {
                out.push('[');
                out.push_str(&enum_vals.join("|"));
                out.push(']');
            }
        }
        out.push(')');
        if !tool.description.is_empty() {
            out.push_str(" \"");
            out.push_str(&tool.description.replace('"', "\\\""));
            out.push('"');
        }
        out.push('\n');
    }
    out
}
