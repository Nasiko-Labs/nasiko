//! Offline evaluator for compact tool schemas.
//!
//! Contract (evaluator runs this directly):
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! - Default mode is offline: no network, no API keys, no LLM call.
//!   Deterministic: two runs diff clean.
//! - Reads `EVAL_SET` (one JSON file with `tools`, `cases`, `decoder_cases`);
//!   writes one JSONL line per case to `OUT`.
//! - `cases` (`ct-*`) lines:
//!   `{"id","compact_request","compacted","rendered_calls","roundtrip_calls"}`
//!   where `compact_request` is the OpenAI-shaped body that would be sent
//!   (`messages` with the injected compact block, native `tools` instead when
//!   bypassed; `model`/`temperature` are transport boilerplate added at send
//!   time, not counted here, so both modes stay comparable), `rendered_calls`
//!   is the case's expected calls written in this crate's `<<call …>>`
//!   grammar (joined by `\n`; `""` when none or when bypassed), and
//!   `roundtrip_calls` is that text decoded back (`[{name, arguments}]`).
//! - `decoder_cases` (`dc-*`) lines: `{"id","decoded":{"calls":[…]}}` or
//!   `{"id","decoded":{"error":"unknown_tool"|"invalid_arguments"}}`. Chunks
//!   are fed through `StreamDecoder` chunk by chunk, proving split markers
//!   reconstruct.
//! - Live mode (format adherence): when `PROVIDER_BASE_URL` and `MODEL` are
//!   both set, each `compact_request` is POSTed to
//!   `{PROVIDER_BASE_URL}/chat/completions` with `model` and `temperature: 0`
//!   injected at send time, and the line additionally carries `raw_output`
//!   (model text) and `live_calls` (decoded calls) or `live_error`. Auth:
//!   `Authorization: Bearer $PROVIDER_API_KEY` (or `$OPENAI_API_KEY`) when
//!   set, else no header (the scorer's proxy holds the keys).
//!
//! Exit code 0 means the evaluation ran; the scorer recomputes all metrics.

use std::path::PathBuf;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools, render_call};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct EvalSet {
    #[allow(dead_code)]
    schema_version: Option<String>,
    #[allow(dead_code)]
    purpose: Option<String>,
    tools: Vec<NativeTool>,
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Clone, Deserialize)]
struct NativeTool {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    function: NativeFunction,
}

#[derive(Debug, Clone, Deserialize)]
struct NativeFunction {
    name: String,
    description: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<CaseMessage>,
    expected: Vec<ExpectedCall>,
    #[allow(dead_code)]
    r#match: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct CaseMessage {
    role: String,
    content: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[allow(dead_code)]
    note: Option<String>,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: Value,
}

#[derive(Debug, Clone, Serialize)]
struct RoundtripCall {
    name: String,
    arguments: Value,
}

fn native_to_compact(t: &NativeTool) -> ToolDef {
    ToolDef {
        name: t.function.name.clone(),
        description: t.function.description.clone(),
        parameters: t.function.parameters.clone(),
    }
}

fn native_to_json(t: &NativeTool) -> Value {
    json!({
        "type": t.kind.clone().unwrap_or_else(|| "function".to_string()),
        "function": {
            "name": t.function.name,
            "description": t.function.description,
            "parameters": t.function.parameters,
        }
    })
}

fn lookup<'a>(all: &'a [NativeTool], name: &str) -> Option<&'a NativeTool> {
    all.iter().find(|t| t.function.name == name)
}

fn error_kind(e: &nasiko_tool_compact::CompactError) -> &'static str {
    match e {
        nasiko_tool_compact::CompactError::UnknownTool(_) => "unknown_tool",
        // Malformed markers are validation failures too: never a guessed
        // call, always surfaced as invalid arguments for scoring.
        nasiko_tool_compact::CompactError::InvalidArguments { .. }
        | nasiko_tool_compact::CompactError::Malformed(_)
        | nasiko_tool_compact::CompactError::InvalidToolDef(_)
        | nasiko_tool_compact::CompactError::UnsupportedSchema { .. } => "invalid_arguments",
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set = std::env::var("EVAL_SET").unwrap_or_else(|_| {
        eprintln!("EVAL_SET not set; expected path to the eval JSON file");
        std::process::exit(1);
    });
    let out = std::env::var("OUT").unwrap_or_else(|_| {
        eprintln!("OUT not set; expected output JSONL path");
        std::process::exit(1);
    });
    let live_base = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let live_model = std::env::var("MODEL").ok().filter(|s| !s.is_empty());
    let live = live_base.is_some() && live_model.is_some();
    let live_key = std::env::var("PROVIDER_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok()
        .filter(|s| !s.is_empty());

    let raw = std::fs::read_to_string(&eval_set)?;
    let eval: EvalSet = serde_json::from_str(&raw)?;

    let out_path = PathBuf::from(&out);
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut lines: Vec<String> = Vec::new();

    let client = if live {
        Some(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
        )
    } else {
        None
    };

    // --- ct-* cases: compact request + render + round-trip ---
    for case in &eval.cases {
        let mut native_defs: Vec<&NativeTool> = Vec::new();
        let mut compact_defs: Vec<ToolDef> = Vec::new();
        for name in &case.tools {
            if let Some(t) = lookup(&eval.tools, name) {
                native_defs.push(t);
                compact_defs.push(native_to_compact(t));
            }
        }
        let encoded = encode_tools(&compact_defs).unwrap_or(nasiko_tool_compact::CompactTools {
            text: String::new(),
            compacted: false,
            bypass_reason: Some("encode_error".to_string()),
        });

        let compact_request = if encoded.compacted && !encoded.text.is_empty() {
            let mut messages: Vec<Value> = vec![json!({"role": "system", "content": encoded.text})];
            for m in &case.messages {
                messages.push(json!({"role": m.role, "content": m.content}));
            }
            json!({"messages": messages})
        } else {
            let native_tools: Vec<Value> = native_defs.iter().map(|t| native_to_json(t)).collect();
            let messages: Vec<Value> = case
                .messages
                .iter()
                .map(|m| json!({"role": m.role, "content": m.content}))
                .collect();
            json!({"messages": messages, "tools": native_tools})
        };

        // Render the expected calls in this grammar (never by hand per case:
        // mechanical rendering). When compaction was bypassed the schema is
        // outside the grammar, so there is no faithful rendering: emit empty
        // and let the scorer count the case as 0% savings.
        let rendered_calls = if encoded.compacted {
            case.expected
                .iter()
                .map(|e| render_call(&e.name, &e.arguments))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };

        // Decode the rendered text back (empty → plain answer → []).
        let roundtrip_calls: Vec<RoundtripCall> = if rendered_calls.trim().is_empty() {
            Vec::new()
        } else {
            match decode_calls(&rendered_calls, &compact_defs) {
                Ok(calls) => calls
                    .into_iter()
                    .map(|c| RoundtripCall {
                        name: c.name,
                        arguments: c.arguments,
                    })
                    .collect(),
                Err(_) => Vec::new(),
            }
        };

        let mut line = serde_json::Map::new();
        line.insert("id".to_string(), json!(case.id));
        line.insert("compact_request".to_string(), compact_request.clone());
        line.insert("compacted".to_string(), json!(encoded.compacted));
        line.insert("rendered_calls".to_string(), json!(rendered_calls));
        line.insert(
            "roundtrip_calls".to_string(),
            serde_json::to_value(&roundtrip_calls)?,
        );
        if !encoded.compacted {
            line.insert(
                "bypass_reason".to_string(),
                json!(encoded.bypass_reason.clone()),
            );
        }

        // Live mode: POST the compact request, decode the model's text.
        if live {
            let (raw_output, live_calls, live_error) = live_once(
                client.as_ref().unwrap(),
                live_base.as_ref().unwrap(),
                live_model.as_ref().unwrap(),
                live_key.as_deref(),
                &compact_request,
                &compact_defs,
            )
            .await;
            line.insert("raw_output".to_string(), json!(raw_output));
            match live_error {
                None => {
                    line.insert("live_calls".to_string(), json!(live_calls));
                }
                Some(err) => {
                    line.insert("live_calls".to_string(), json!(live_calls));
                    line.insert("live_error".to_string(), json!(err));
                }
            }
        }

        lines.push(serde_json::to_string(&Value::Object(line))?);
    }

    // --- dc-* cases: chunk-by-chunk stream decoding ---
    for dc in &eval.decoder_cases {
        let compact_defs: Vec<ToolDef> = dc
            .tools
            .iter()
            .filter_map(|name| lookup(&eval.tools, name))
            .map(native_to_compact)
            .collect();
        let mut dec = StreamDecoder::new();
        for chunk in &dc.chunks {
            dec.push(chunk);
        }
        let mut line = serde_json::Map::new();
        line.insert("id".to_string(), json!(dc.id));
        match dec.finish(&compact_defs) {
            Ok(calls) => {
                let out_calls: Vec<Value> = calls
                    .into_iter()
                    .map(|c| json!({"name": c.name, "arguments": c.arguments}))
                    .collect();
                line.insert("decoded".to_string(), json!({"calls": out_calls}));
            }
            Err(e) => {
                line.insert("decoded".to_string(), json!({"error": error_kind(&e)}));
            }
        }
        // Echo the case's expectation shape for debuggability (never used for
        // scoring; the scorer recomputes from `decoded`).
        let _ = &dc.expected;
        lines.push(serde_json::to_string(&Value::Object(line))?);
    }

    std::fs::write(&out_path, lines.join("\n") + "\n")?;
    eprintln!(
        "compact_tools_eval: wrote {} lines to {}",
        lines.len(),
        out_path.display()
    );
    Ok(())
}

async fn live_once(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    api_key: Option<&str>,
    compact_request: &Value,
    compact_defs: &[ToolDef],
) -> (String, Vec<RoundtripCall>, Option<String>) {
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    // Transport boilerplate lives here, not in the recorded `compact_request`,
    // so token comparisons stay on the differential part.
    let mut body = compact_request.clone();
    if let Value::Object(ref mut map) = body {
        map.insert("model".to_string(), Value::String(model.to_string()));
        map.insert("temperature".to_string(), json!(0));
    }
    let mut req = client.post(&url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let text = match req.send().await {
        Ok(resp) => match resp.text().await {
            Ok(t) => t,
            Err(e) => return (String::new(), Vec::new(), Some(format!("read_error:{e}"))),
        },
        Err(e) => {
            return (
                String::new(),
                Vec::new(),
                Some(format!("request_error:{e}")),
            );
        }
    };
    // The endpoint returns an OpenAI chat completion; extract the assistant text.
    let raw_output = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("choices")?
                .get(0)?
                .get("message")?
                .get("content")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| text.clone());
    match decode_calls(&raw_output, compact_defs) {
        Ok(calls) => (
            raw_output,
            calls
                .into_iter()
                .map(|c| RoundtripCall {
                    name: c.name,
                    arguments: c.arguments,
                })
                .collect(),
            None,
        ),
        Err(e) => (raw_output, Vec::new(), Some(error_kind(&e).to_string())),
    }
}
