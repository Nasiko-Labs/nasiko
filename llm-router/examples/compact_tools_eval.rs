//! Compact tool schemas evaluation runner (Track P1).
//!
//! Run:
//!   EVAL_SET=path/to/compact-tools-eval.json OUT=path/to/eval-output.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Reads evaluation cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. Does not compute scores; the official Nasiko harness computes all metrics.

use std::collections::HashMap;
use std::io::Write;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde_json::{Value, json};

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    // Index global tools defined at root if present
    let mut tool_catalog: HashMap<String, ToolDef> = HashMap::new();
    if let Some(tools_arr) = data.get("tools").and_then(Value::as_array) {
        for t in tools_arr {
            if let Ok(tool_def) = serde_json::from_value::<ToolDef>(t.clone()) {
                tool_catalog.insert(tool_def.function.name.clone(), tool_def);
            }
        }
    }

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    // 1. Process end-to-end / schema cases
    if let Some(cases) = data.get("cases").and_then(Value::as_array) {
        for case in cases {
            let id = case["id"].as_str().unwrap_or("unknown");
            let tool_defs = resolve_tools(&case["tools"], &tool_catalog);

            let (compacted, compact_request, rendered_calls, roundtrip_calls) =
                match encode_tools(&tool_defs) {
                    Ok(ct) => {
                        // Extract expected calls to test rendering and round-trip decoding
                        let expected = extract_expected_calls(case);
                        let mut rendered_marker_text = String::new();
                        for call in &expected {
                            let name = call["name"].as_str().unwrap_or_default();
                            let args = &call["arguments"];
                            rendered_marker_text.push_str(&format!("<<call {name} {args}>>\n"));
                        }

                        let roundtrip = match decode_calls(&rendered_marker_text, &tool_defs) {
                            Ok(calls) => calls_to_json(&calls),
                            Err(_) => json!([]),
                        };

                        // Build full OpenAI-shaped chat request body
                        let mut messages = vec![json!({
                            "role": "system",
                            "content": format!("[Tools]\n{}", ct.prompt)
                        })];
                        if let Some(msg_arr) = case.get("messages").and_then(Value::as_array) {
                            messages.extend(msg_arr.clone());
                        }

                        let req_body = json!({
                            "model": "gpt-4o-mini",
                            "messages": messages,
                        });

                        (
                            true,
                            req_body,
                            json!(rendered_marker_text.trim()),
                            roundtrip,
                        )
                    }
                    Err(_) => {
                        let mut req_body = json!({
                            "model": "gpt-4o-mini",
                            "messages": case.get("messages").cloned().unwrap_or(json!([])),
                            "tools": tool_defs,
                        });
                        (false, req_body, json!(""), json!([]))
                    }
                };

            let line = json!({
                "id": id,
                "compact_request": compact_request,
                "compacted": compacted,
                "rendered_calls": rendered_calls,
                "roundtrip_calls": roundtrip_calls,
            });
            writeln!(out, "{line}").expect("write OUT");
        }
    }

    // 2. Process decoder-only cases
    if let Some(decoder_cases) = data.get("decoder_cases").and_then(Value::as_array) {
        for case in decoder_cases {
            let id = case["id"].as_str().unwrap_or("unknown");
            let tool_defs = resolve_tools(&case["tools"], &tool_catalog);

            let decoded_result = if let Some(chunks) = case.get("chunks").and_then(Value::as_array)
            {
                let mut decoder = StreamDecoder::new(tool_defs);
                for chunk in chunks {
                    if let Some(chunk_str) = chunk.as_str() {
                        decoder.push_chunk(chunk_str);
                    }
                }
                decoder.finish()
            } else if let Some(full_text) = case.get("full_text").and_then(Value::as_str) {
                decode_calls(full_text, &tool_defs)
            } else {
                decode_calls("", &tool_defs)
            };

            let decoded_obj = match decoded_result {
                Ok(calls) => {
                    json!({
                        "calls": calls_to_json(&calls)
                    })
                }
                Err(err) => {
                    json!({
                        "error": err.category()
                    })
                }
            };

            let line = json!({
                "id": id,
                "decoded": decoded_obj
            });
            writeln!(out, "{line}").expect("write OUT");
        }
    }

    out.flush().expect("flush OUT");
}

fn resolve_tools(tools_val: &Value, catalog: &HashMap<String, ToolDef>) -> Vec<ToolDef> {
    let mut resolved = Vec::new();
    if let Some(arr) = tools_val.as_array() {
        for item in arr {
            if let Some(name) = item.as_str() {
                if let Some(def) = catalog.get(name) {
                    resolved.push(def.clone());
                }
            } else if let Ok(def) = serde_json::from_value::<ToolDef>(item.clone()) {
                resolved.push(def);
            }
        }
    }
    resolved
}

fn extract_expected_calls(case: &Value) -> Vec<Value> {
    if let Some(arr) = case.get("expected").and_then(Value::as_array) {
        return arr.clone();
    }
    if let Some(single) = case.get("expected_call") {
        if single.is_object() {
            return vec![single.clone()];
        }
    }
    Vec::new()
}

fn calls_to_json(calls: &[nasiko_tool_compact::ToolCall]) -> Value {
    let list: Vec<Value> = calls
        .iter()
        .map(|c| {
            let parsed_args = serde_json::from_str::<Value>(&c.function.arguments)
                .unwrap_or(Value::String(c.function.arguments.clone()));
            json!({
                "name": c.function.name,
                "arguments": parsed_args
            })
        })
        .collect();
    json!(list)
}
