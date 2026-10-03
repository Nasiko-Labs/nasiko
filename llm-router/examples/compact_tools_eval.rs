use nasiko_tool_compact::{decode_calls, encode_tools, render_compact_tools, StreamDecoder, ToolDef};
use serde_json::{json, Value};
use std::io::Write;

fn selected_tools(all: &[Value], names: &[Value]) -> Vec<ToolDef> {
    names
        .iter()
        .map(|name| {
            let n = name.as_str().expect("tool name");

            let raw = all
                .iter()
                .find(|t| t["function"]["name"] == n)
                .expect("referenced tool exists");

            ToolDef {
                name: n.to_string(),
                description: raw["function"]["description"]
                    .as_str()
                    .map(str::to_string),
                parameters: Some(raw["function"]["parameters"].clone()),
            }
        })
        .collect()
}

fn render_call(name: &str, args: &Value) -> String {
    format!(
        "<<call {} {}>>",
        name,
        serde_json::to_string(args).expect("serialize arguments")
    )
}

fn json_calls(calls: Vec<nasiko_tool_compact::ToolCall>) -> Value {
    Value::Array(
        calls
            .into_iter()
            .map(|call| {
                json!({
                    "name": call.name,
                    "arguments": serde_json::from_str::<Value>(&call.arguments)
                        .unwrap_or(Value::Null)
                })
            })
            .collect(),
    )
}

fn error_value(error: impl std::fmt::Display) -> Value {
    let message = error.to_string();

    let category = if message.starts_with("unknown tool:") {
        "unknown_tool"
    } else if message.starts_with("missing required argument:")
        || message.contains("invalid JSON")
        || message.contains("must be")
    {
        "invalid_arguments"
    } else if message.contains("unterminated tool call")
        || message.contains("invalid compact tool format")
    {
        "invalid_format"
    } else {
        "invalid_arguments"
    };

    json!({
        "error": category
    })
}

fn compact_request(
    compact: &nasiko_tool_compact::CompactTools,
    messages: &Value,
) -> Value {
    json!({
        "messages": messages,
        "tools": [{
            "type": "compact",
            "definitions": render_compact_tools(compact)
        }],
        "tool_call_format": "<<call name {json args}>>"
    })
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET");
    let out_path =
        std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let raw = std::fs::read_to_string(path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid JSON");

    let all_tools = data["tools"].as_array().expect("tools array");
    let cases = data["cases"].as_array().expect("cases array");

    let mut out =
        std::io::BufWriter::new(std::fs::File::create(out_path).expect("create OUT"));

    // Normal evaluation cases.
    for case in cases {
        let names = case["tools"].as_array().expect("case tools");
        let tools = selected_tools(all_tools, names);

        let compact = encode_tools(&tools);
        let compacted = compact.is_ok();

        let expected = case["expected"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let rendered_calls = expected
            .iter()
            .filter_map(|call| {
                Some(render_call(
                    call["name"].as_str()?,
                    &call["arguments"],
                ))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let roundtrip_calls = match &compact {
            Ok(compact) => match decode_calls(&rendered_calls, compact) {
                Ok(calls) => json_calls(calls),
                Err(error) => json!([error_value(error)]),
            },
            Err(error) => json!([error_value(error)]),
        };

        let line = json!({
            "id": case["id"],
            "compact_request": compact
                .as_ref()
                .map(|c| compact_request(c, &case["messages"]))
                .unwrap_or_else(|_| json!({
                    "messages": case["messages"],
                    "tools": [],
                    "tool_call_format": "<<call name {json args}>>"
                })),
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls
        });

        writeln!(out, "{line}").expect("write OUT");
    }

    // Decoder evaluation cases.
    if let Some(decoder_cases) = data["decoder_cases"].as_array() {
        for decoder_case in decoder_cases {
            let names = decoder_case["tools"]
                .as_array()
                .expect("decoder tools");

            let tools = selected_tools(all_tools, names);

            let compact =
                encode_tools(&tools).expect("decoder tool schema");

            let chunks = decoder_case["chunks"]
                .as_array()
                .expect("decoder chunks");

            let mut decoder = StreamDecoder::new();
            let mut decoded = Vec::new();
            let mut error = None;

            for chunk in chunks {
                let chunk = chunk.as_str().expect("decoder chunk");

                match decoder.push(chunk, &compact) {
                    Ok(mut calls) => decoded.append(&mut calls),
                    Err(err) => {
                        error = Some(err.to_string());
                        break;
                    }
                }
            }

            if error.is_none() {
                match decoder.finish(&compact) {
                    Ok(mut calls) => decoded.append(&mut calls),
                    Err(err) => {
                        error = Some(err.to_string());
                    }
                }
            }

            let decoded_value = if let Some(error) = error {
                error_value(error)
            } else {
                json!({
                    "calls": json_calls(decoded)
                })
            };

            let line = json!({
                "id": decoder_case["id"],
                "decoded": decoded_value
            });

            writeln!(out, "{line}").expect("write OUT");
        }
    }

    out.flush().expect("flush OUT");
}
