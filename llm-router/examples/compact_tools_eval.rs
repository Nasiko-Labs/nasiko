//! Compact tool schemas evaluation harness.
//!
//! Run offline:
//!   EVAL_SET=/path/to/compact-tools-eval.json OUT=/path/to/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Run live against OpenAI-compatible endpoint:
//!   PROVIDER_BASE_URL="https://api.openai.com/v1" MODEL="gpt-4o-mini" OPENAI_API_KEY="..." \
//!   EVAL_SET=/path/to/compact-tools-eval.json OUT=/path/to/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolCall, ToolDef};
use serde_json::Value;

const REFERENCE_TIME_MSG: &str = "Today is 2026-10-02, timezone Asia/Kolkata.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = std::env::var("EVAL_SET").unwrap_or_else(|_| "compact-tools-eval.json".into());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "out.jsonl".into());

    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    let api_key = std::env::var("OPENAI_API_KEY").or_else(|_| std::env::var("AUTH_TOKEN")).ok();
    let is_live = provider_base_url.is_some() && model.is_some();

    let raw_eval = std::fs::read_to_string(&eval_set_path)
        .unwrap_or_else(|e| panic!("Failed to read EVAL_SET from '{eval_set_path}': {e}"));
    let eval_data: Value = serde_json::from_str(&raw_eval)
        .unwrap_or_else(|e| panic!("Failed to parse JSON in '{eval_set_path}': {e}"));

    // Ensure output parent directory exists if specified
    if let Some(parent) = Path::new(&out_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }
    let out_file = File::create(&out_path)
        .unwrap_or_else(|e| panic!("Failed to create output file '{out_path}': {e}"));
    let mut out_writer = BufWriter::new(out_file);

    // Initialize tiktoken for local token reduction measurement
    let bpe = tiktoken_rs::o200k_base().expect("Failed to initialize o200k_base tokenizer");

    // Index all available tools from dataset
    let mut tools_map: HashMap<String, ToolDef> = HashMap::new();
    if let Some(tools_arr) = eval_data.get("tools").and_then(Value::as_array) {
        for t in tools_arr {
            if let Ok(td) = serde_json::from_value::<ToolDef>(t.clone()) {
                tools_map.insert(td.function.name.clone(), td);
            }
        }
    } else if let Some(tools_obj) = eval_data.get("tools").and_then(Value::as_object) {
        for (name, val) in tools_obj {
            if let Ok(mut td) = serde_json::from_value::<ToolDef>(val.clone()) {
                if td.function.name.is_empty() {
                    td.function.name = name.clone();
                }
                tools_map.insert(name.clone(), td);
            } else if let Ok(fd) = serde_json::from_value::<nasiko_tool_compact::FunctionDef>(val.clone()) {
                tools_map.insert(
                    name.clone(),
                    ToolDef {
                        r#type: "function".to_string(),
                        function: fd,
                    },
                );
            }
        }
    }

    let http_client = if is_live {
        Some(reqwest::Client::new())
    } else {
        None
    };

    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // 1. Process standard evaluation cases
    if let Some(cases) = eval_data.get("cases").and_then(Value::as_array) {
        for case in cases {
            let id = case["id"].as_str().unwrap_or("unknown");

            // Resolve active tools for this case
            let mut active_tools: Vec<ToolDef> = Vec::new();
            if let Some(tool_items) = case.get("tools").and_then(Value::as_array) {
                for item in tool_items {
                    if let Some(tool_name) = item.as_str() {
                        if let Some(td) = tools_map.get(tool_name) {
                            active_tools.push(td.clone());
                        }
                    } else if let Ok(td) = serde_json::from_value::<ToolDef>(item.clone()) {
                        active_tools.push(td);
                    }
                }
            }

            let messages = case.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
            let compacted = !active_tools.is_empty();

            // Construct compact request
            let compact_request = if compacted {
                let compact_tools = encode_tools(&active_tools).expect("encode_tools should succeed");
                let system_prompt = format!("{}\n\n{}", REFERENCE_TIME_MSG, compact_tools.combined_prompt);

                let mut compact_messages = Vec::new();
                compact_messages.push(serde_json::json!({
                    "role": "system",
                    "content": system_prompt
                }));

                for msg in &messages {
                    if msg.get("role").and_then(Value::as_str) == Some("system") {
                        // Append original system message
                        if let Some(existing_content) = msg.get("content").and_then(Value::as_str) {
                            compact_messages[0]["content"] = Value::String(format!(
                                "{system_prompt}\n\n{existing_content}"
                            ));
                        }
                    } else {
                        compact_messages.push(msg.clone());
                    }
                }

                let mut req_map = serde_json::Map::new();
                req_map.insert("messages".into(), Value::Array(compact_messages));
                req_map.insert("temperature".into(), serde_json::json!(0.0));
                if let Some(m) = &model {
                    req_map.insert("model".into(), Value::String(m.clone()));
                }
                Value::Object(req_map)
            } else {
                let mut req_map = serde_json::Map::new();
                req_map.insert("messages".into(), Value::Array(messages.clone()));
                req_map.insert("temperature".into(), serde_json::json!(0.0));
                if let Some(m) = &model {
                    req_map.insert("model".into(), Value::String(m.clone()));
                }
                Value::Object(req_map)
            };

            // Build native baseline request for token comparison
            let mut baseline_messages = Vec::new();
            baseline_messages.push(serde_json::json!({
                "role": "system",
                "content": REFERENCE_TIME_MSG
            }));
            for msg in &messages {
                baseline_messages.push(msg.clone());
            }
            let mut baseline_map = serde_json::Map::new();
            baseline_map.insert("messages".into(), Value::Array(baseline_messages));
            if !active_tools.is_empty() {
                baseline_map.insert("tools".into(), serde_json::to_value(&active_tools).unwrap());
            }
            let baseline_request = Value::Object(baseline_map);

            // Token count comparison
            let baseline_str = serde_json::to_string(&baseline_request).unwrap();
            let compact_str = serde_json::to_string(&compact_request).unwrap();
            let baseline_tokens = bpe.encode_with_special_tokens(&baseline_str).len();
            let compact_tokens = bpe.encode_with_special_tokens(&compact_str).len();
            total_baseline_tokens += baseline_tokens;
            total_compact_tokens += compact_tokens;

            // Render expected calls into custom compact syntax: <<call name {json}>>
            let mut rendered_calls_vec = Vec::new();
            if let Some(expected_arr) = case.get("expected").and_then(Value::as_array) {
                for exp in expected_arr {
                    if let Some(name) = exp.get("name").and_then(Value::as_str) {
                        let args = exp.get("arguments").cloned().unwrap_or(serde_json::json!({}));
                        let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
                        rendered_calls_vec.push(format!("<<call {name} {args_str}>>"));
                    }
                }
            }
            let rendered_calls = rendered_calls_vec.join("\n");

            // Decode rendered calls back to standard tool calls (round-trip verification)
            let roundtrip_calls: Vec<ToolCall> = if rendered_calls.is_empty() {
                Vec::new()
            } else {
                decode_calls(&rendered_calls, &active_tools).unwrap_or_default()
            };

            let mut line_obj = serde_json::json!({
                "id": id,
                "compact_request": compact_request,
                "compacted": compacted,
                "rendered_calls": rendered_calls,
                "roundtrip_calls": roundtrip_calls,
            });

            // Live Mode execution if endpoint and model are configured
            if is_live {
                if let (Some(client), Some(base_url), Some(m)) = (&http_client, &provider_base_url, &model) {
                    let mut req_body = compact_request.clone();
                    req_body["model"] = Value::String(m.clone());

                    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                    let mut req_builder = client.post(&url).json(&req_body);
                    if let Some(k) = &api_key {
                        req_builder = req_builder.header("Authorization", format!("Bearer {k}"));
                    }

                    match req_builder.send().await {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                let body: Value = resp.json().await.unwrap_or_default();
                                let raw_output = body["choices"][0]["message"]["content"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();

                                let live_decoded = match decode_calls(&raw_output, &active_tools) {
                                    Ok(calls) => serde_json::to_value(calls).unwrap(),
                                    Err(e) => serde_json::json!({ "error": e.error_code() }),
                                };

                                line_obj["raw_output"] = Value::String(raw_output);
                                line_obj["live_calls"] = live_decoded;
                            } else {
                                let err_status = resp.status();
                                let err_text = resp.text().await.unwrap_or_default();
                                eprintln!("Live model call failed for case {id} ({err_status}): {err_text}");
                                line_obj["raw_output"] = Value::String(String::new());
                                line_obj["live_calls"] = serde_json::json!({ "error": "live_request_failed" });
                            }
                        }
                        Err(e) => {
                            eprintln!("Live model network error for case {id}: {e}");
                            line_obj["raw_output"] = Value::String(String::new());
                            line_obj["live_calls"] = serde_json::json!({ "error": "network_error" });
                        }
                    }
                }
            }

            writeln!(out_writer, "{}", serde_json::to_string(&line_obj).unwrap())?;
        }
    }

    // 2. Process streaming decoder test cases
    if let Some(decoder_cases) = eval_data.get("decoder_cases").and_then(Value::as_array) {
        for dc in decoder_cases {
            let id = dc["id"].as_str().unwrap_or("unknown");

            let mut active_tools: Vec<ToolDef> = Vec::new();
            if let Some(tool_items) = dc.get("tools").and_then(Value::as_array) {
                for item in tool_items {
                    if let Some(tool_name) = item.as_str() {
                        if let Some(td) = tools_map.get(tool_name) {
                            active_tools.push(td.clone());
                        }
                    } else if let Ok(td) = serde_json::from_value::<ToolDef>(item.clone()) {
                        active_tools.push(td);
                    }
                }
            }

            let mut stream_decoder = StreamDecoder::new();
            if let Some(chunks) = dc.get("chunks").and_then(Value::as_array) {
                for chunk in chunks {
                    if let Some(chunk_str) = chunk.as_str() {
                        stream_decoder.push_chunk(chunk_str);
                    }
                }
            }

            let decoded_payload = match stream_decoder.finish(&active_tools) {
                Ok(calls) => serde_json::json!({ "calls": calls }),
                Err(err) => serde_json::json!({ "error": err.error_code() }),
            };

            let line_obj = serde_json::json!({
                "id": id,
                "decoded": decoded_payload,
            });

            writeln!(out_writer, "{}", serde_json::to_string(&line_obj).unwrap())?;
        }
    }

    out_writer.flush()?;

    // Print evaluation statistics
    println!("=== Compact Tools Eval Summary ===");
    if total_baseline_tokens > 0 {
        let reduction = 1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64);
        let reduction_pct = reduction * 100.0;
        println!("Baseline Request Tokens:  {total_baseline_tokens}");
        println!("Compacted Request Tokens: {total_compact_tokens}");
        println!("Token Reduction Savings:  {reduction_pct:.2}% (Target: >=30%)");
    } else {
        println!("No tool cases evaluated for token comparison.");
    }
    println!("Evaluation output successfully written to: {out_path}");

    Ok(())
}
