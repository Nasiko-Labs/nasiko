//! Compact tool schemas eval — offline by default; live when `PROVIDER_BASE_URL` + `MODEL` set.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Env vars:
//! - `EVAL_SET` (required) — path to compact-tools-eval JSON
//! - `OUT` (required) — JSONL output path
//! - `PROVIDER_BASE_URL` + `MODEL` (optional) — live OpenAI-compatible chat at temperature 0
//! - `PROVIDER_API_KEY` (optional) — Bearer token for live mode
//!
//! Grammar matches the public sample's `<<call ...>>` markers, so decoder_cases chunks are
//! fed as-is (no per-case hand conversion). If the grammar ever diverges, convert by
//! re-rendering each case's expected call with [`nasiko_tool_compact::render_calls`] and
//! splitting at the same relative offsets as the published chunks.
//!
//! Reference clock for live runs (injected as system text): today=`2026-10-02`, tz=`Asia/Kolkata`.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_calls,
    tool_call,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const REF_SYSTEM: &str =
    "Today is 2026-10-02. Timezone Asia/Kolkata. Resolve relative dates from this reference.";

fn main() -> ExitCode {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };
    rt.block_on(async_main())
}

async fn async_main() -> ExitCode {
    let eval_set = match env::var("EVAL_SET") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            eprintln!("EVAL_SET is required");
            return ExitCode::from(2);
        }
    };
    let out_path = match env::var("OUT") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            eprintln!("OUT is required");
            return ExitCode::from(2);
        }
    };

    let raw = match fs::read_to_string(&eval_set) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read EVAL_SET: {e}");
            return ExitCode::from(1);
        }
    };
    let set: EvalSet = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to parse EVAL_SET: {e}");
            return ExitCode::from(1);
        }
    };

    let tool_index = index_tools(&set.tools);
    let live = live_config();

    let mut out = match fs::File::create(&out_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("failed to create OUT: {e}");
            return ExitCode::from(1);
        }
    };

    let mut token_baseline = 0usize;
    let mut token_compact = 0usize;
    let bpe = tiktoken_rs::o200k_base().ok();

    for case in &set.cases {
        let line = run_case(case, &tool_index, live.as_ref()).await;
        if let Some(bpe) = &bpe {
            let baseline = baseline_request(case, &tool_index);
            let base_s = serde_json::to_string(&baseline).unwrap_or_default();
            let compact_s =
                serde_json::to_string(&line.get("compact_request").unwrap_or(&Value::Null))
                    .unwrap_or_default();
            let bt = bpe.encode_with_special_tokens(&base_s).len();
            let ct = if line.get("compacted").and_then(Value::as_bool) == Some(true) {
                bpe.encode_with_special_tokens(&compact_s).len()
            } else {
                bt
            };
            token_baseline += bt;
            token_compact += ct;
        }
        writeln!(out, "{}", line).expect("write OUT");
    }

    for dc in &set.decoder_cases {
        let line = run_decoder_case(dc, &tool_index);
        writeln!(out, "{}", line).expect("write OUT");
    }

    if token_baseline > 0 {
        let saving = 1.0 - (token_compact as f64 / token_baseline as f64);
        eprintln!(
            "local token estimate (o200k_base, claims only): baseline={token_baseline} \
             compact={token_compact} reduction={:.1}%",
            saving * 100.0
        );
    }
    eprintln!(
        "wrote {} lines to {}",
        set.cases.len() + set.decoder_cases.len(),
        out_path.display()
    );
    ExitCode::SUCCESS
}

#[derive(Debug, Deserialize)]
struct EvalSet {
    tools: Vec<Value>,
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
    #[serde(default, rename = "match")]
    match_spec: Option<Value>,
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
    expected: Value,
}

#[derive(Clone)]
struct LiveConfig {
    base_url: String,
    model: String,
    api_key: String,
}

fn live_config() -> Option<LiveConfig> {
    let base_url = env::var("PROVIDER_BASE_URL").ok().filter(|s| !s.is_empty())?;
    let model = env::var("MODEL").ok().filter(|s| !s.is_empty())?;
    let api_key = env::var("PROVIDER_API_KEY").unwrap_or_default();
    Some(LiveConfig {
        base_url,
        model,
        api_key,
    })
}

fn index_tools(tools: &[Value]) -> HashMap<String, ToolDef> {
    let mut map = HashMap::new();
    for t in tools {
        let name = t
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let description = t
            .pointer("/function/description")
            .and_then(Value::as_str)
            .map(str::to_string);
        let parameters = t.pointer("/function/parameters").cloned();
        map.insert(
            name.clone(),
            ToolDef {
                name,
                description,
                parameters,
            },
        );
    }
    map
}

fn resolve_tools(names: &[String], index: &HashMap<String, ToolDef>) -> Vec<ToolDef> {
    names
        .iter()
        .filter_map(|n| index.get(n).cloned())
        .collect()
}

async fn run_case(
    case: &Case,
    index: &HashMap<String, ToolDef>,
    live: Option<&LiveConfig>,
) -> Value {
    let tools = resolve_tools(&case.tools, index);
    let encoded = encode_tools(&tools);
    let (compacted, compact_request, tools_for_decode) = match encoded {
        Ok(c) => {
            let req = compact_chat_request(&case.messages, &c.prompt);
            (true, req, tools.clone())
        }
        Err(_) => {
            // Bypass: native tools kept (scorer counts 0% savings).
            let req = native_chat_request(&case.messages, &case.tools, index);
            (false, req, tools)
        }
    };

    let expected_calls: Vec<ToolCall> = case
        .expected
        .iter()
        .filter_map(|e| tool_call(&e.name, &e.arguments).ok())
        .collect();
    let rendered_calls = render_calls(&expected_calls).unwrap_or_default();
    let roundtrip = decode_calls(&rendered_calls, &tools_for_decode);
    let roundtrip_calls = match roundtrip {
        Ok(calls) => calls_to_json(&calls),
        Err(e) => json!({"error": e.as_label()}),
    };

    let mut line = json!({
        "id": case.id,
        "compact_request": compact_request,
        "compacted": compacted,
        "rendered_calls": rendered_calls,
        "roundtrip_calls": roundtrip_calls,
    });

    if let Some(live) = live
        && compacted
    {
        match live_complete(live, &compact_request).await {
            Ok(raw_output) => {
                let live_calls = match decode_calls(&raw_output, &tools_for_decode) {
                    Ok(calls) => json!({"calls": calls_to_json(&calls)}),
                    Err(e) => json!({"error": e.as_label()}),
                };
                if let Some(obj) = line.as_object_mut() {
                    obj.insert("raw_output".into(), json!(raw_output));
                    obj.insert("live_calls".into(), live_calls);
                }
            }
            Err(err) => {
                if let Some(obj) = line.as_object_mut() {
                    obj.insert("live_error".into(), json!(err));
                }
            }
        }
    }

    let _ = &case.match_spec; // reserved for local checks; scorer owns matching
    line
}

fn run_decoder_case(dc: &DecoderCase, index: &HashMap<String, ToolDef>) -> Value {
    let tools = resolve_tools(&dc.tools, index);
    // Same grammar as the public sample — feed chunks directly.
    // (If grammar diverged: render expected calls, re-split at relative offsets.)
    let mut dec = StreamDecoder::new(tools);
    let mut push_err: Option<CompactError> = None;
    for chunk in &dc.chunks {
        if let Err(e) = dec.push(chunk) {
            push_err = Some(e);
            break;
        }
    }
    let decoded = if let Some(e) = push_err {
        json!({"error": e.as_label()})
    } else {
        match dec.finish() {
            Ok(calls) => json!({"calls": calls_to_json(&calls)}),
            Err(e) => json!({"error": e.as_label()}),
        }
    };
    let _ = &dc.expected; // scorer compares; we only report outputs
    json!({
        "id": dc.id,
        "decoded": decoded,
    })
}

fn compact_chat_request(messages: &[Value], compact_prompt: &str) -> Value {
    let mut msgs = vec![json!({
        "role": "system",
        "content": format!("{REF_SYSTEM}\n\n{compact_prompt}")
    })];
    msgs.extend(messages.iter().cloned());
    json!({
        "messages": msgs,
        "temperature": 0.0,
    })
}

fn native_chat_request(
    messages: &[Value],
    tool_names: &[String],
    index: &HashMap<String, ToolDef>,
) -> Value {
    let tools: Vec<Value> = tool_names
        .iter()
        .filter_map(|n| {
            let t = index.get(n)?;
            Some(json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            }))
        })
        .collect();
    let mut msgs = vec![json!({
        "role": "system",
        "content": REF_SYSTEM
    })];
    msgs.extend(messages.iter().cloned());
    json!({
        "messages": msgs,
        "tools": tools,
        "temperature": 0.0,
    })
}

fn baseline_request(case: &Case, index: &HashMap<String, ToolDef>) -> Value {
    native_chat_request(&case.messages, &case.tools, index)
}

fn calls_to_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args: Value =
                    serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                json!({
                    "name": c.name,
                    "arguments": args,
                })
            })
            .collect(),
    )
}

async fn live_complete(live: &LiveConfig, compact_request: &Value) -> Result<String, String> {
    let mut body = compact_request.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), json!(live.model));
        obj.insert("temperature".into(), json!(0.0));
    }
    let url = format!(
        "{}/chat/completions",
        live.base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(&url).json(&body);
    if !live.api_key.is_empty() {
        req = req.bearer_auth(&live.api_key);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(content)
}

#[allow(dead_code)]
fn _serde_markers(_: impl Serialize) {}
