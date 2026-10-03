//! Compact tool schemas eval.
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! `EVAL_SET` defaults to `/tmp/compact-tools-eval.json` and `OUT` to
//! `/tmp/out.jsonl` when unset. Reads cases from the eval JSON and writes one
//! JSONL line of outputs per case. It does not compute scores; the scorer does that.
//!
//! The harness is intentionally total: missing env vars, missing fields, or an
//! unexpected eval shape degrade to defaults/empty output rather than panicking,
//! so the run always exits 0 on a readable eval file.
//!
//! Optional live mode: set `PROVIDER_BASE_URL` and `MODEL` (OpenAI-compatible
//! `/chat/completions`) to also send each compact request to a real model and
//! decode its raw output. Without them the run is fully offline and deterministic.
//!
//! Token numbers (o200k_base, pinned `tiktoken-rs`) are printed to stderr for
//! inspection; they are not part of the JSONL output.
use std::io::Write;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde_json::{Value, json};

/// Fixed reference time used in the system prompt so relative dates resolve
/// deterministically (2026-10-02, Asia/Kolkata).
const REFERENCE_TIME: &str = "Reference time: 2026-10-02T00:00:00+05:30 (Asia/Kolkata). \
Resolve relative dates like \"Monday\" or \"tomorrow\" against this time.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".into());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".into());
    let raw = std::fs::read_to_string(&path)?;
    let data: Value = serde_json::from_str(&raw)?;

    // Tool catalog: accept the public array shape, or an object map (defensive).
    let tool_entries: Vec<Value> = match data.get("tools") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(m)) => m.values().cloned().collect(),
        _ => Vec::new(),
    };
    let mut tool_defs = std::collections::BTreeMap::<String, Value>::new();
    for t in &tool_entries {
        if let Some(name) = t
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
        {
            tool_defs.insert(name.to_string(), t.clone());
        }
    }
    let to_tool_def = |name: &str| -> Option<ToolDef> {
        let full = tool_defs.get(name)?;
        let function = full.get("function")?;
        let resolved = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        let parameters = function.get("parameters").cloned();
        Some(ToolDef::new(resolved, description, parameters))
    };
    let case_tools = |case: &Value| -> Vec<ToolDef> {
        case.get("tools")
            .and_then(Value::as_array)
            .map(|ts| {
                ts.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|n| to_tool_def(n))
                    .collect()
            })
            .unwrap_or_default()
    };

    let model = std::env::var("MODEL")
        .or_else(|_| {
            std::env::var("MODELS").map(|m| {
                m.split(',')
                    .next()
                    .unwrap_or("compact-tools-eval")
                    .trim()
                    .to_string()
            })
        })
        .unwrap_or_else(|_| "compact-tools-eval".to_string());
    let live_base = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());

    // Pinned tokenizer for the reduction measurement (dev-only). Degrades to
    // skipping the measurement if the tables ever fail to load.
    let bpe = tiktoken_rs::o200k_base().ok();
    let tokens = |v: &Value| -> u64 {
        bpe.as_ref()
            .map(|b| {
                b.encode_ordinary(&serde_json::to_string(v).unwrap_or_default())
                    .len() as u64
            })
            .unwrap_or(0)
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path)?);
    let mut total_compact_tokens = 0u64;
    let mut total_baseline_tokens = 0u64;

    // Lazily-created runtime: only needed for live mode.
    let mut rt: Option<tokio::runtime::Runtime> = None;

    // ── ordinary cases ──────────────────────────────────────────────────
    let cases: Vec<Value> = data
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for case in &cases {
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let tools = case_tools(case);

        let (compacted, instructions) = match encode_tools(&tools) {
            Ok(c) => (c.compacted.iter().all(|x| *x), c.instructions),
            Err(_) => (false, String::new()),
        };

        let messages: Vec<Value> = case
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let system_content = if compacted {
            format!("{REFERENCE_TIME}\n\n{instructions}")
        } else {
            REFERENCE_TIME.to_string()
        };
        let mut req_messages = vec![json!({"role": "system", "content": system_content})];
        req_messages.extend(messages);

        let mut compact_request = json!({"model": model, "temperature": 0, "messages": req_messages});
        if !compacted {
            // Bypass: fall back to the native tool definitions.
            let native: Vec<&Value> = case
                .get("tools")
                .and_then(Value::as_array)
                .map(|ts| {
                    ts.iter()
                        .filter_map(Value::as_str)
                        .filter_map(|n| tool_defs.get(n))
                        .collect()
                })
                .unwrap_or_default();
            compact_request["tools"] = json!(native);
        }

        // Expected calls rendered in our grammar, then decoded back.
        let rendered_calls = case
            .get("expected")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .map(|c| {
                        format!(
                            "<<call {} {}>>",
                            c.get("name").and_then(Value::as_str).unwrap_or("?"),
                            serde_json::to_string(c.get("arguments").unwrap_or(&Value::Null))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let roundtrip_calls = match decode_calls(&rendered_calls, &tools) {
            Ok(calls) => json!(calls
                .iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                .collect::<Vec<_>>()),
            Err(e) => json!({"error": e.code()}),
        };

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });

        // Token reduction vs the native baseline request (measured, not scored here).
        let baseline_tools: Vec<&Value> = case
            .get("tools")
            .and_then(Value::as_array)
            .map(|ts| {
                ts.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|n| tool_defs.get(n))
                    .collect()
            })
            .unwrap_or_default();
        let baseline_request = json!({
            "model": model,
            "temperature": 0,
            "messages": req_messages,
            "tools": baseline_tools,
        });
        let ct = tokens(&compact_request);
        let bt = tokens(&baseline_request);
        total_compact_tokens += ct;
        total_baseline_tokens += bt;
        if bt > 0 {
            eprintln!("[compact-tools-eval] case {id}: compact={ct} baseline={bt} tokens");
        }

        // ── live mode (best-effort; provider errors become output, not panics) ──
        if let Some(base) = &live_base {
            let url = format!("{}/chat/completions", base.trim_end_matches('/'));
            let live_result: Value = (|| {
                if rt.is_none() {
                    rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .ok();
                }
                let rt_ref = rt.as_ref()?;
                let http = reqwest::Client::new();
                rt_ref.block_on(async {
                    let resp = http.post(&url).json(&compact_request).send().await.ok()?;
                    let body: Value = resp.json().await.ok()?;
                    let raw_output = message_text(&body);
                    let live_calls = match raw_output.as_ref().map(|t| decode_calls(t, &tools)) {
                        Some(Ok(calls)) => json!({"calls": calls.iter()
                            .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                            .collect::<Vec<_>>()}),
                        Some(Err(e)) => json!({"error": e.code()}),
                        None => json!({"error": "provider_error"}),
                    };
                    Some(json!({
                        "raw_output": raw_output.map(Value::String).unwrap_or(Value::Null),
                        "live_calls": live_calls,
                    }))
                })
            })()
            .unwrap_or_else(|| json!({"error": "live_mode_unavailable"}));
            if let Some(obj) = live_result.as_object() {
                for (k, v) in obj {
                    line[k] = v.clone();
                }
            }
        }

        writeln!(out, "{line}")?;
    }

    // ── decoder cases ───────────────────────────────────────────────────
    let decoder_cases: Vec<Value> = data
        .get("decoder_cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for case in &decoder_cases {
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let tools = case_tools(case);
        let mut decoder = StreamDecoder::new(&tools);
        if let Some(chunks) = case.get("chunks").and_then(Value::as_array) {
            for chunk in chunks {
                if let Some(s) = chunk.as_str() {
                    decoder.push(s);
                }
            }
        }
        let decoded = match decoder.finish() {
            Ok(calls) => json!({"calls": calls.iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                .collect::<Vec<_>>()}),
            Err(e) => json!({"error": e.code()}),
        };
        let line = json!({"id": id, "decoded": decoded});
        writeln!(out, "{line}")?;
    }

    out.flush()?;

    if total_baseline_tokens > 0 {
        let reduction = 100.0 * (total_baseline_tokens - total_compact_tokens) as f64
            / total_baseline_tokens as f64;
        eprintln!(
            "[compact-tools-eval] TOTAL: compact={total_compact_tokens} baseline={total_baseline_tokens} reduction={reduction:.1}% (o200k_base)"
        );
    }
    Ok(())
}

/// Best-effort assistant text from an OpenAI-compatible chat completion body.
fn message_text(body: &Value) -> Option<String> {
    let content = body
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .clone();
    match content {
        Value::String(s) => Some(s),
        Value::Array(parts) => {
            let joined: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect();
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}
