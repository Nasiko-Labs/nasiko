//! Evaluation harness for P1 — Compact tool schemas.
//!
//! Default (offline) mode: encode/decode round-trip, no network.
//! Live mode: set `PROVIDER_BASE_URL` and `MODEL` to send compact requests to an LLM.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```

use nasiko_tool_compact::{
    CompactTools, StreamDecoder, StreamEvent, ToolDef, decode_calls, encode_tools,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::Write;
use tiktoken_rs::o200k_base;

fn main() {
    let eval_path = std::env::var("EVAL_SET").unwrap_or_else(|_| {
        eprintln!("EVAL_SET not set, using default path /tmp/compact-tools-eval.json");
        "/tmp/compact-tools-eval.json".to_string()
    });
    let out_path = std::env::var("OUT").unwrap_or_else(|_| {
        eprintln!("OUT not set, using default path /tmp/out.jsonl");
        "/tmp/out.jsonl".to_string()
    });

    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    let live_mode = provider_base_url.is_some() && model.is_some();

    let data = std::fs::read_to_string(&eval_path)
        .unwrap_or_else(|e| panic!("cannot read EVAL_SET at {eval_path}: {e}"));
    let eval_set: Value =
        serde_json::from_str(&data).unwrap_or_else(|e| panic!("invalid JSON in EVAL_SET: {e}"));

    let all_tools = parse_tools(&eval_set["tools"]);
    let tool_map: HashMap<String, ToolDef> = all_tools
        .iter()
        .map(|t| (t.name.clone(), t.clone()))
        .collect();

    let bpe = o200k_base().expect("failed to load o200k_base tokenizer");

    let mut out_file = std::fs::File::create(&out_path)
        .unwrap_or_else(|e| panic!("cannot create OUT at {out_path}: {e}"));

    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // Process cases (encode/decode round-trip)
    if let Some(cases) = eval_set["cases"].as_array() {
        for case in cases {
            let id = case["id"].as_str().unwrap_or("unknown");
            let case_tool_names: Vec<&str> = case["tools"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let case_tools: Vec<ToolDef> = case_tool_names
                .iter()
                .filter_map(|n| tool_map.get(*n).cloned())
                .collect();

            let messages: Vec<Value> = case["messages"].as_array().cloned().unwrap_or_default();

            let expected: Vec<Value> = case["expected"].as_array().cloned().unwrap_or_default();

            // Build baseline request (native OpenAI format)
            let baseline_tools_json: Vec<Value> =
                case_tools.iter().map(tool_def_to_openai_json).collect();

            let baseline_request = json!({
                "model": "gpt-4o",
                "messages": messages,
                "tools": baseline_tools_json,
            });
            let baseline_tokens = bpe.encode_ordinary(&baseline_request.to_string()).len();

            // Encode tools compactly
            let compact_result = match encode_tools(&case_tools) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("  [{id}] encode error: {e}");
                    write_jsonl(
                        &mut out_file,
                        &json!({"id": id, "compacted": false, "error": e.to_string()}),
                    );
                    total_baseline_tokens += baseline_tokens;
                    total_compact_tokens += baseline_tokens;
                    continue;
                }
            };

            // Build compact request
            let compact_request = build_compact_request(&messages, &compact_result);
            let compact_tokens = bpe.encode_ordinary(&compact_request.to_string()).len();

            total_baseline_tokens += baseline_tokens;
            total_compact_tokens += compact_tokens;

            // Render expected calls in compact format for round-trip
            let rendered_calls = render_expected_calls(&expected);

            // Round-trip: decode the rendered calls back
            let roundtrip_calls = match decode_calls(&rendered_calls, &case_tools) {
                Ok(calls) => calls
                    .iter()
                    .map(|c| {
                        json!({
                            "name": c.name,
                            "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null)
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    eprintln!("  [{id}] decode error: {e}");
                    vec![json!({"error": e.to_string()})]
                }
            };

            let mut line = json!({
                "id": id,
                "compact_request": compact_request,
                "compacted": true,
                "rendered_calls": rendered_calls,
                "roundtrip_calls": roundtrip_calls,
            });

            // Live mode: send to LLM
            if live_mode {
                let live_result = send_live_request(
                    provider_base_url.as_deref().unwrap(),
                    model.as_deref().unwrap(),
                    &compact_request,
                    &case_tools,
                );
                if let Some(obj) = line.as_object_mut() {
                    for (k, v) in live_result {
                        obj.insert(k, v);
                    }
                }
            }

            write_jsonl(&mut out_file, &line);
        }
    }

    // Process decoder_cases (stream decoder)
    if let Some(decoder_cases) = eval_set["decoder_cases"].as_array() {
        for dc in decoder_cases {
            let id = dc["id"].as_str().unwrap_or("unknown");
            let case_tool_names: Vec<&str> = dc["tools"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let case_tools: Vec<ToolDef> = case_tool_names
                .iter()
                .filter_map(|n| tool_map.get(*n).cloned())
                .collect();

            let chunks: Vec<&str> = dc["chunks"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let decoded = run_stream_decoder(&case_tools, &chunks);
            let line = json!({"id": id, "decoded": decoded});
            write_jsonl(&mut out_file, &line);
        }
    }

    // Print summary
    let reduction = if total_baseline_tokens > 0 {
        1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)
    } else {
        0.0
    };
    eprintln!("--- Summary ---");
    eprintln!("Baseline tokens: {total_baseline_tokens}");
    eprintln!("Compact tokens:  {total_compact_tokens}");
    eprintln!("Token reduction: {:.1}%", reduction * 100.0);
    eprintln!("Output written to: {out_path}");
}

fn parse_tools(tools_json: &Value) -> Vec<ToolDef> {
    tools_json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let func = t.get("function")?;
                    Some(ToolDef {
                        name: func["name"].as_str()?.to_string(),
                        description: func["description"].as_str().map(String::from),
                        parameters: func.get("parameters").cloned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tool_def_to_openai_json(tool: &ToolDef) -> Value {
    let mut func = json!({ "name": tool.name });
    if let Some(desc) = &tool.description {
        func["description"] = Value::String(desc.clone());
    }
    if let Some(params) = &tool.parameters {
        func["parameters"] = params.clone();
    }
    json!({ "type": "function", "function": func })
}

fn build_compact_request(messages: &[Value], compact: &CompactTools) -> Value {
    let mut compact_messages = Vec::new();

    // System message with compact tool definitions and reference time
    compact_messages.push(json!({
        "role": "system",
        "content": format!(
            "Ref: 2026-10-02 Asia/Kolkata\n{}",
            compact.text
        )
    }));

    // Original messages
    for msg in messages {
        compact_messages.push(msg.clone());
    }

    json!({
        "model": "gpt-4o",
        "messages": compact_messages,
    })
}

fn render_expected_calls(expected: &[Value]) -> String {
    if expected.is_empty() {
        return String::new();
    }

    expected
        .iter()
        .filter_map(|e| {
            let name = e["name"].as_str()?;
            let args = &e["arguments"];
            Some(format!("<<call {name} {}>>", args))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_stream_decoder(tools: &[ToolDef], chunks: &[&str]) -> Value {
    let mut decoder = StreamDecoder::new(tools);
    let mut all_events = Vec::new();

    for chunk in chunks {
        match decoder.push(chunk) {
            Ok(events) => all_events.extend(events),
            Err(e) => return error_to_json(&e),
        }
    }

    match decoder.finish() {
        Ok(events) => all_events.extend(events),
        Err(e) => return error_to_json(&e),
    }

    let calls: Vec<Value> = all_events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Call(c) => Some(json!({
                "name": c.name,
                "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null)
            })),
            _ => None,
        })
        .collect();

    json!({ "calls": calls })
}

fn error_to_json(e: &nasiko_tool_compact::CompactError) -> Value {
    let error_type = match e {
        nasiko_tool_compact::CompactError::UnknownTool(_) => "unknown_tool",
        nasiko_tool_compact::CompactError::MissingRequired { .. } => "invalid_arguments",
        nasiko_tool_compact::CompactError::InvalidArgument { .. } => "invalid_arguments",
        nasiko_tool_compact::CompactError::MalformedCall(_) => "malformed_call",
        nasiko_tool_compact::CompactError::UnsupportedSchema(_, _) => "unsupported_schema",
        nasiko_tool_compact::CompactError::Json(_) => "invalid_arguments",
    };
    json!({ "error": error_type })
}

fn send_live_request(
    base_url: &str,
    model: &str,
    compact_request: &Value,
    tools: &[ToolDef],
) -> Map<String, Value> {
    let mut result = Map::new();

    let mut req_body = compact_request.clone();
    if let Some(obj) = req_body.as_object_mut() {
        obj.insert("model".to_string(), Value::String(model.to_string()));
    }

    let client = reqwest::blocking::Client::new();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let api_key = std::env::var("API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .unwrap_or_default();

    let mut request = client.post(&url).json(&req_body);
    if !api_key.is_empty() {
        request = request.bearer_auth(&api_key);
    }

    match request.send() {
        Ok(resp) => match resp.json::<Value>() {
            Ok(body) => {
                let raw_output = body["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                result.insert("raw_output".to_string(), Value::String(raw_output.clone()));

                match decode_calls(&raw_output, tools) {
                    Ok(calls) => {
                        let live_calls: Vec<Value> = calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "name": c.name,
                                    "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null)
                                })
                            })
                            .collect();
                        result.insert("live_calls".to_string(), json!(live_calls));
                    }
                    Err(e) => {
                        result.insert("live_calls".to_string(), json!({"error": e.to_string()}));
                    }
                }
            }
            Err(e) => {
                result.insert("live_error".to_string(), json!(e.to_string()));
            }
        },
        Err(e) => {
            result.insert("live_error".to_string(), json!(e.to_string()));
        }
    }

    result
}

fn write_jsonl(file: &mut std::fs::File, value: &Value) {
    let line = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    writeln!(file, "{line}").unwrap_or_else(|e| eprintln!("write error: {e}"));
}
