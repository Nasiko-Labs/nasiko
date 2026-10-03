//! P1 — Compact Tool Schemas: Evaluation Example
//!
//! Runs the public `compact-tools-eval-v1` evaluation dataset offline
//! (or live when `PROVIDER_BASE_URL` and `MODEL` are set).
//!
//! # Usage
//!
//! ```sh
//! # Offline (default):
//! EVAL_SET=/tmp/compact-tools-eval.json \
//! OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! # Live (optional):
//! PROVIDER_BASE_URL=https://api.openai.com/v1 \
//! MODEL=gpt-4o-mini \
//! OPENAI_API_KEY=sk-... \
//! EVAL_SET=/tmp/compact-tools-eval.json \
//! OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Output contract (JSONL, one record per case)
//!
//! For `cases`:
//! ```json
//! {
//!   "id": "ct-001",
//!   "kind": "case",
//!   "compact_request": { ... },
//!   "compacted": { "schema_text": "...", "grammar_hint": "...", "system_prompt_tokens": 42 },
//!   "rendered_calls": [...],
//!   "roundtrip_calls": [...],
//!   "live_calls": [...],    // only when PROVIDER_BASE_URL is set
//!   "raw_output": "...",    // only when PROVIDER_BASE_URL is set
//!   "token_savings": { "original_tokens": 100, "compact_tokens": 42 }
//! }
//! ```
//!
//! For `decoder_cases`:
//! ```json
//! {
//!   "id": "dc-001",
//!   "kind": "decoder_case",
//!   "chunks": [...],
//!   "calls": [...],
//!   "error": null
//! }
//! ```
//!
//! # IMPORTANT
//!
//! This file uses `FIXTURE` results only when `EVAL_SET` is not set.
//! Results are clearly labeled. Official evaluation requires `EVAL_SET`
//! pointing to the downloaded dataset.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use nasiko_tool_compact::{
    FunctionDef, StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiktoken_rs::o200k_base;

// ─── Eval dataset schema ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EvalSet {
    schema_version: String,
    tools: Vec<EvalToolDef>,
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

/// OpenAI-compatible tool definition in the eval JSON.
#[derive(Debug, Deserialize, Clone)]
struct EvalToolDef {
    #[serde(rename = "type")]
    kind: String,
    function: EvalFunctionDef,
}

#[derive(Debug, Deserialize, Clone)]
struct EvalFunctionDef {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    r#match: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: DecoderExpected,
}

#[derive(Debug, Deserialize)]
struct DecoderExpected {
    #[serde(default)]
    calls: Vec<Value>,
    #[serde(default)]
    error: Option<String>,
}

// ─── Output record shapes ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct CaseOutput {
    id: String,
    kind: &'static str,
    compact_request: Value,
    compacted: CompactedInfo,
    rendered_calls: Vec<Value>,
    roundtrip_calls: Vec<Value>,
    token_savings: TokenSavings,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Vec<Value>>,
}

#[derive(Serialize)]
struct CompactedInfo {
    schema_text: String,
    grammar_hint: String,
    system_prompt_tokens: usize,
}

#[derive(Serialize)]
struct TokenSavings {
    original_tokens: usize,
    compact_tokens: usize,
}

#[derive(Serialize)]
struct DecoderOutput {
    id: String,
    kind: &'static str,
    chunks: Vec<String>,
    calls: Vec<Value>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_error: Option<String>,
    passed: bool,
}

// ─── Conversion helpers ───────────────────────────────────────────────────────

fn eval_tool_to_tooldef(t: &EvalToolDef) -> ToolDef {
    ToolDef {
        kind: t.kind.clone(),
        function: FunctionDef {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        },
    }
}

fn toolcall_to_value(tc: &ToolCall) -> Value {
    json!({
        "name": tc.name,
        "arguments": tc.arguments
    })
}

// ─── Token counting ───────────────────────────────────────────────────────────

fn count_tokens(bpe: &tiktoken_rs::CoreBPE, text: &str) -> usize {
    bpe.encode_with_special_tokens(text).len()
}

// ─── Live call (optional) ─────────────────────────────────────────────────────

fn try_live_call(
    compact_request: &Value,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Option<(String, Vec<Value>)> {
    let mut req_body = compact_request.clone();
    if let Some(obj) = req_body.as_object_mut() {
        obj.insert("model".to_string(), json!(model));
        obj.insert("temperature".to_string(), json!(0));
    }

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body_str = serde_json::to_string(&req_body).ok()?;

    let resp = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .send(body_str.as_bytes())
        .ok()?;

    if resp.status() != 200 {
        return None;
    }

    let resp_json: Value = resp.into_body().read_json().ok()?;
    let content = resp_json
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    Some((content, vec![]))
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    // Read env vars.
    let eval_set_path = std::env::var("EVAL_SET").ok();
    let out_path = std::env::var("OUT").ok();
    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    // Never log API keys.
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("API_KEY"))
        .unwrap_or_default();

    let live_mode = provider_base_url.is_some() && model.is_some();

    // Load evaluation dataset.
    let eval_set: EvalSet = if let Some(path) = eval_set_path {
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read EVAL_SET at {}: {}", path, e));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("Failed to parse EVAL_SET JSON: {}", e))
    } else {
        // Offline fixture (development only — not official eval results).
        eprintln!(
            "[compact_tools_eval] WARNING: EVAL_SET not set. Using local development fixture."
        );
        eprintln!(
            "[compact_tools_eval] Set EVAL_SET=/path/to/compact-tools-eval.json for official results."
        );
        load_dev_fixture()
    };

    assert_eq!(
        eval_set.schema_version, "compact-tools-eval-v1",
        "Unsupported eval schema version: {}",
        eval_set.schema_version
    );

    // Build a lookup table for tool definitions.
    let tool_map: HashMap<String, EvalToolDef> = eval_set
        .tools
        .iter()
        .map(|t| (t.function.name.clone(), t.clone()))
        .collect();

    // Initialize tiktoken with o200k_base.
    let bpe = o200k_base().expect("Failed to load o200k_base tokenizer");

    // Open output file.
    let mut out: Box<dyn Write> = if let Some(ref path) = out_path {
        Box::new(
            fs::File::create(path)
                .unwrap_or_else(|e| panic!("Cannot create OUT file {}: {}", path, e)),
        )
    } else {
        Box::new(std::io::stdout())
    };

    // ─── Process eval cases ───────────────────────────────────────────────────
    for case in &eval_set.cases {
        let case_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tool_map.get(name).map(eval_tool_to_tooldef))
            .collect();

        // Encode tools into compact form.
        let compact = encode_tools(&case_tools)
            .unwrap_or_else(|e| panic!("encode_tools failed for case {}: {}", case.id, e));

        let system_prompt_tokens = count_tokens(&bpe, &compact.system_prompt_block());

        // Build the compact_request: messages with compact schema in system prompt.
        let mut messages = vec![json!({
            "role": "system",
            "content": compact.system_prompt_block()
        })];
        messages.extend(case.messages.iter().cloned());

        // Count tokens for the standard (non-compact) tool schema.
        let standard_tools_json = serde_json::to_string(
            &case_tools
                .iter()
                .map(|t| json!({ "type": t.kind, "function": { "name": t.function.name, "description": t.function.description, "parameters": t.function.parameters } }))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default();
        let original_tokens = count_tokens(&bpe, &standard_tools_json);
        let compact_tokens = system_prompt_tokens;

        let compact_request = json!({
            "messages": messages,
        });

        // rendered_calls: decode the expected calls using our parser
        // (simulate what would happen if the model returned them verbatim).
        let rendered_calls: Vec<Value> = case
            .expected
            .iter()
            .filter_map(|exp| {
                let name = exp.get("name")?.as_str()?;
                let args = exp.get("arguments")?.clone();
                let text = format!(
                    "<<call {} {}>>",
                    name,
                    serde_json::to_string(&args).unwrap_or_default()
                );
                decode_calls(&text, &case_tools)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .map(|c| toolcall_to_value(&c))
            })
            .collect();

        // roundtrip_calls: verify schema round-trip via decode_tools.
        let roundtrip_calls: Vec<Value> = decode_tools(&compact)
            .map(|recovered_tools| {
                // Re-encode and decode; should be identical.
                rendered_calls
                    .iter()
                    .filter_map(|rc| {
                        let name = rc.get("name")?.as_str()?;
                        let args = rc.get("arguments")?.clone();
                        let text = format!(
                            "<<call {} {}>>",
                            name,
                            serde_json::to_string(&args).unwrap_or_default()
                        );
                        decode_calls(&text, &recovered_tools)
                            .ok()
                            .and_then(|v| v.into_iter().next())
                            .map(|c| toolcall_to_value(&c))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Live mode (optional).
        let (raw_output, live_calls) = if live_mode {
            if let Some((raw, _)) = try_live_call(
                &compact_request,
                provider_base_url.as_deref().unwrap_or(""),
                model.as_deref().unwrap_or(""),
                &api_key,
            ) {
                // Decode the live model output.
                let lc: Vec<Value> = decode_calls(&raw, &case_tools)
                    .unwrap_or_default()
                    .iter()
                    .map(toolcall_to_value)
                    .collect();
                (Some(raw), Some(lc))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let record = CaseOutput {
            id: case.id.clone(),
            kind: "case",
            compact_request,
            compacted: CompactedInfo {
                schema_text: compact.schema_text.clone(),
                grammar_hint: compact.grammar_hint.clone(),
                system_prompt_tokens,
            },
            rendered_calls,
            roundtrip_calls,
            token_savings: TokenSavings {
                original_tokens,
                compact_tokens,
            },
            raw_output,
            live_calls,
        };

        let line = serde_json::to_string(&record).expect("Failed to serialize case output");
        writeln!(out, "{}", line).expect("Failed to write output");
    }

    // ─── Process decoder cases ────────────────────────────────────────────────
    for dc in &eval_set.decoder_cases {
        let dc_tools: Vec<ToolDef> = dc
            .tools
            .iter()
            .filter_map(|name| tool_map.get(name).map(eval_tool_to_tooldef))
            .collect();

        // Feed chunks through StreamDecoder (same grammar → direct pass-through).
        let mut decoder = StreamDecoder::new(&dc_tools);
        let mut all_calls: Vec<ToolCall> = Vec::new();
        let mut decode_error: Option<String> = None;

        'chunks: for chunk in &dc.chunks {
            match decoder.push(chunk) {
                Ok((_, calls)) => all_calls.extend(calls),
                Err(e) => {
                    decode_error = Some(error_kind(&e));
                    break 'chunks;
                }
            }
        }

        if decode_error.is_none() {
            match decoder.flush() {
                Ok((_, calls)) => all_calls.extend(calls),
                Err(e) => decode_error = Some(error_kind(&e)),
            }
        }

        let calls_json: Vec<Value> = all_calls.iter().map(toolcall_to_value).collect();

        let passed = if let Some(ref expected_err) = dc.expected.error {
            // Expect an error.
            decode_error.as_deref() == Some(expected_err.as_str())
        } else {
            // Expect calls to match expected.calls structurally.
            decode_error.is_none() && calls_match_expected(&calls_json, &dc.expected.calls)
        };

        let record = DecoderOutput {
            id: dc.id.clone(),
            kind: "decoder_case",
            chunks: dc.chunks.clone(),
            calls: calls_json,
            error: decode_error,
            expected_error: dc.expected.error.clone(),
            passed,
        };

        let line = serde_json::to_string(&record).expect("Failed to serialize decoder output");
        writeln!(out, "{}", line).expect("Failed to write output");
    }

    eprintln!(
        "[compact_tools_eval] Done. {} cases + {} decoder_cases written.",
        eval_set.cases.len(),
        eval_set.decoder_cases.len()
    );
    if out_path.is_some() {
        eprintln!(
            "[compact_tools_eval] Output: {}",
            out_path.as_deref().unwrap_or("stdout")
        );
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Map a DecodeError to the string expected by the eval schema.
fn error_kind(e: &nasiko_tool_compact::DecodeError) -> String {
    match e {
        nasiko_tool_compact::DecodeError::UnknownTool { .. } => "unknown_tool".to_string(),
        nasiko_tool_compact::DecodeError::InvalidArguments { .. } => {
            "invalid_arguments".to_string()
        }
        nasiko_tool_compact::DecodeError::MalformedCall { .. } => "malformed_call".to_string(),
    }
}

/// Check whether decoded calls structurally match the expected array.
/// Name must match exactly; argument keys are compared for presence.
fn calls_match_expected(calls: &[Value], expected: &[Value]) -> bool {
    if calls.len() != expected.len() {
        return false;
    }
    for (call, exp) in calls.iter().zip(expected.iter()) {
        let call_name = call.get("name").and_then(Value::as_str).unwrap_or("");
        let exp_name = exp.get("name").and_then(Value::as_str).unwrap_or("");
        if call_name != exp_name {
            return false;
        }
        // Check each expected argument key is present.
        if let Some(exp_args) = exp.get("arguments").and_then(Value::as_object) {
            let call_args = call.get("arguments");
            for (k, v) in exp_args {
                let got = call_args.and_then(|a| a.get(k));
                if got != Some(v) {
                    return false;
                }
            }
        }
    }
    true
}

/// Development fixture (NOT official eval data).
///
/// Used only when EVAL_SET is not set. Results are labeled as fixture
/// in the stderr output and must never be presented as official results.
fn load_dev_fixture() -> EvalSet {
    // Try to load from the fixtures directory relative to the project root.
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..") // up from llm-router/
        .join("tool-compact")
        .join("tests")
        .join("fixtures")
        .join("compact-tools-eval.json");

    if fixture_path.exists() {
        let raw = fs::read_to_string(&fixture_path)
            .unwrap_or_else(|e| panic!("Failed to read fixture: {}", e));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("Failed to parse fixture: {}", e))
    } else {
        // Minimal inline fixture for offline CI.
        eprintln!("[compact_tools_eval] Fixture file not found; using minimal inline fixture.");
        serde_json::from_str(MINIMAL_FIXTURE).expect("Inline fixture is valid JSON")
    }
}

const MINIMAL_FIXTURE: &str = r#"{
  "schema_version": "compact-tools-eval-v1",
  "purpose": "Minimal inline fixture for offline CI — not official eval data.",
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "create_calendar_event",
        "description": "Create an event in the user's calendar.",
        "parameters": {
          "type": "object",
          "properties": {
            "title": { "type": "string", "description": "Event title" },
            "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" }
          },
          "required": ["title", "start"]
        }
      }
    }
  ],
  "cases": [
    {
      "id": "fixture-001",
      "tools": ["create_calendar_event"],
      "messages": [
        { "role": "user", "content": "Book a design review Monday 3pm IST" }
      ],
      "expected": [
        {
          "name": "create_calendar_event",
          "arguments": {
            "title": "Design review",
            "start": "2026-10-05T15:00:00+05:30"
          }
        }
      ]
    }
  ],
  "decoder_cases": [
    {
      "id": "dc-fixture-001",
      "note": "valid single call",
      "tools": ["create_calendar_event"],
      "chunks": ["<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>"],
      "expected": {
        "calls": [
          {
            "name": "create_calendar_event",
            "arguments": {
              "title": "Design review",
              "start": "2026-10-05T15:00:00+05:30"
            }
          }
        ]
      }
    }
  ]
}"#;
