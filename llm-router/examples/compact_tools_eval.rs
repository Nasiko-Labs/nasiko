//! Evaluation harness for the compact tool-schema format.
//!
//! Offline by default: no network, no API key, deterministic.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Env vars:
//! * `EVAL_SET` (required) - path to the evaluation JSON.
//! * `OUT` (optional, default `compact_tools_out.jsonl`) - JSONL output path.
//! * `PROVIDER_BASE_URL` + `MODEL` (optional) - enable live mode: each
//!   `compact_request` is sent to that OpenAI-compatible endpoint at
//!   temperature 0 and the line gains `raw_output` and `live_calls`.

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_call,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;

/// Fixed reference time so relative dates resolve identically for everyone.
const REFERENCE_TIME: &str =
    "Today is 2026-10-02. The user's timezone is Asia/Kolkata (UTC+05:30). \
Resolve relative dates and times against that.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET")
        .map_err(|_| "EVAL_SET must point at the evaluation JSON file")?;
    let out_path = env::var("OUT").unwrap_or_else(|_| "compact_tools_out.jsonl".to_string());

    let raw = fs::read_to_string(&eval_path)?;
    let data: Value = serde_json::from_str(&raw)?;

    let catalog = build_catalog(&data)?;
    let mut lines: Vec<String> = Vec::new();

    for case in data.get("cases").and_then(Value::as_array).unwrap_or(&vec![]) {
        lines.push(serde_json::to_string(&run_case(case, &catalog))?);
    }
    for case in data.get("decoder_cases").and_then(Value::as_array).unwrap_or(&vec![]) {
        lines.push(serde_json::to_string(&run_decoder_case(case, &catalog))?);
    }

    let mut f = fs::File::create(&out_path)?;
    for line in &lines {
        writeln!(f, "{line}")?;
    }
    eprintln!("wrote {} lines to {out_path}", lines.len());
    Ok(())
}

/// Tool definitions from the eval file, keyed by name. `BTreeMap` keeps
/// iteration deterministic.
fn build_catalog(data: &Value) -> Result<BTreeMap<String, ToolDef>, Box<dyn std::error::Error>> {
    let mut map = BTreeMap::new();
    for t in data.get("tools").and_then(Value::as_array).ok_or("eval file has no `tools`")? {
        let f = t.get("function").unwrap_or(t);
        let name = f.get("name").and_then(Value::as_str).ok_or("tool without a name")?;
        map.insert(
            name.to_string(),
            ToolDef::new(
                name,
                f.get("description").and_then(Value::as_str).map(str::to_string),
                f.get("parameters").cloned(),
            ),
        );
    }
    Ok(map)
}

fn tools_for(case: &Value, catalog: &BTreeMap<String, ToolDef>) -> Vec<ToolDef> {
    case.get("tools")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|n| catalog.get(n).cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// Rebuild the native OpenAI tool entry, used when compaction is bypassed.
fn native_tool(def: &ToolDef) -> Value {
    let mut f = Map::new();
    f.insert("name".into(), Value::String(def.name.clone()));
    if let Some(d) = &def.description {
        f.insert("description".into(), Value::String(d.clone()));
    }
    if let Some(p) = &def.parameters {
        f.insert("parameters".into(), p.clone());
    }
    json!({"type": "function", "function": Value::Object(f)})
}

fn run_case(case: &Value, catalog: &BTreeMap<String, ToolDef>) -> Value {
    let id = case.get("id").cloned().unwrap_or(Value::Null);
    let tools = tools_for(case, catalog);
    let messages = case.get("messages").cloned().unwrap_or_else(|| json!([]));

    let (request, compacted) = match encode_tools(&tools) {
        Ok(ct) => {
            let system = format!("{REFERENCE_TIME}\n\n{}", ct.render());
            let mut msgs = vec![json!({"role": "system", "content": system})];
            if let Some(arr) = messages.as_array() {
                msgs.extend(arr.iter().cloned());
            }
            let mut body = Map::new();
            if let Ok(model) = env::var("MODEL") {
                body.insert("model".into(), Value::String(model));
            }
            body.insert("messages".into(), Value::Array(msgs));
            body.insert("temperature".into(), json!(0));
            (Value::Object(body), true)
        }
        Err(_) => {
            // Unsupported schema feature: send native tool definitions unchanged.
            let mut msgs = vec![json!({"role": "system", "content": REFERENCE_TIME})];
            if let Some(arr) = messages.as_array() {
                msgs.extend(arr.iter().cloned());
            }
            let mut body = Map::new();
            if let Ok(model) = env::var("MODEL") {
                body.insert("model".into(), Value::String(model));
            }
            body.insert("messages".into(), Value::Array(msgs));
            body.insert(
                "tools".into(),
                Value::Array(tools.iter().map(native_tool).collect()),
            );
            body.insert("temperature".into(), json!(0));
            (Value::Object(body), false)
        }
    };

    // Expected calls, written in our grammar, then decoded back.
    let expected = case.get("expected").and_then(Value::as_array).cloned().unwrap_or_default();
    let rendered: String = expected
        .iter()
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?;
            let args = c.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Some(render_call(name, &args))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let roundtrip = match decode_calls(&rendered, &tools) {
        Ok(calls) => calls_to_json(&calls),
        Err(e) => json!([{"error": e.code()}]),
    };

    let mut line = Map::new();
    line.insert("id".into(), id);
    line.insert("compact_request".into(), request.clone());
    line.insert("compacted".into(), Value::Bool(compacted));
    line.insert("rendered_calls".into(), Value::String(rendered));
    line.insert("roundtrip_calls".into(), roundtrip);

    if let (Ok(base), Ok(_model)) = (env::var("PROVIDER_BASE_URL"), env::var("MODEL")) {
        let (raw_output, live) = run_live(&base, &request, &tools);
        line.insert("raw_output".into(), raw_output);
        line.insert("live_calls".into(), live);
    }

    Value::Object(line)
}

fn run_decoder_case(case: &Value, catalog: &BTreeMap<String, ToolDef>) -> Value {
    let id = case.get("id").cloned().unwrap_or(Value::Null);
    let tools = tools_for(case, catalog);
    let mut decoder = StreamDecoder::new(tools);

    let mut failure: Option<CompactError> = None;
    if let Some(chunks) = case.get("chunks").and_then(Value::as_array) {
        for chunk in chunks.iter().filter_map(Value::as_str) {
            if let Err(e) = decoder.push(chunk) {
                failure = Some(e);
                break;
            }
        }
    }

    let decoded = match failure {
        Some(e) => json!({"error": e.code()}),
        None => {
            let (calls, _text) = decoder.finish();
            json!({"calls": calls_to_json(&calls)})
        }
    };

    json!({"id": id, "decoded": decoded})
}

fn calls_to_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args: Value =
                    serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
                json!({"name": c.name, "arguments": args})
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Live mode. Only reached when PROVIDER_BASE_URL and MODEL are both set.
// ---------------------------------------------------------------------------

fn run_live(base: &str, request: &Value, tools: &[ToolDef]) -> (Value, Value) {
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let mut req = ureq::post(&url);
    if let Ok(key) = env::var("PROVIDER_API_KEY") {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    match req.send_json(request) {
        Ok(mut resp) => {
            let body: Value = match resp.body_mut().read_json() {
                Ok(v) => v,
                Err(e) => return (Value::Null, json!({"error": e.to_string()})),
            };
            let text = body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
            let decoded = match decode_calls(&text, tools) {
                Ok(calls) => calls_to_json(&calls),
                Err(e) => json!({"error": e.code()}),
            };
            (Value::String(text), decoded)
        }
        Err(e) => (Value::Null, json!({"error": e.to_string()})),
    }
}
