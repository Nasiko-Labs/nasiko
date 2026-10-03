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
//!
//! Live mode (format adherence): set `PROVIDER_BASE_URL` (OpenAI-compatible, e.g.
//! `https://api.openai.com/v1`) and `MODEL`; `PROVIDER_API_KEY` is sent as a bearer token
//! if set (never commit it). Each `compact_request` is sent at temperature 0 and the line
//! gains `raw_output` (the model's text) and `live_calls` (decoded calls or error; for a
//! bypassed case, the native `tool_calls` validated the same way).
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

/// Live-mode settings, present only when `PROVIDER_BASE_URL` and `MODEL` are set.
struct Live {
    url: String,
    model: String,
    key: Option<String>,
    client: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

impl Live {
    fn from_env() -> Option<Self> {
        let base = std::env::var("PROVIDER_BASE_URL").ok()?;
        let model = std::env::var("MODEL").ok()?;
        Some(Self {
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            key: std::env::var("PROVIDER_API_KEY").ok(),
            client: reqwest::Client::new(),
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
        })
    }

    /// Send one request; returns (`raw_output`, `live_calls`).
    fn run(&self, request: &Value, tools: &[ToolDef]) -> (Value, Value) {
        let mut body = request.clone();
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        let mut req = self.client.post(&self.url).json(&body);
        if let Some(key) = &self.key {
            req = req.bearer_auth(key);
        }
        let resp: Result<Value, String> = self.rt.block_on(async {
            let r = req.send().await.map_err(|e| e.to_string())?;
            let status = r.status();
            let v: Value = r.json().await.map_err(|e| e.to_string())?;
            if status.is_success() {
                Ok(v)
            } else {
                Err(format!("HTTP {status}: {v}"))
            }
        });
        let msg = match resp {
            Ok(v) => v["choices"][0]["message"].clone(),
            Err(e) => return (Value::Null, json!({"error": "provider", "detail": e})),
        };
        let text = msg["content"].as_str().unwrap_or_default().to_string();
        let native = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let calls = if native.is_empty() {
            decode_calls(&text, tools)
        } else {
            native_calls(&native, tools)
        };
        (json!(text), outcome(calls))
    }
}

/// Validate native `tool_calls` (bypassed cases) with the same rules as compact calls.
fn native_calls(
    native: &[Value],
    tools: &[ToolDef],
) -> Result<Vec<ToolCall>, nasiko_tool_compact::Error> {
    let text: String = native
        .iter()
        .map(|c| {
            let f = &c["function"];
            let args = f["arguments"].as_str().unwrap_or("null");
            format!("<<call {} {args}>>", f["name"].as_str().unwrap_or_default())
        })
        .collect();
    decode_calls(&text, tools)
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
    let live = Live::from_env();

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
        let mut line = json!({
            "id": id,
            "compact_request": request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });
        if let Some(live) = &live {
            let (raw_output, live_calls) = live.run(&line["compact_request"], &tools);
            line["raw_output"] = raw_output;
            line["live_calls"] = live_calls;
        }
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
