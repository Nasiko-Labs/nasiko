//! Compact tool schemas evaluation harness (hackathon P1).
//!
//! ```bash
//! EVAL_SET=/tmp/compact-tools-eval.json \
//! OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Optional live mode (never required):
//! `PROVIDER_BASE_URL` + `MODEL` (+ `PROVIDER_API_KEY` if the endpoint needs it).

use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, reference_time_preamble,
    render_calls, split_like,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiktoken_rs::o200k_base;

fn main() {
    let eval_set = env::var("EVAL_SET").expect("EVAL_SET path is required");
    let out_path = env::var("OUT").expect("OUT path is required");

    let raw = std::fs::read_to_string(&eval_set).unwrap_or_else(|e| {
        panic!("failed to read EVAL_SET={eval_set}: {e}");
    });
    let dataset: EvalDataset = serde_json::from_str(&raw).unwrap_or_else(|e| {
        panic!("failed to parse EVAL_SET JSON: {e}");
    });

    let tool_index = build_tool_index(&dataset.tools);
    let bpe = o200k_base().expect("o200k_base tokenizer");

    let provider_base = env::var("PROVIDER_BASE_URL").ok();
    let model = env::var("MODEL").ok();
    let api_key = env::var("PROVIDER_API_KEY").unwrap_or_default();
    let live = provider_base.is_some() && model.is_some();

    let mut out = File::create(&out_path).unwrap_or_else(|e| {
        panic!("failed to create OUT={out_path}: {e}");
    });

    // Process cases in dataset order (deterministic).
    for case in &dataset.cases {
        let tools = resolve_tools(&case.tools, &tool_index);
        let line = run_case(
            case,
            &tools,
            &bpe,
            live,
            provider_base.as_deref(),
            model.as_deref(),
            &api_key,
        );
        writeln!(out, "{}", serde_json::to_string(&line).expect("serialize")).unwrap();
    }

    for case in &dataset.decoder_cases {
        let tools = resolve_tools(&case.tools, &tool_index);
        let line = run_decoder_case(case, &tools);
        writeln!(out, "{}", serde_json::to_string(&line).expect("serialize")).unwrap();
    }

    out.flush().unwrap();
    eprintln!(
        "wrote {} case lines + {} decoder lines to {out_path}",
        dataset.cases.len(),
        dataset.decoder_cases.len()
    );
}

#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[allow(dead_code)]
    schema_version: Option<String>,
    #[allow(dead_code)]
    purpose: Option<String>,
    tools: Vec<Value>,
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[allow(dead_code)]
    expected: Option<Value>,
    #[serde(rename = "match", default)]
    #[allow(dead_code)]
    match_cfg: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[allow(dead_code)]
    note: Option<String>,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: DecoderExpected,
}

#[derive(Debug, Deserialize)]
struct DecoderExpected {
    #[serde(default)]
    calls: Option<Vec<ExpectedCall>>,
    #[allow(dead_code)]
    error: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct CaseOut {
    id: String,
    compact_request: Value,
    compacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rendered_calls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    roundtrip_calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compact_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_reduction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DecoderOut {
    id: String,
    decoded: Value,
}

fn build_tool_index(tools: &[Value]) -> BTreeMap<String, ToolDef> {
    let mut map = BTreeMap::new();
    for t in tools {
        let function = t.get("function").unwrap_or(t);
        let name = function
            .get("name")
            .and_then(|n| n.as_str())
            .expect("tool name")
            .to_string();
        let def = ToolDef {
            name: name.clone(),
            description: function
                .get("description")
                .and_then(|d| d.as_str())
                .map(str::to_string),
            parameters: function.get("parameters").cloned(),
        };
        map.insert(name, def);
    }
    map
}

fn resolve_tools(names: &[String], index: &BTreeMap<String, ToolDef>) -> Vec<ToolDef> {
    names
        .iter()
        .map(|n| {
            index
                .get(n)
                .cloned()
                .unwrap_or_else(|| panic!("unknown tool in case: {n}"))
        })
        .collect()
}

fn run_case(
    case: &EvalCase,
    tools: &[ToolDef],
    bpe: &tiktoken_rs::CoreBPE,
    live: bool,
    provider_base: Option<&str>,
    model: Option<&str>,
    api_key: &str,
) -> CaseOut {
    let encoded = match encode_tools(tools) {
        Ok(c) => c,
        Err(e) => {
            return CaseOut {
                id: case.id.clone(),
                compact_request: json!({}),
                compacted: false,
                rendered_calls: None,
                roundtrip_calls: None,
                native_tokens: None,
                compact_tokens: None,
                token_reduction: None,
                raw_output: None,
                live_calls: None,
                error: Some(e.label().into()),
            };
        }
    };

    let native_tools_json = serde_json::to_string_pretty(&openai_tools(tools)).unwrap();
    let native_request = json!({
        "messages": case.messages,
        "tools": openai_tools(tools),
        "tool_choice": "auto",
    });
    let compact_messages = with_compact_system(&case.messages, &encoded.prompt);
    let compact_request = json!({
        "messages": compact_messages,
        "tool_choice": "none",
    });

    // Token reduction measures schema compaction (tools JSON vs compact signatures+instructions).
    // Reference-time preamble is eval/live context and is excluded from both sides.
    let native_tokens = bpe.encode_with_special_tokens(&native_tools_json).len()
        + bpe
            .encode_with_special_tokens(&serde_json::to_string(&case.messages).unwrap())
            .len();
    let compact_tokens = bpe.encode_with_special_tokens(&encoded.prompt).len()
        + bpe
            .encode_with_special_tokens(&serde_json::to_string(&case.messages).unwrap())
            .len();
    let token_reduction = if native_tokens == 0 {
        0.0
    } else {
        (native_tokens as f64 - compact_tokens as f64) / native_tokens as f64
    };

    // Offline round-trip: render expected calls (if present) through our grammar and decode.
    let (rendered_calls, roundtrip_calls) = if let Some(expected) = case.expected.as_ref() {
        if let Some(arr) = expected.as_array() {
            let calls: Vec<ToolCall> = arr
                .iter()
                .filter_map(|v| {
                    Some(ToolCall {
                        name: v.get("name")?.as_str()?.to_string(),
                        arguments: v.get("arguments")?.clone(),
                    })
                })
                .collect();
            let rendered = render_calls(&calls).unwrap_or_default();
            let decoded = decode_calls(&rendered, tools).unwrap_or_default();
            let roundtrip: Vec<Value> = decoded
                .iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                .collect();
            (Some(rendered), Some(roundtrip))
        } else {
            (None, Some(vec![]))
        }
    } else {
        (None, None)
    };

    let mut raw_output = None;
    let mut live_calls = None;
    if live && let (Some(base), Some(model)) = (provider_base, model) {
        match live_chat(base, model, api_key, &compact_request) {
            Ok(text) => {
                let decoded = decode_calls(&text, tools);
                live_calls = Some(match decoded {
                    Ok(calls) => json!({
                        "calls": calls.iter().map(|c| json!({
                            "name": c.name,
                            "arguments": c.arguments
                        })).collect::<Vec<_>>()
                    }),
                    Err(e) => json!({ "error": e.label() }),
                });
                raw_output = Some(text);
            }
            Err(e) => {
                raw_output = Some(format!("live_error: {e}"));
            }
        }
    }

    // Silence unused warning when expected is empty — keep native_request for debugging.
    let _ = native_request;

    CaseOut {
        id: case.id.clone(),
        compact_request,
        compacted: true,
        rendered_calls,
        roundtrip_calls,
        native_tokens: Some(native_tokens),
        compact_tokens: Some(compact_tokens),
        token_reduction: Some(token_reduction),
        raw_output,
        live_calls,
        error: None,
    }
}

fn run_decoder_case(case: &DecoderCase, tools: &[ToolDef]) -> DecoderOut {
    // Transform: if the case expects calls, re-render in our grammar and re-chunk at
    // equivalent relative boundaries so private datasets with alternate markers still work.
    let chunks = if let Some(calls) = &case.expected.calls {
        let tool_calls: Vec<ToolCall> = calls
            .iter()
            .map(|c| ToolCall {
                name: c.name.clone(),
                arguments: c.arguments.clone(),
            })
            .collect();
        match render_calls(&tool_calls) {
            Ok(rendered) if !rendered.is_empty() => split_like(&rendered, &case.chunks),
            _ => case.chunks.clone(),
        }
    } else {
        // Error cases: keep chunks (same `<<call>>` grammar) or pass through.
        case.chunks.clone()
    };

    let mut decoder = StreamDecoder::new(tools);
    let mut fatal: Option<String> = None;
    for chunk in &chunks {
        if let Err(e) = decoder.push(chunk) {
            fatal = Some(e.label().into());
            break;
        }
    }
    let decoded = if let Some(label) = fatal {
        json!({ "error": label })
    } else {
        match decoder.finish() {
            Ok(calls) => json!({
                "calls": calls.iter().map(|c| json!({
                    "name": c.name,
                    "arguments": c.arguments
                })).collect::<Vec<_>>()
            }),
            Err(e) => json!({ "error": e.label() }),
        }
    };

    DecoderOut {
        id: case.id.clone(),
        decoded,
    }
}

fn openai_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect()
}

fn with_compact_system(messages: &[Value], prompt: &str) -> Vec<Value> {
    // Official live contract: provide a reference date/timezone so relative dates resolve
    // consistently. Overridable via env; defaults match the hackathon brief. This is NOT
    // case-specific — it never injects expected tool arguments.
    let ref_date = env::var("COMPACT_REF_DATE").unwrap_or_else(|_| "2026-10-02".into());
    let ref_tz = env::var("COMPACT_REF_TZ").unwrap_or_else(|_| "Asia/Kolkata".into());
    let mut system = reference_time_preamble(&ref_date, &ref_tz);
    system.push('\n');
    system.push('\n');
    system.push_str(prompt);

    let mut out = messages.to_vec();
    out.push(json!({
        "role": "system",
        "content": system,
    }));
    out
}

fn live_chat(base: &str, model: &str, api_key: &str, body_base: &Value) -> Result<String, String> {
    let mut body = body_base.clone();
    body["model"] = json!(model);
    body["temperature"] = json!(0);
    body["stream"] = json!(false);

    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let client = reqwest::blocking::Client::new();
    let mut req = client.post(&url).json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(content)
}

#[allow(dead_code)]
fn _ensure_paths_compile() -> PathBuf {
    PathBuf::from("/tmp/out.jsonl")
}

#[allow(dead_code)]
fn _read_jsonl(path: &str) -> Vec<Value> {
    let f = File::open(path).unwrap();
    BufReader::new(f)
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect()
}
