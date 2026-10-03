//! Renders tool calls in the compact call format.

use serde_json::Value;

use crate::types::ToolCall;

/// Write each call as `<<call name {json}>>`, one per line. Arguments are canonical JSON
/// (sorted keys), so the output is deterministic.
pub fn render_calls(calls: &[ToolCall]) -> String {
    let mut lines = Vec::with_capacity(calls.len());
    for call in calls {
        let args = match serde_json::from_str::<Value>(&call.arguments) {
            Ok(value) => canonical_json(&value),
            // Not valid JSON: keep the text so the problem stays visible to the decoder.
            Err(_) => call.arguments.clone(),
        };
        lines.push(format!("<<call {} {}>>", call.name, args));
    }
    lines.join("\n")
}

/// Compact JSON with object keys sorted at every depth.
pub(crate) fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                if let Some(child) = map.get(key) {
                    write_canonical(child, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            name: name.into(),
            arguments: args.into(),
        }
    }

    #[test]
    fn sorts_keys_at_every_depth() {
        let out = render_calls(&[call("t", r#"{"b":1,"a":{"z":true,"y":[{"q":1,"p":2}]}}"#)]);
        assert_eq!(
            out,
            r#"<<call t {"a":{"y":[{"p":2,"q":1}],"z":true},"b":1}>>"#
        );
    }

    #[test]
    fn joins_calls_with_newlines_and_keeps_gt_gt_in_strings() {
        let out = render_calls(&[call("a", r#"{"x":"1>>2"}"#), call("b", "{}")]);
        assert_eq!(out, "<<call a {\"x\":\"1>>2\"}>>\n<<call b {}>>");
    }

    #[test]
    fn no_calls_renders_empty() {
        assert_eq!(render_calls(&[]), "");
    }

    #[test]
    fn keeps_invalid_argument_text_visible() {
        assert_eq!(render_calls(&[call("t", "{oops")]), "<<call t {oops>>");
    }
}
