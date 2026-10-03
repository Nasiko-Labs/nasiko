//! Compact tool schemas eval (offline, deterministic).
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Writes one JSONL line per case to `OUT`:
//! * `cases`: `compact_request` (full chat body: reference-date system message with the
//!   compact tool block, then the case messages; or the native body with `tools` when
//!   compaction is bypassed), `compacted`, `rendered_calls` (expected calls in the
//!   `<<call NAME {JSON}>>` grammar), `roundtrip_calls` (that text decoded back).
//! * `decoder_cases`: `decoded`, the `StreamDecoder` result fed chunk by chunk. The chunks
//!   are already in our grammar, so they are used as given (no per-case conversion).
//!
//! No network, no clock, no randomness: two runs give identical `OUT`. Token counts
//! (o200k_base) go to stderr for information only.
use std::io::Write;

use nasiko_tool_compact::{
    StreamDecoder, StreamEvent, ToolCall, ToolDef, decode_calls, encode_tools, render_call,
};
use serde_json::{Value, json};

/// Fixed reference time so relative dates resolve the same way for every run and model.
const REFERENCE: &str = "Today is 2026-10-02 (Friday), timezone Asia/Kolkata.";

fn tool_def(native: &Value) -> ToolDef {
    let f = &native["function"];
    ToolDef {
        name: f["name"].as_str().expect("tool name").to_string(),
        description: f["description"].as_str().map(str::to_string),
        parameters: f.get("parameters").cloned(),
    }
}

fn calls_json(calls: &[ToolCall]) -> Value {
    calls
        .iter()
        .map(|c| json!({"name": c.name, "arguments": c.arguments}))
        .collect()
}

fn outcome(result: Result<Vec<ToolCall>, nasiko_tool_compact::Error>) -> Value {
    match result {
        Ok(calls) => json!({"calls": calls_json(&calls)}),
        Err(e) => json!({"error": e.as_label()}),
    }
}

/// Feed `chunks` to a `StreamDecoder`; any error fails the whole response.
fn stream_decode(chunks: &[Value], tools: &[ToolDef]) -> Value {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();
    let mut events = Vec::new();
    for chunk in chunks {
        match decoder.push(chunk.as_str().expect("chunk string")) {
            Ok(e) => events.extend(e),
            Err(e) => return outcome(Err(e)),
        }
    }
    match decoder.finish() {
        Ok(e) => events.extend(e),
        Err(e) => return outcome(Err(e)),
    }
    for e in events {
        if let StreamEvent::Call(c) = e {
            calls.push(c);
        }
    }
    outcome(Ok(calls))
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let natives = data["tools"].as_array().expect("tools array");
    let pick = |names: &Value| -> Vec<Value> {
        let names: Vec<&str> = names
            .as_array()
            .expect("case tools")
            .iter()
            .map(|n| n.as_str().expect("tool name"))
            .collect();
        names
            .iter()
            .map(|n| {
                natives
                    .iter()
                    .find(|t| t["function"]["name"] == *n)
                    .cloned()
                    .expect("case names a defined tool")
            })
            .collect()
    };
    let bpe = tiktoken_rs::o200k_base().expect("o200k_base");
    let tokens = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();
    let (mut base_total, mut compact_total) = (0usize, 0usize);

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for case in data["cases"].as_array().expect("cases array") {
        let id = case["id"].as_str().expect("id");
        let native = pick(&case["tools"]);
        let tools: Vec<ToolDef> = native.iter().map(tool_def).collect();
        let user = case["messages"].as_array().expect("messages").clone();
        let with_system = |system: String| -> Vec<Value> {
            std::iter::once(json!({"role": "system", "content": system}))
                .chain(user.iter().cloned())
                .collect()
        };
        let baseline = json!({"messages": with_system(REFERENCE.into()), "tools": native});
        // Bypass on unsupported schemas, and when the compact body would not be smaller.
        let compact = encode_tools(&tools).ok().and_then(|ct| {
            let body = json!({"messages": with_system(format!("{REFERENCE}\n\n{}", ct.text))});
            (body.to_string().len() < baseline.to_string().len()).then_some(body)
        });
        let compacted = compact.is_some();
        let request = compact.unwrap_or_else(|| baseline.clone());

        let expected: Vec<ToolCall> = case["expected"]
            .as_array()
            .expect("expected calls")
            .iter()
            .map(|c| ToolCall {
                name: c["name"].as_str().expect("call name").to_string(),
                arguments: c["arguments"].clone(),
            })
            .collect();
        let rendered = expected
            .iter()
            .map(render_call)
            .collect::<Vec<_>>()
            .join("\n");
        // Bypassed requests use native tool calls, which pass through unchanged.
        let roundtrip = if compacted {
            match decode_calls(&rendered, &tools) {
                Ok(calls) => calls_json(&calls),
                Err(e) => json!({"error": e.as_label()}),
            }
        } else {
            calls_json(&expected)
        };

        let (b, c) = (tokens(&baseline), tokens(&request));
        base_total += b;
        compact_total += c;
        eprintln!("{id}: baseline {b} compact {c} compacted {compacted}");
        let line = json!({
            "id": id,
            "compact_request": request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    for case in data["decoder_cases"]
        .as_array()
        .expect("decoder_cases array")
    {
        let tools: Vec<ToolDef> = pick(&case["tools"]).iter().map(tool_def).collect();
        let chunks = case["chunks"].as_array().expect("chunks");
        let line = json!({"id": case["id"], "decoded": stream_decode(chunks, &tools)});
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    if base_total > 0 {
        let saved = 100.0 * (1.0 - compact_total as f64 / base_total as f64);
        eprintln!("tokens: baseline {base_total} compact {compact_total} saved {saved:.1}%");
    }
}
