//! Compact-tools evaluator (P1 submission).
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Optional live mode (needs an OpenAI-compatible endpoint):
//!   PROVIDER_BASE_URL=https://... MODEL=gpt-4o ... (same command)
//!
//! Env vars:
//!   EVAL_SET         Path to the public eval JSON (required).
//!   OUT              Output JSONL path (default: compact-tools-out.jsonl).
//!   PROVIDER_BASE_URL  Base URL for live mode (e.g. https://api.openai.com/v1).
//!   MODEL            Model ID for live mode (e.g. gpt-4o).
//!   OPENAI_API_KEY   Key for live mode (or PROVIDER_API_KEY).
//!
//! Output format (one JSON line per case):
//!
//! For `cases`:
//!   {"id":"ct-001","compact_request":{...},"compacted":true,"rendered_calls":"<<call ...>>","roundtrip_calls":[...]}
//!
//! For `decoder_cases`:
//!   {"id":"dc-002","decoded":{"calls":[...]}}   or
//!   {"id":"dc-002","decoded":{"error":"unknown_tool"}}
//!
//! Live mode adds `raw_output` and `live_calls` to each `cases` line.

use std::collections::HashMap;
use std::io::Write;

use nasiko_tool_compact::{
    decode::StreamDecoder,
    decode_calls, encode_tools,
    types::{ToolCall, ToolDef, FunctionDef},
};
use serde_json::{Value, json};

// ─── fixed reference time for relative-date resolution ───────────────────────
const SYSTEM_DATE_HINT: &str =
    "Today is 2026-10-02. Timezone: Asia/Kolkata (IST, UTC+05:30). \
     Use this to resolve relative dates like 'tomorrow' or 'Monday'.";

fn main() {
    let eval_path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path =
        std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let raw = std::fs::read_to_string(&eval_path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    // Build tool registry: name → ToolDef
    let tool_registry: HashMap<String, ToolDef> = data["tools"]
        .as_array()
        .expect("top-level tools array")
        .iter()
        .map(|v| {
            let name = v["function"]["name"]
                .as_str()
                .expect("tool name")
                .to_string();
            let tool = parse_tool_def(v);
            (name, tool)
        })
        .collect();

    // Live mode config
    let provider_base = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("PROVIDER_API_KEY"))
        .ok();
    let live_mode =
        provider_base.is_some() && model.is_some() && api_key.is_some();

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path).expect("create OUT"),
    );

    // ── cases ────────────────────────────────────────────────────────────────
    if let Some(cases) = data["cases"].as_array() {
        for case in cases {
            let id = case["id"].as_str().expect("case id");
            let tool_names: Vec<&str> = case["tools"]
                .as_array()
                .expect("tools array in case")
                .iter()
                .map(|v| v.as_str().expect("tool name ref"))
                .collect();
            let tools: Vec<ToolDef> = tool_names
                .iter()
                .filter_map(|n| tool_registry.get(*n).cloned())
                .collect();
            let messages = case["messages"].as_array().expect("messages");

            // Encode tools.
            let encoded = encode_tools(&tools).expect("encode_tools");
            let compacted = encoded.is_encoded();

            // Build the compact_request (OpenAI chat request body).
            let compact_request = build_compact_request(&messages, &tools, &encoded, &model);

            // Render expected calls in compact format (for round-trip test).
            let rendered_calls = render_expected_calls(case);

            // Round-trip: decode the rendered calls back.
            let roundtrip_result = decode_calls(&rendered_calls, &tools);
            let roundtrip_calls: Vec<Value> = roundtrip_result
                .calls
                .iter()
                .map(tool_call_to_openai_json)
                .collect();

            let mut line = json!({
                "id": id,
                "compact_request": compact_request,
                "compacted": compacted,
                "rendered_calls": rendered_calls,
                "roundtrip_calls": roundtrip_calls,
            });

            // Live mode: send to model and decode its output.
            if live_mode {
                if let (Some(base), Some(m), Some(key)) =
                    (&provider_base, &model, &api_key)
                {
                    match call_live_api(base, m, key, &compact_request) {
                        Ok(raw_output) => {
                            let live_result = decode_calls(&raw_output, &tools);
                            let live_calls_json = decode_result_to_json(&live_result);
                            line["raw_output"] = json!(raw_output);
                            line["live_calls"] = live_calls_json;
                        }
                        Err(e) => {
                            line["live_error"] = json!(e.to_string());
                        }
                    }
                }
            }

            writeln!(out, "{line}").expect("write OUT");
        }
    }

    // ── decoder_cases ────────────────────────────────────────────────────────
    if let Some(dcases) = data["decoder_cases"].as_array() {
        for dc in dcases {
            let id = dc["id"].as_str().expect("decoder_case id");
            let tool_names: Vec<&str> = dc["tools"]
                .as_array()
                .expect("tools in decoder_case")
                .iter()
                .map(|v| v.as_str().expect("tool name"))
                .collect();
            let tools: Vec<ToolDef> = tool_names
                .iter()
                .filter_map(|n| tool_registry.get(*n).cloned())
                .collect();

            let chunks: Vec<&str> = dc["chunks"]
                .as_array()
                .expect("chunks")
                .iter()
                .map(|v| v.as_str().expect("chunk str"))
                .collect();

            let mut decoder = StreamDecoder::new(tools.clone());
            for chunk in &chunks {
                decoder.push(chunk);
            }
            let result = decoder.finish();
            let decoded_json = decode_result_to_json(&result);

            let line = json!({
                "id": id,
                "decoded": decoded_json,
            });
            writeln!(out, "{line}").expect("write OUT");
        }
    }

    out.flush().expect("flush OUT");
    eprintln!("✓ compact_tools_eval complete → {out_path}");
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn parse_tool_def(v: &Value) -> ToolDef {
    let func = &v["function"];
    ToolDef {
        kind: v["type"].as_str().unwrap_or("function").to_string(),
        function: FunctionDef {
            name: func["name"].as_str().expect("name").to_string(),
            description: func["description"].as_str().map(str::to_string),
            parameters: func.get("parameters").cloned(),
        },
    }
}

fn build_compact_request(
    messages: &[Value],
    tools: &[ToolDef],
    encoded: &nasiko_tool_compact::CompactTools,
    model: &Option<String>,
) -> Value {
    let mut msgs: Vec<Value> = vec![json!({
        "role": "system",
        "content": SYSTEM_DATE_HINT
    })];

    // Inject compact tool definitions + call-format instructions as a system message.
    if let Some(compact_text) = encoded.text() {
        if !compact_text.is_empty() {
            msgs.push(json!({
                "role": "system",
                "content": format!("Available tools:\n{compact_text}")
            }));
        }
    }

    // Add the original user/assistant messages (no native `tools` field).
    for msg in messages {
        msgs.push(msg.clone());
    }

    let mut req = json!({
        "messages": msgs,
        "temperature": 0,
    });

    if let Some(m) = model {
        req["model"] = json!(m);
    }

    // If compaction was bypassed, fall back to native tools array.
    if !encoded.is_encoded() {
        let native_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut func = json!({
                    "name": t.function.name,
                });
                if let Some(d) = &t.function.description {
                    func["description"] = json!(d);
                }
                if let Some(p) = &t.function.parameters {
                    func["parameters"] = p.clone();
                }
                json!({ "type": "function", "function": func })
            })
            .collect();
        req["tools"] = json!(native_tools);
    }

    req
}

/// Render the expected calls from a case in `<<call name {...}>>` format.
/// Used to test the round-trip path.
fn render_expected_calls(case: &Value) -> String {
    let expected = match case["expected"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    expected
        .iter()
        .map(|e| {
            let name = e["name"].as_str().unwrap_or("unknown");
            let args = e
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
            format!("<<call {name} {args_str}>>")
        })
        .collect::<Vec<_>>()
        .join("")
}

fn tool_call_to_openai_json(tc: &ToolCall) -> Value {
    json!({
        "type": tc.kind,
        "function": {
            "name": tc.function.name,
            "arguments": tc.function.arguments,
        }
    })
}

fn decode_result_to_json(result: &nasiko_tool_compact::decode::DecodeResult) -> Value {
    if let Some(err) = &result.error {
        json!({ "error": err.as_wire() })
    } else {
        let calls: Vec<Value> = result.calls.iter().map(tool_call_to_openai_json).collect();
        json!({ "calls": calls })
    }
}

// ─── live mode ───────────────────────────────────────────────────────────────

fn call_live_api(
    base_url: &str,
    model: &str,
    api_key: &str,
    request: &Value,
) -> Result<String, String> {
    let url = format!("{base_url}/chat/completions");
    // Build body with the correct model field set.
    let mut body = request.clone();
    body["model"] = json!(model);
    body["temperature"] = json!(0);

    let client = ureq::Agent::new_with_defaults();
    let resp = client
        .post(&url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| e.to_string())?;

    let text = resp.into_body().read_to_string().map_err(|e| e.to_string())?;
    let parsed: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let content = parsed["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(content)
}

