//! Compact tool-schema eval.
//!
//! # Usage
//!
//! ```sh
//! # Offline (default, deterministic, no network or credentials):
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! # Live (adds raw_output + live_calls to each line):
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   PROVIDER_BASE_URL=https://api.openai.com/v1 MODEL=gpt-4o-mini \
//!   OPENAI_API_KEY=sk-... \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Eval set format
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "purpose": "...",
//!   "tools": [ { "type":"function", "function": { "name":"...", ... } } ],
//!   "cases": [
//!     {
//!       "id": "ct-001",
//!       "tool_names": ["create_calendar_event", "send_email"],
//!       "user_message": "Schedule a meeting tomorrow ...",
//!       "expected": [
//!         { "name": "create_calendar_event", "arguments": { ... } }
//!       ]
//!     }
//!   ],
//!   "decoder_cases": [
//!     {
//!       "id": "dc-001",
//!       "tool_names": ["..."],
//!       "chunks": ["<<call ping ", "{", "}}>>"]
//!     }
//!   ]
//! }
//! ```
//!
//! # Output JSONL format (one line per case)
//!
//! Normal case:
//! ```json
//! {"id":"ct-001","compact_request":{...},"compacted":true,
//!  "rendered_calls":"<<call ... {}>>","roundtrip_calls":[{"name":"...","arguments":"..."}]}
//! ```
//!
//! Decoder case:
//! ```json
//! {"id":"dc-001","decoded":{"calls":[{"name":"...","arguments":"..."}]}}
//! ```
//! or on error:
//! ```json
//! {"id":"dc-001","decoded":{"error":"unknown_tool"}}
//! ```
//!
//! Live mode adds `"raw_output"` and `"live_calls"` to normal-case lines.

use std::collections::HashMap;
use std::io::Write;

use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolDef};
use serde_json::{json, Value};
use tiktoken_rs::o200k_base;

// ── Reference time ────────────────────────────────────────────────────────────

const REFERENCE_TIME: &str = "2026-10-02";
const REFERENCE_TZ: &str = "Asia/Kolkata";

fn system_message_prefix() -> String {
    format!("Today is Friday, {REFERENCE_TIME} ({REFERENCE_TZ}).")
}

// ── Render helpers ────────────────────────────────────────────────────────────

/// Render expected calls into the compact grammar string.
///
/// This is used for `rendered_calls` in the output and as input to the
/// roundtrip decoder. The grammar is exactly `<<call NAME {json}>>`.
fn render_calls(expected: &[Value]) -> String {
    expected
        .iter()
        .map(|call| {
            let name = call["name"].as_str().unwrap_or("unknown");
            let args = &call["arguments"];
            // Re-serialize args as compact JSON.
            let args_str = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
            format!("<<call {name} {args_str}>>")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the system message content that includes reference time + compact tools.
fn build_system_content(compact: &nasiko_tool_compact::CompactTools) -> String {
    let mut parts = vec![system_message_prefix()];

    if !compact.tool_block.is_empty() {
        parts.push(compact.call_instructions.clone());
        parts.push(format!("Available tools:\n{}", compact.tool_block));
    }

    parts.join("\n\n")
}

/// Build the full OpenAI-shaped chat request body for a case.
fn build_compact_request(
    user_message: &str,
    compact: &nasiko_tool_compact::CompactTools,
    tools: &[ToolDef],
) -> Value {
    let system_content = build_system_content(compact);

    // Tools still passed natively (fallback tools).
    let native_tools: Vec<Value> = compact
        .native_tools
        .iter()
        .map(|nt| serde_json::to_value(&nt.tool).unwrap())
        .collect();

    let mut req = json!({
        "messages": [
            {"role": "system", "content": system_content},
            {"role": "user", "content": user_message}
        ],
        "temperature": 0
    });

    if !native_tools.is_empty() {
        req["tools"] = json!(native_tools);
    }

    // Record which tools were compacted vs. native.
    let _ = tools; // available for reference; compact carries the info.

    req
}

// ── Token counting ────────────────────────────────────────────────────────────

fn count_tokens(s: &str) -> usize {
    let bpe = o200k_base().unwrap();
    bpe.encode_with_special_tokens(s).len()
}

fn request_token_count(req: &Value) -> usize {
    let s = serde_json::to_string(req).unwrap_or_default();
    count_tokens(&s)
}

fn baseline_request_token_count(user_message: &str, tools: &[ToolDef]) -> usize {
    let system_content = system_message_prefix();
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| serde_json::to_value(t).unwrap())
        .collect();
    let req = json!({
        "messages": [
            {"role": "system", "content": system_content},
            {"role": "user", "content": user_message}
        ],
        "tools": tools_json,
        "temperature": 0
    });
    let s = serde_json::to_string(&req).unwrap_or_default();
    count_tokens(&s)
}

// ── Live mode ─────────────────────────────────────────────────────────────────

// Live mode: when PROVIDER_BASE_URL and MODEL are set, make a real request.
async fn maybe_live_call(req: &Value, tools: &[ToolDef]) -> Option<(String, Vec<Value>)> {
    let base_url = std::env::var("PROVIDER_BASE_URL").ok()?;
    let model = std::env::var("MODEL").ok()?;
    // Auth token read from environment only.
    let auth = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("API_KEY"))
        .unwrap_or_default();

    let mut body = req.clone();
    body["model"] = json!(model);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;

    let mut request = client
        .post(format!("{base_url}/chat/completions"))
        .header("Content-Type", "application/json");

    if !auth.is_empty() {
        request = request.header("Authorization", format!("Bearer {auth}"));
    }

    let resp: Value = request.json(&body).send().await.ok()?.json().await.ok()?;
    let raw_output = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // Decode live output back to tool calls.
    let live_calls = match decode_calls(&raw_output, tools) {
        Ok(calls) => calls
            .iter()
            .map(|c| {
                json!({
                    "name": c.function.name,
                    "arguments": c.function.arguments
                })
            })
            .collect(),
        Err(_) => vec![],
    };

    Some((raw_output, live_calls))
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let eval_path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let raw = std::fs::read_to_string(&eval_path).expect("read EVAL_SET");
    let raw = raw.trim_start_matches('\u{feff}');
    let data: Value = serde_json::from_str(raw).expect("valid eval JSON");

    // Build a map from tool name → ToolDef.
    let empty_tools_list = vec![];
    let all_tools: Vec<ToolDef> = data["tools"]
        .as_array()
        .unwrap_or(&empty_tools_list)
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();

    let tool_map: HashMap<&str, &ToolDef> = all_tools
        .iter()
        .map(|t| (t.function.name.as_str(), t))
        .collect();

    let mut out =
        std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT file"));

    // ── Token measurement accumulators ─────────────────────────────────────
    let mut total_baseline_tokens: usize = 0;
    let mut total_compact_tokens: usize = 0;
    let mut official_cases_count: usize = 0;
    let mut skipped_cases_count: usize = 0;

    // ── Normal cases ──────────────────────────────────────────────────────
    let empty_cases = vec![];
    let cases = data["cases"].as_array().unwrap_or(&empty_cases);

    for case in cases {
        let id = case["id"].as_str().unwrap_or("?");
        let user_message = case.get("messages")
            .and_then(|m| m.as_array())
            .and_then(|arr| arr.iter().find(|msg| msg["role"] == "user"))
            .and_then(|msg| msg["content"].as_str())
            .or_else(|| case["user_message"].as_str())
            .unwrap_or("");

        // Resolve tools for this case.
        let tool_names_val = case.get("tools").or_else(|| case.get("tool_names"));
        let empty_names: &[Value] = &[];
        let case_tools: Vec<ToolDef> = tool_names_val
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(empty_names)
            .iter()
            .filter_map(|n| n.as_str())
            .filter_map(|name| tool_map.get(name).copied().cloned())
            .collect();

        let empty_expected: &[Value] = &[];
        let expected: Vec<Value> = case["expected"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(empty_expected)
            .to_vec();

        // Encode tools.
        let compact = encode_tools(&case_tools).expect("encode_tools");
        let compacted = !compact.tool_block.is_empty();

        // Build compact request.
        let compact_request = build_compact_request(user_message, &compact, &case_tools);

        // Render expected calls in the compact grammar.
        let rendered_calls = render_calls(&expected);

        // Roundtrip: decode the rendered calls.
        let roundtrip_calls: Vec<Value> = if rendered_calls.is_empty() {
            vec![]
        } else {
            match decode_calls(&rendered_calls, &case_tools) {
                Ok(calls) => calls
                    .iter()
                    .map(|c| {
                        let parsed_args: Value =
                            serde_json::from_str(&c.function.arguments).unwrap_or(json!({}));
                        json!({
                            "name": c.function.name,
                            "arguments": parsed_args
                        })
                    })
                    .collect(),
                Err(e) => {
                    eprintln!("WARN: roundtrip decode failed for {id}: {e}");
                    vec![]
                }
            }
        };

        // Token counting.
        let baseline_tokens = baseline_request_token_count(user_message, &case_tools);
        let compact_tokens = request_token_count(&compact_request);

        total_baseline_tokens += baseline_tokens;
        if compacted {
            total_compact_tokens += compact_tokens;
        } else {
            // Skipped: count as 0% savings (i.e. baseline == compact).
            total_compact_tokens += baseline_tokens;
            skipped_cases_count += 1;
        }
        official_cases_count += 1;

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });

        // Live mode.
        if let Some((raw_output, live_calls)) =
            maybe_live_call(&compact_request, &case_tools).await
        {
            line["raw_output"] = json!(raw_output);
            line["live_calls"] = json!(live_calls);
        }

        writeln!(out, "{line}").expect("write OUT");
    }

    // ── Decoder cases ─────────────────────────────────────────────────────
    let empty_dc = vec![];
    let decoder_cases = data["decoder_cases"].as_array().unwrap_or(&empty_dc);

    for case in decoder_cases {
        let id = case["id"].as_str().unwrap_or("?");

        let tool_names_val = case.get("tools").or_else(|| case.get("tool_names"));
        let empty_dt: &[Value] = &[];
        let case_tools: Vec<ToolDef> = tool_names_val
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(empty_dt)
            .iter()
            .filter_map(|n| n.as_str())
            .filter_map(|name| tool_map.get(name).copied().cloned())
            .collect();

        let empty_chunks: &[Value] = &[];
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(empty_chunks)
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        let mut dec = StreamDecoder::new();
        let mut push_err: Option<nasiko_tool_compact::Error> = None;
        for chunk in &chunks {
            if let Err(e) = dec.push(chunk) {
                push_err = Some(e);
                break;
            }
        }

        let decoded_val = match push_err {
            Some(e) => json!({"error": error_label(&e)}),
            None => match dec.finish(&case_tools) {
                Ok(calls) => json!({
                    "calls": calls.iter().map(|c| {
                        let parsed_args: Value =
                            serde_json::from_str(&c.function.arguments).unwrap_or(json!({}));
                        json!({
                            "name": c.function.name,
                            "arguments": parsed_args,
                        })
                    }).collect::<Vec<_>>()
                }),
                Err(e) => json!({"error": error_label(&e)}),
            },
        };

        let line = json!({"id": id, "decoded": decoded_val});
        writeln!(out, "{line}").expect("write OUT");
    }

    out.flush().expect("flush OUT");

    // ── Token report (stderr) ─────────────────────────────────────────────
    let reduction = if total_baseline_tokens > 0 {
        1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)
    } else {
        0.0
    };
    eprintln!(
        "Token reduction report ({official_cases_count} official cases, {skipped_cases_count} skipped/native):"
    );
    eprintln!("  Baseline total tokens : {total_baseline_tokens}");
    eprintln!("  Compact  total tokens : {total_compact_tokens}");
    eprintln!(
        "  Reduction (1 - compact/baseline): {:.1}%",
        reduction * 100.0
    );
}

/// Convert an error to a stable label string for the decoder case output.
fn error_label(e: &nasiko_tool_compact::Error) -> &'static str {
    match e {
        nasiko_tool_compact::Error::UnknownTool(_) => "unknown_tool",
        nasiko_tool_compact::Error::InvalidArguments { .. } => "invalid_arguments",
        nasiko_tool_compact::Error::InvalidCall(_) => "invalid_call",
        nasiko_tool_compact::Error::UnterminatedCall => "unterminated_call",
        nasiko_tool_compact::Error::BodyTooLarge { .. } => "body_too_large",
        nasiko_tool_compact::Error::DepthExceeded { .. } => "depth_exceeded",
        nasiko_tool_compact::Error::UnsupportedSchema(_, _) => "unsupported_schema",
    }
}
