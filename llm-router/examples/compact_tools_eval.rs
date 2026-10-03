//! Compact tool schemas evaluation harness.
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Reads evaluation cases from `EVAL_SET` and writes one JSONL line per case to `OUT`.

use std::collections::HashMap;
use std::io::Write;
use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolDef};
use reqwest::blocking::Client;
use serde_json::{json, Value};

fn main() {
    let path = std::env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".into());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".into());

    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read EVAL_SET from '{}': {}", path, e));
    let data: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse JSON in EVAL_SET: {}", e));

    let tools_catalog = parse_tools_catalog(&data["tools"]);
    let live_config = LiveConfig::from_env();
    let live_client = live_config.as_ref().map(|_| Client::new());

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path)
            .unwrap_or_else(|e| panic!("Failed to create OUT at '{}': {}", out_path, e)),
    );

    // 1. Process standard compaction / roundtrip cases
    if let Some(cases) = data["cases"].as_array() {
        for case in cases {
            let id = case["id"].as_str().unwrap_or("");
            let tool_names: Vec<&str> = case["tools"]
                .as_array()
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let tool_defs: Vec<ToolDef> = tool_names
                .iter()
                .filter_map(|name| tools_catalog.get(*name).cloned())
                .collect();

            let compact_res = encode_tools(&tool_defs);

            let (compact_request, rendered_calls, roundtrip_calls, compacted) = match compact_res {
                Ok(compact) => {
                    let mut req_messages = vec![json!({
                        "role": "system",
                        "content": compact.prompt_injection
                    })];
                    if let Some(msgs) = case["messages"].as_array() {
                        req_messages.extend(msgs.clone());
                    }

                    let compact_request = json!({
                        "messages": req_messages
                    });

                    // Render expected calls into compact format
                    let mut rendered_parts = Vec::new();
                    if let Some(expected_arr) = case["expected"].as_array() {
                        for exp in expected_arr {
                            let name = exp["name"].as_str().unwrap_or("");
                            let args = exp.get("arguments").cloned().unwrap_or(json!({}));
                            let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
                            rendered_parts.push(format!("<<call {} {}>>", name, args_str));
                        }
                    }
                    let rendered_calls = rendered_parts.join(" ");

                    let roundtrip_calls = decode_calls(&rendered_calls, &tool_defs).unwrap_or_default();

                    (compact_request, rendered_calls, roundtrip_calls, true)
                }
                Err(_) => {
                    // Compaction bypass fallback
                    (
                        json!({
                            "messages": case.get("messages").cloned().unwrap_or(json!([])),
                            "tools": case.get("tools").cloned().unwrap_or(json!([]))
                        }),
                        String::new(),
                        vec![],
                        false,
                    )
                }
            };

            let mut line = json!({
                "id": id,
                "compact_request": compact_request,
                "compacted": compacted,
                "rendered_calls": rendered_calls,
                "roundtrip_calls": roundtrip_calls
            });

            if let (Some(config), Some(client)) = (&live_config, &live_client) {
                if let Some((raw_output, live_calls)) =
                    run_live_request(client, config, &line["compact_request"], &tool_defs, id)
                {
                    let line_obj = line.as_object_mut().expect("JSON line must be an object");
                    line_obj.insert("raw_output".to_string(), json!(raw_output));
                    line_obj.insert("live_calls".to_string(), live_calls);
                }
            }

            writeln!(out, "{line}").expect("Failed to write to OUT");
        }
    }

    // 2. Process streaming decoder cases
    if let Some(decoder_cases) = data["decoder_cases"].as_array() {
        for dc in decoder_cases {
            let id = dc["id"].as_str().unwrap_or("");
            let tool_names: Vec<&str> = dc["tools"]
                .as_array()
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let tool_defs: Vec<ToolDef> = tool_names
                .iter()
                .filter_map(|name| tools_catalog.get(*name).cloned())
                .collect();

            let mut decoder = StreamDecoder::new();
            if let Some(chunks) = dc["chunks"].as_array() {
                for chunk in chunks {
                    if let Some(c_str) = chunk.as_str() {
                        decoder.feed(c_str);
                    }
                }
            }

            let decoded_val = match decoder.finish(&tool_defs) {
                Ok(calls) => json!({ "calls": calls }),
                Err(err) => json!({ "error": err.error_code() }),
            };

            let line = json!({
                "id": id,
                "decoded": decoded_val
            });

            writeln!(out, "{line}").expect("Failed to write to OUT");
        }
    }

    out.flush().expect("Failed to flush OUT");
}

struct LiveConfig {
    base_url: String,
    model: String,
    api_key: Option<String>,
    api_type: String,
}

impl LiveConfig {
    fn from_env() -> Option<Self> {
        let base_url = std::env::var("PROVIDER_BASE_URL").ok()?;
        let model = std::env::var("MODEL").ok()?;
        Some(Self {
            base_url,
            model,
            api_key: std::env::var("PROVIDER_API_KEY").ok(),
            api_type: std::env::var("PROVIDER_API_TYPE").unwrap_or_else(|_| "chat".to_string()),
        })
    }
}

fn run_live_request(
    client: &Client,
    config: &LiveConfig,
    compact_request: &Value,
    tools: &[ToolDef],
    case_id: &str,
) -> Option<(String, Value)> {
    let (url, request_body) = if config.api_type == "responses" {
        let messages = match compact_request["messages"].as_array() {
            Some(messages) => messages
                .iter()
                .filter(|message| {
                    matches!(
                        message["role"].as_str(),
                        Some("system") | Some("user")
                    )
                })
                .cloned()
                .collect::<Vec<_>>(),
            None => {
                eprintln!("warning: compact request for case {case_id} had no messages array");
                return None;
            }
        };

        (
            format!("{}/responses", config.base_url.trim_end_matches('/')),
            json!({
                "model": config.model,
                "input": messages,
                "temperature": 0
            }),
        )
    } else {
        let mut request_body = compact_request.clone();
        if let Some(request_obj) = request_body.as_object_mut() {
            request_obj.insert("model".to_string(), json!(config.model));
            request_obj.insert("temperature".to_string(), json!(0));
        } else {
            eprintln!("warning: compact request for case {case_id} was not a JSON object");
            return None;
        }

        (
            format!("{}/chat/completions", config.base_url.trim_end_matches('/')),
            request_body,
        )
    };
    let mut request = client.post(url).json(&request_body);
    if let Some(api_key) = &config.api_key {
        request = request.bearer_auth(api_key);
    }

    let response = match request.send().and_then(reqwest::blocking::Response::error_for_status) {
        Ok(response) => response,
        Err(err) => {
            eprintln!("warning: live request for case {case_id} failed: {err}");
            return None;
        }
    };

    let response_json: Value = match response.json() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("warning: live response for case {case_id} was not valid JSON: {err}");
            return None;
        }
    };
    let raw_output = match if config.api_type == "responses" {
        response_json["output"][0]["content"][0]["text"].as_str()
    } else {
        response_json["choices"][0]["message"]["content"].as_str()
    } {
        Some(content) => content.to_string(),
        None => {
            eprintln!("warning: live response for case {case_id} had no assistant content");
            return None;
        }
    };

    let live_calls = match decode_calls(&raw_output, tools) {
        Ok(calls) => json!({ "calls": calls }),
        Err(err) => json!({ "error": err.error_code() }),
    };
    Some((raw_output, live_calls))
}

fn parse_tools_catalog(tools_val: &Value) -> HashMap<String, ToolDef> {
    let mut catalog = HashMap::new();
    if let Some(arr) = tools_val.as_array() {
        for item in arr {
            if let Some(fn_obj) = item.get("function") {
                let name = fn_obj.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let description = fn_obj.get("description").and_then(Value::as_str).map(|s| s.to_string());
                let parameters = fn_obj.get("parameters").cloned();
                if !name.is_empty() {
                    catalog.insert(name.clone(), ToolDef { name, description, parameters });
                }
            } else if let Some(name) = item.get("name").and_then(Value::as_str) {
                let description = item.get("description").and_then(Value::as_str).map(|s| s.to_string());
                let parameters = item.get("parameters").cloned();
                catalog.insert(name.to_string(), ToolDef { name: name.to_string(), description, parameters });
            }
        }
    }
    catalog
}
