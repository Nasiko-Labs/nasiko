//! Compact tool schemas evaluation harness.
//!
//! Usage:
//!
//! ```sh
//! # Offline mode (default — deterministic, no network):
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! # Live mode (format adherence — sends requests to an LLM):
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//! PROVIDER_BASE_URL=https://api.openai.com/v1 MODEL=gpt-4o \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Required | Description |
//! |----------|----------|-------------|
//! | `EVAL_SET` | yes | Path to the evaluation JSON file |
//! | `OUT` | yes | Path for JSONL output |
//! | `PROVIDER_BASE_URL` | no | OpenAI-compatible API base URL (enables live mode) |
//! | `MODEL` | no | Model ID for live mode |

use std::fs;
use std::io::{BufWriter, Write};

use nasiko_tool_compact::{CompactTools, StreamDecoder, ToolDef, encode_tools, decode_calls};
use serde_json::{Value, json};

/// Fixed reference time and timezone for deterministic date resolution.
const SYSTEM_TIME_CONTEXT: &str =
    "Today is 2026-10-02. The current timezone is Asia/Kolkata (IST, UTC+05:30).";

fn main() {
    let eval_path = std::env::var("EVAL_SET").expect("EVAL_SET env var must be set");
    let out_path = std::env::var("OUT").expect("OUT env var must be set");

    let live_mode = std::env::var("PROVIDER_BASE_URL").ok().and_then(|base| {
        std::env::var("MODEL").ok().map(|model| (base, model))
    });

    // Read eval set.
    let eval_data: Value =
        serde_json::from_str(&fs::read_to_string(&eval_path).expect("cannot read EVAL_SET"))
            .expect("EVAL_SET is not valid JSON");

    // Extract top-level tool definitions.
    let tools_json = eval_data
        .get("tools")
        .and_then(|t| t.as_array())
        .expect("EVAL_SET must have a 'tools' array");

    let tool_defs = parse_tool_defs(tools_json);

    // Open output.
    let out_file = fs::File::create(&out_path).expect("cannot create OUT file");
    let mut writer = BufWriter::new(out_file);

    // ─── Process cases ─────────────────────────────────────────────────
    if let Some(cases) = eval_data.get("cases").and_then(|c| c.as_array()) {
        for case in cases {
            let id = case.get("id").and_then(|v| v.as_str()).unwrap_or("?");

            // Which tools does this case use?
            let case_tool_names: Vec<&str> = case
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let case_tools: Vec<ToolDef> = tool_defs
                .iter()
                .filter(|t| case_tool_names.contains(&t.name.as_str()))
                .cloned()
                .collect();

            // Encode tools compactly.
            let compact = match encode_tools(&case_tools) {
                Ok(c) => c,
                Err(e) => {
                    // Bypass compaction for unsupported schemas.
                    let line = json!({
                        "id": id,
                        "compacted": false,
                        "bypass_reason": e.to_string(),
                    });
                    writeln!(writer, "{}", serde_json::to_string(&line).unwrap()).unwrap();
                    continue;
                }
            };

            // Build the compact OpenAI-shaped request body.
            let messages = case.get("messages").cloned().unwrap_or(json!([]));
            let compact_request = build_compact_request(&compact, &messages);

            // Render expected calls in compact format.
            let expected = case.get("expected").and_then(|e| e.as_array());
            let rendered_calls = render_expected_calls(expected, &case_tools);

            // Decode the rendered calls back (roundtrip check).
            let roundtrip_calls = match decode_calls(&rendered_calls, &case_tools) {
                Ok(calls) => calls
                    .into_iter()
                    .map(|c| {
                        json!({
                            "name": c.name,
                            "arguments": c.arguments,
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
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

            // Live mode: send to LLM and decode response.
            if let Some((ref base_url, ref model)) = live_mode {
                match send_live_request(base_url, model, &compact_request) {
                    Ok((raw_output, live_calls)) => {
                        line["raw_output"] = json!(raw_output);
                        line["live_calls"] = json!(live_calls);
                    }
                    Err(e) => {
                        line["live_error"] = json!(e);
                    }
                }
            }

            writeln!(writer, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        }
    }

    // ─── Process decoder cases ─────────────────────────────────────────
    if let Some(decoder_cases) = eval_data.get("decoder_cases").and_then(|c| c.as_array()) {
        for dc in decoder_cases {
            let id = dc.get("id").and_then(|v| v.as_str()).unwrap_or("?");

            let case_tool_names: Vec<&str> = dc
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let case_tools: Vec<ToolDef> = tool_defs
                .iter()
                .filter(|t| case_tool_names.contains(&t.name.as_str()))
                .cloned()
                .collect();

            let chunks: Vec<&str> = dc
                .get("chunks")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            // Feed chunks to StreamDecoder.
            let mut decoder = StreamDecoder::new(case_tools);
            for chunk in &chunks {
                decoder.push(chunk);
            }
            let result = decoder.finish();

            let decoded = if result.errors.is_empty() {
                json!({
                    "calls": result.calls.iter().map(|c| json!({
                        "name": c.name,
                        "arguments": c.arguments,
                    })).collect::<Vec<_>>(),
                })
            } else {
                // Report the first error type.
                let err = &result.errors[0];
                let error_type = match err {
                    nasiko_tool_compact::DecodeError::UnknownTool(_) => "unknown_tool",
                    nasiko_tool_compact::DecodeError::MissingRequired { .. } => "invalid_arguments",
                    nasiko_tool_compact::DecodeError::InvalidArgument { .. } => "invalid_arguments",
                    nasiko_tool_compact::DecodeError::MalformedCall(_) => "malformed_call",
                    nasiko_tool_compact::DecodeError::InvalidJson { .. } => "invalid_arguments",
                };
                json!({ "error": error_type })
            };

            let line = json!({
                "id": id,
                "decoded": decoded,
            });

            writeln!(writer, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        }
    }

    writer.flush().unwrap();
    eprintln!("Eval complete. Output written to: {out_path}");
}

/// Parse the top-level `tools` array from the eval set into `ToolDef`s.
fn parse_tool_defs(tools_json: &[Value]) -> Vec<ToolDef> {
    tools_json
        .iter()
        .filter_map(|t| {
            // Tools in the eval set use the OpenAI shape: { "type": "function", "function": {...} }
            let func = t.get("function")?;
            Some(ToolDef {
                name: func.get("name")?.as_str()?.to_string(),
                description: func.get("description").and_then(|d| d.as_str()).map(String::from),
                parameters: func.get("parameters").cloned(),
            })
        })
        .collect()
}

/// Build a full OpenAI-shaped chat request body using compact tool definitions.
fn build_compact_request(compact: &CompactTools, messages: &Value) -> Value {
    let mut request_messages = vec![
        // System message with time context and compact tool definitions.
        json!({
            "role": "system",
            "content": format!("{}\n\n{}", SYSTEM_TIME_CONTEXT, compact.prompt),
        }),
    ];

    // Add the case's messages.
    if let Some(msgs) = messages.as_array() {
        for msg in msgs {
            request_messages.push(msg.clone());
        }
    }

    json!({
        "messages": request_messages,
        "temperature": 0,
    })
}

/// Render expected tool calls in compact `<<call ...>>` format.
fn render_expected_calls(expected: Option<&Vec<Value>>, _tools: &[ToolDef]) -> String {
    let Some(expected) = expected else {
        return String::new();
    };

    if expected.is_empty() {
        return String::new();
    }

    expected
        .iter()
        .filter_map(|call| {
            let name = call.get("name")?.as_str()?;
            let args = call.get("arguments")?;
            Some(format!("<<call {} {}>>", name, serde_json::to_string(args).ok()?))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Send a request to a live LLM endpoint and decode the response.
fn send_live_request(
    base_url: &str,
    model: &str,
    compact_request: &Value,
) -> Result<(String, Vec<Value>), String> {
    // Build the full request with model.
    let mut request = compact_request.clone();
    request["model"] = json!(model);

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    // Use ureq for synchronous HTTP (we're in a non-async main).
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("API_KEY"))
        .unwrap_or_default();

    let mut response = ureq::post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {}", api_key))
        .send_json(&request)
        .map_err(|e| format!("HTTP error: {e}"))?;

    let body: Value = response.body_mut().read_json()
        .map_err(|e| format!("JSON parse error: {e}"))?;

    let raw_output = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    // Extract tool definitions from compact_request for decoding.
    // We decode using the same tools the request was built with.
    let messages = compact_request.get("messages").and_then(|m| m.as_array());
    let _system_content = messages
        .and_then(|msgs| msgs.first())
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    // For live decoding, we need the tool defs. We'll parse from the compact text.
    // Since we can't easily reconstruct ToolDefs from the compact format in the example,
    // we'll just report the raw calls without full validation.
    let live_calls: Vec<Value> = extract_raw_calls(&raw_output)
        .into_iter()
        .map(|(name, args)| json!({"name": name, "arguments": args}))
        .collect();

    Ok((raw_output, live_calls))
}

/// Extract raw calls from model output without schema validation (for live mode reporting).
fn extract_raw_calls(text: &str) -> Vec<(String, Value)> {
    let mut calls = Vec::new();
    let mut pos = 0;

    while pos < text.len() {
        if let Some(start) = text[pos..].find("<<call ") {
            let marker_start = pos + start + 7; // len("<<call ")
            let rest = &text[marker_start..];

            // Extract tool name.
            let name_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .unwrap_or(rest.len());
            let name = rest[..name_end].to_string();
            let after_name = rest[name_end..].trim_start();

            // Extract JSON args.
            if after_name.starts_with('{') {
                if let Some(json_end) = find_json_end_simple(after_name) {
                    let json_str = &after_name[..json_end];
                    if let Ok(args) = serde_json::from_str::<Value>(json_str) {
                        calls.push((name, args));
                    }
                    pos = marker_start + name_end + (rest[name_end..].len() - after_name.len()) + json_end;
                    continue;
                }
            }
            pos = marker_start;
        } else {
            break;
        }
    }

    calls
}

/// Simple balanced-brace JSON end finder.
fn find_json_end_simple(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, b) in text.bytes().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}
