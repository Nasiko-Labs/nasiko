//! Compact tools evaluation runner.
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval

use std::collections::BTreeMap;
use std::io::Write;

use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_calls,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tiktoken_rs::o200k_base;

// ── Eval Data Schemas ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EvalSet {
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

// ── Main Evaluation ────────────────────────────────────────────────────────

fn main() {
    let eval_set_env = std::env::var("EVAL_SET").ok();
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-out.jsonl".into());
    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model_env = std::env::var("MODEL").ok();

    let eval_set = match eval_set_env {
        Some(ref path) if std::path::Path::new(path).exists() => {
            let raw = std::fs::read_to_string(path).expect("failed to read EVAL_SET");
            serde_json::from_str::<EvalSet>(&raw).expect("invalid EVAL_SET JSON format")
        }
        _ => default_embedded_eval_set(),
    };

    let btree_tools: BTreeMap<String, ToolDef> = eval_set
        .tools
        .into_iter()
        .map(|t| (t.function.name.clone(), t))
        .collect();

    let bpe = o200k_base().expect("failed to load o200k_base tokenizer");

    let mut out_file = std::io::BufWriter::new(
        std::fs::File::create(&out_path).expect("failed to create OUT file"),
    );

    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // Separate tracking for sets
    let mut stats_by_tool_count: BTreeMap<usize, (usize, usize)> = BTreeMap::new();

    // ── Process Prompt / Encoding Cases ────────────────────────────────────
    for case in eval_set.cases {
        let case_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| btree_tools.get(name).cloned())
            .collect();

        let tool_count = case_tools.len();
        let compact_res = encode_tools(&case_tools).expect("encode_tools failed");

        // Build messages: ensure system message with date/timezone is present
        let mut messages = case.messages.clone();
        let date_system_msg = json!({
            "role": "system",
            "content": "Today is 2026-10-02, timezone Asia/Kolkata."
        });

        messages.insert(0, date_system_msg);

        if compact_res.compacted && !compact_res.text.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": compact_res.text
            }));
        }

        let mut compact_request = json!({
            "messages": messages,
        });

        if let Some(m) = &model_env {
            compact_request["model"] = json!(m);
        }

        if !compact_res.compacted {
            compact_request["tools"] = json!(case_tools);
        }

        // Render expected calls in compact format
        let expected_tuples: Vec<(String, Value)> = case
            .expected
            .iter()
            .map(|e| (e.name.clone(), e.arguments.clone()))
            .collect();
        let rendered_calls = render_calls(&expected_tuples);

        // Decode rendered calls back
        let roundtrip_calls = decode_calls(&rendered_calls, &case_tools).unwrap_or_default();

        let mut line_obj = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": compact_res.compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });

        // ── Token Reduction Measurement ───────────────────────────────────
        let baseline_req = json!({
            "messages": case.messages,
            "tools": case_tools,
        });
        let baseline_text = serde_json::to_string(&baseline_req).unwrap_or_default();
        let compact_text = serde_json::to_string(&compact_request).unwrap_or_default();

        let baseline_tokens = bpe.encode_with_special_tokens(&baseline_text).len();
        let compact_tokens = bpe.encode_with_special_tokens(&compact_text).len();

        total_baseline_tokens += baseline_tokens;
        total_compact_tokens += compact_tokens;

        let entry = stats_by_tool_count.entry(tool_count).or_insert((0, 0));
        entry.0 += baseline_tokens;
        entry.1 += compact_tokens;

        // ── Live LLM Call (Optional) ───────────────────────────────────────
        if let (Some(base_url), Some(model)) = (&provider_base_url, &model_env) {
            if let Ok((raw_out, live_calls)) =
                execute_live_call(base_url, model, &compact_request, &case_tools)
            {
                line_obj["raw_output"] = json!(raw_out);
                line_obj["live_calls"] = json!(live_calls);
            }
        }

        writeln!(out_file, "{}", serde_json::to_string(&line_obj).unwrap()).expect("write line");
    }

    // ── Process Decoder Cases ──────────────────────────────────────────────
    for dc in eval_set.decoder_cases {
        let dc_tools: Vec<ToolDef> = dc
            .tools
            .iter()
            .filter_map(|name| btree_tools.get(name).cloned())
            .collect();

        let mut decoder = StreamDecoder::new();
        for chunk in &dc.chunks {
            decoder.push(chunk);
        }

        let decoded_value = match decoder.finish() {
            Ok(raw_calls) => {
                let mut valid_calls = Vec::new();
                let mut err_tag = None;

                for (idx, raw) in raw_calls.into_iter().enumerate() {
                    match serde_json::from_str::<Value>(&raw.args) {
                        Ok(args_val) => {
                            if let Err(e) = nasiko_tool_compact::validate::validate_call(
                                &raw.name, &args_val, &dc_tools,
                            ) {
                                err_tag = Some(e.eval_tag());
                                break;
                            }
                            let args_str = serde_json::to_string(&args_val).unwrap_or(raw.args);
                            valid_calls.push(ToolCall {
                                id: format!("call_{}", idx + 1),
                                kind: "function".to_string(),
                                function: nasiko_tool_compact::FunctionCall {
                                    name: raw.name,
                                    arguments: args_str,
                                },
                                extra: serde_json::Map::new(),
                            });
                        }
                        Err(_) => {
                            err_tag = Some("invalid_arguments");
                            break;
                        }
                    }
                }

                if let Some(tag) = err_tag {
                    json!({ "error": tag })
                } else {
                    json!({ "calls": valid_calls })
                }
            }
            Err(e) => json!({ "error": e.eval_tag() }),
        };

        let line_obj = json!({
            "id": dc.id,
            "decoded": decoded_value,
        });

        writeln!(out_file, "{}", serde_json::to_string(&line_obj).unwrap()).expect("write line");
    }

    out_file.flush().expect("flush OUT");

    // Print Token reduction summary
    let overall_reduction_pct = if total_baseline_tokens > 0 {
        (1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)) * 100.0
    } else {
        0.0
    };

    println!("=== Compact Tools Evaluation Summary ===");
    println!("Total Baseline Tokens: {total_baseline_tokens}");
    println!("Total Compact Tokens:  {total_compact_tokens}");
    println!("Overall Token Reduction: {overall_reduction_pct:.2}%\n");

    for (count, (base, comp)) in stats_by_tool_count {
        let red = (1.0 - (comp as f64 / base as f64)) * 100.0;
        println!("{count}-tool set: Baseline = {base}, Compact = {comp}, Reduction = {red:.2}%");
    }
}

fn execute_live_call(
    base_url: &str,
    model: &str,
    request_body: &Value,
    tools: &[ToolDef],
) -> Result<(String, Value), Box<dyn std::error::Error>> {
    let mut req = request_body.clone();
    req["model"] = json!(model);
    req["temperature"] = json!(0.0);

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{base_url}/chat/completions"))
        .json(&req)
        .send()?;

    let json_resp: Value = resp.json()?;
    let content = json_resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let live_calls = match decode_calls(&content, tools) {
        Ok(calls) => json!({ "calls": calls }),
        Err(err) => json!({ "error": err.eval_tag() }),
    };

    Ok((content, live_calls))
}

fn default_embedded_eval_set() -> EvalSet {
    serde_json::from_value(json!({
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "create_calendar_event",
                    "description": "Create an event in the user's calendar.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "start": { "type": "string", "format": "date-time" },
                            "duration_min": { "type": "integer" },
                            "attendees": { "type": "array", "items": { "type": "string" } },
                            "visibility": { "type": "string", "enum": ["public", "private"] }
                        },
                        "required": ["title", "start"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "send_email",
                    "description": "Send an email.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "to": { "type": "string" },
                            "subject": { "type": "string" },
                            "body": { "type": "string" }
                        },
                        "required": ["to", "subject", "body"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "search_documents",
                    "description": "Search user documents.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" },
                            "limit": { "type": "integer" }
                        },
                        "required": ["query"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather report for a location.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" },
                            "date": { "type": "string", "format": "date" }
                        },
                        "required": ["location"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "update_user_profile",
                    "description": "Update user profile settings.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "user_id": { "type": "string" },
                            "name": { "type": "string" },
                            "email": { "type": "string" },
                            "role": { "type": "string", "enum": ["admin", "user"] }
                        },
                        "required": ["user_id"]
                    }
                }
            }
        ],
        "cases": [
            {
                "id": "ct-001",
                "tools": ["create_calendar_event", "send_email"],
                "messages": [
                    { "role": "user", "content": "Schedule a meeting with Riya on Oct 5 at 3pm for 1 hour." }
                ],
                "expected": [
                    {
                        "name": "create_calendar_event",
                        "arguments": {
                            "title": "Design review",
                            "start": "2026-10-05T15:00:00+05:30",
                            "attendees": ["riya@example.com"]
                        }
                    }
                ]
            },
            {
                "id": "ct-002-5tools",
                "tools": ["create_calendar_event", "send_email", "search_documents", "get_weather", "update_user_profile"],
                "messages": [
                    { "role": "user", "content": "What is the weather in Bengaluru today?" }
                ],
                "expected": [
                    {
                        "name": "get_weather",
                        "arguments": {
                            "location": "Bengaluru",
                            "date": "2026-10-02"
                        }
                    }
                ]
            }
        ],
        "decoder_cases": [
            {
                "id": "dc-001",
                "tools": ["create_calendar_event"],
                "chunks": [
                    "<<call create_calendar_event {\"title\":",
                    "\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"
                ]
            },
            {
                "id": "dc-002",
                "tools": ["create_calendar_event"],
                "chunks": [
                    "<<call unknown_tool {}>>"
                ]
            },
            {
                "id": "dc-003",
                "tools": ["create_calendar_event"],
                "chunks": [
                    "<<call create_calendar_event {\"title\":\"Missing start\"}>>"
                ]
            }
        ]
    })).unwrap()
}
