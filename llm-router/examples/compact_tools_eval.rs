//! Compact tool schemas eval.
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case to
//! `OUT`. It does not compute scores; the scorer does that.
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

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async_main());
}

async fn async_main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    let tool_defs: std::collections::BTreeMap<String, Value> = data["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| {
            (
                t["function"]["name"].as_str().expect("tool name").to_string(),
                t.clone(),
            )
        })
        .collect();
    let to_tool_def = |name: &str| -> ToolDef {
        let full = tool_defs
            .get(name)
            .unwrap_or_else(|| panic!("tool {name} not in eval tools"));
        ToolDef::new(
            full["function"]["name"]
                .as_str()
                .unwrap_or(name)
                .to_string(),
            full["function"]["description"].as_str().map(str::to_string),
            full["function"].get("parameters").cloned(),
        )
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
    let http = reqwest::Client::new();

    // Pinned tokenizer for the reduction measurement (dev-only).
    let bpe = tiktoken_rs::o200k_base().expect("o200k_base tokenizer");
    let tokens = |v: &Value| {
        bpe.encode_ordinary(&serde_json::to_string(v).unwrap())
            .len() as u64
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut total_compact_tokens = 0u64;
    let mut total_baseline_tokens = 0u64;

    // ── ordinary cases ──────────────────────────────────────────────────
    for case in data["cases"].as_array().expect("cases array") {
        let id = case["id"].as_str().expect("id");
        let tools: Vec<ToolDef> = case["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| to_tool_def(t.as_str().expect("tool name")))
            .collect();

        let compact = encode_tools(&tools).expect("encode tools");
        let compacted = compact.compacted.iter().all(|c| *c);

        let messages: Vec<Value> = case["messages"].as_array().expect("messages").to_vec();
        let system_content = if compacted {
            format!("{REFERENCE_TIME}\n\n{}", compact.instructions)
        } else {
            REFERENCE_TIME.to_string()
        };
        let mut req_messages = vec![json!({"role": "system", "content": system_content})];
        req_messages.extend(messages);

        let mut compact_request =
            json!({"model": model, "temperature": 0, "messages": req_messages});
        if !compacted {
            // Bypass: fall back to the native tool definitions.
            let native: Vec<&Value> = case["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| &tool_defs[t.as_str().unwrap()])
                .collect();
            compact_request["tools"] = json!(native);
        }

        // Expected calls rendered in our grammar, then decoded back.
        let rendered_calls = case["expected"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .map(|c| {
                        format!(
                            "<<call {} {}>>",
                            c["name"].as_str().unwrap_or("?"),
                            serde_json::to_string(&c["arguments"]).unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let roundtrip_calls = match decode_calls(&rendered_calls, &tools) {
            Ok(calls) => json!(
                calls
                    .iter()
                    .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                    .collect::<Vec<_>>()
            ),
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
        let baseline_request = json!({
            "model": model,
            "temperature": 0,
            "messages": req_messages,
            "tools": case["tools"].as_array().unwrap().iter()
                .map(|t| &tool_defs[t.as_str().unwrap()]).collect::<Vec<_>>(),
        });
        let ct = tokens(&compact_request);
        let bt = tokens(&baseline_request);
        total_compact_tokens += ct;
        total_baseline_tokens += bt;
        eprintln!("[compact-tools-eval] case {id}: compact={ct} baseline={bt} tokens");

        // ── live mode ──
        if let Some(base) = &live_base {
            let url = format!("{}/chat/completions", base.trim_end_matches('/'));
            let resp = http
                .post(&url)
                .json(&compact_request)
                .send()
                .await
                .expect("live provider request");
            let body: Value = resp.json().await.expect("live provider JSON");
            let raw_output = message_text(&body);
            let live_calls = match raw_output.as_ref().map(|t| decode_calls(t, &tools)) {
                Some(Ok(calls)) => json!({"calls": calls.iter()
                    .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                    .collect::<Vec<_>>()}),
                Some(Err(e)) => json!({"error": e.code()}),
                None => json!({"error": "provider_error"}),
            };
            line["raw_output"] = raw_output.map(Value::String).unwrap_or(Value::Null);
            line["live_calls"] = live_calls;
        }

        writeln!(out, "{line}").expect("write OUT");
    }

    // ── decoder cases ───────────────────────────────────────────────────
    if let Some(decoder_cases) = data.get("decoder_cases").and_then(Value::as_array) {
        for case in decoder_cases {
            let id = case["id"].as_str().expect("id");
            let tools: Vec<ToolDef> = case["tools"]
                .as_array()
                .expect("tools array")
                .iter()
                .map(|t| to_tool_def(t.as_str().expect("tool name")))
                .collect();
            let mut decoder = StreamDecoder::new(&tools);
            for chunk in case["chunks"].as_array().expect("chunks") {
                decoder.push(chunk.as_str().expect("chunk str"));
            }
            let decoded = match decoder.finish() {
                Ok(calls) => json!({"calls": calls.iter()
                    .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                    .collect::<Vec<_>>()}),
                Err(e) => json!({"error": e.code()}),
            };
            let line = json!({"id": id, "decoded": decoded});
            writeln!(out, "{line}").expect("write OUT");
        }
    }

    out.flush().expect("flush OUT");

    if total_baseline_tokens > 0 {
        let reduction = 100.0 * (total_baseline_tokens - total_compact_tokens) as f64
            / total_baseline_tokens as f64;
        eprintln!(
            "[compact-tools-eval] TOTAL: compact={total_compact_tokens} baseline={total_baseline_tokens} reduction={reduction:.1}% (o200k_base)"
        );
    }
}

/// Best-effort assistant text from an OpenAI-compatible chat completion body.
fn message_text(body: &Value) -> Option<String> {
    let content = body["choices"][0]["message"]["content"].clone();
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
