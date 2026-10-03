//! Compact tool schemas eval — writes one JSONL line of outputs per case; computes no scores.
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! | env                 | default                          | meaning                                  |
//! |---------------------|----------------------------------|------------------------------------------|
//! | `EVAL_SET`          | `/tmp/compact-tools-eval.json`   | dataset (`compact-tools-eval-v1`)        |
//! | `OUT`               | `/tmp/out.jsonl`                 | output JSONL                             |
//! | `PROVIDER_BASE_URL` | unset                            | with `MODEL`: live mode (OpenAI-compat.) |
//! | `MODEL`             | unset                            | model id sent in live mode               |
//! | `PROVIDER_API_KEY`  | unset                            | optional bearer token for live mode      |
//! | `LIVE_BASELINE`     | unset                            | `1`: also send the native-tools request  |
//!
//! Offline by default: no network, deterministic output. Requests are built through
//! [`nasiko_llm_router::compact_tools::compact`], the same function the router calls, and
//! decoder cases run through [`StreamDecoder`] chunk by chunk. The grammar is the brief's
//! `<<call name {json}>>`, so decoder-case chunks are fed verbatim with no conversion.
//!
//! `compact_request` carries no `model` or `temperature`; live mode adds `model` and
//! `temperature: 0` to it (and to the native request) at send time.
//!
//! Token counts (`o200k_base`) go to stderr as a local cross-check only.

use std::io::Write;
use std::time::Duration;

use nasiko_llm_router::compact_tools::{self, Bypass};
use nasiko_llm_router::ir::ChatRequest;
use nasiko_tool_compact::{self as tc, StreamDecoder};
use serde_json::{Value, json};

/// Fixed reference time from the brief, so relative dates resolve identically for everyone.
const REFERENCE_TIME: &str = "Today is Fri 2026-10-02, timezone Asia/Kolkata (+05:30).";

fn main() {
    if let Err(e) = run() {
        eprintln!("compact_tools_eval: {e}");
        std::process::exit(1);
    }
}

struct Live {
    http: reqwest::Client,
    base: String,
    model: String,
    key: Option<String>,
    baseline: bool,
}

fn run() -> Result<(), String> {
    let eval_set = env_or("EVAL_SET", "/tmp/compact-tools-eval.json");
    let out_path = env_or("OUT", "/tmp/out.jsonl");
    let raw = std::fs::read_to_string(&eval_set).map_err(|e| format!("read {eval_set}: {e}"))?;
    let set: Value = serde_json::from_str(&raw).map_err(|e| format!("parse {eval_set}: {e}"))?;

    let catalog: Vec<Value> = set["tools"].as_array().cloned().unwrap_or_default();
    let by_name = |names: &Value| -> Vec<Value> {
        let names: Vec<&str> = names
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        names
            .iter()
            .filter_map(|n| {
                catalog
                    .iter()
                    .find(|t| t["function"]["name"].as_str() == Some(n))
                    .cloned()
            })
            .collect()
    };

    let live = match (std::env::var("PROVIDER_BASE_URL"), std::env::var("MODEL")) {
        (Ok(base), Ok(model)) if !base.is_empty() && !model.is_empty() => Some(Live {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_owned(),
            model,
            key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
            baseline: std::env::var("LIVE_BASELINE").as_deref() == Ok("1"),
        }),
        _ => None,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let bpe = tiktoken_rs::o200k_base().map_err(|e| e.to_string())?;
    let tokens = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();
    let (mut tok_native, mut tok_native_ref, mut tok_compact) = (0usize, 0usize, 0usize);
    let (mut n_compacted, mut n_cases) = (0usize, 0usize);

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path).map_err(|e| format!("create {out_path}: {e}"))?,
    );

    for case in set["cases"].as_array().into_iter().flatten() {
        n_cases += 1;
        let id = case["id"].clone();
        let native_tools = by_name(&case["tools"]);
        let user_messages = case["messages"].as_array().cloned().unwrap_or_default();

        // Native baseline as the scorer builds it, and the same with our reference-time line.
        let baseline = json!({"messages": user_messages, "tools": native_tools});
        let mut messages = vec![json!({"role": "system", "content": REFERENCE_TIME})];
        messages.extend(user_messages);
        let native = json!({"messages": messages, "tools": native_tools});

        let mut req: ChatRequest =
            serde_json::from_value(native.clone()).map_err(|e| format!("{id}: {e}"))?;
        let compacted = compact_tools::compact(&mut req);
        if compacted.is_ok() {
            // The router leaves author messages untouched, so the reference-time line is a second
            // system message after the injected one. Both are ours here, so fold it into the
            // first and save a message envelope; providers that hoist system text join them anyway.
            let reference = req.messages.remove(1);
            if let (Some(Value::String(tools)), Some(Value::String(r))) =
                (&mut req.messages[0].content, reference.content)
            {
                *tools = format!("{r}\n{tools}");
            }
        }
        let compact_request = serde_json::to_value(&req).map_err(|e| e.to_string())?;

        tok_native += tokens(&baseline);
        tok_native_ref += tokens(&native);
        tok_compact += tokens(&compact_request);

        let tool_defs: Vec<tc::ToolDef> = req_tools(&native_tools);
        let expected: Vec<tc::ToolCall> = case["expected"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| tc::ToolCall {
                name: c["name"].as_str().unwrap_or_default().to_owned(),
                arguments: c["arguments"].to_string(),
            })
            .collect();
        let rendered = tc::render_calls(&expected);
        let roundtrip = calls_json(tc::decode_calls(&rendered, &tool_defs));

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted.is_ok(),
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });
        match &compacted {
            Ok(_) => n_compacted += 1,
            Err(Bypass::Unsupported(reason)) => line["bypass_reason"] = json!(reason),
            Err(other) => line["bypass_reason"] = json!(other.as_label()),
        }

        if let Some(live) = &live {
            let (raw_output, live_calls) =
                rt.block_on(live_call(live, &compact_request, &tool_defs));
            line["raw_output"] = raw_output;
            line["live_calls"] = live_calls;
            if live.baseline {
                let (_, native_calls) = rt.block_on(live_call(live, &native, &tool_defs));
                line["native_calls"] = native_calls;
            }
        }
        writeln!(out, "{line}").map_err(|e| e.to_string())?;
    }

    let mut n_decoder = 0;
    for case in set["decoder_cases"].as_array().into_iter().flatten() {
        n_decoder += 1;
        let tool_defs = req_tools(&by_name(&case["tools"]));
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let decoded = stream_decode(&chunks, &tool_defs);
        writeln!(
            out,
            "{}",
            json!({"id": case["id"], "decoded": calls_json(decoded)})
        )
        .map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())?;

    let pct = |a: usize, b: usize| 100.0 * (1.0 - a as f64 / b.max(1) as f64);
    eprintln!(
        "compact_tools_eval: {n_cases} cases ({n_compacted} compacted), {n_decoder} decoder cases → {out_path}"
    );
    eprintln!(
        "  o200k tokens: compact {tok_compact} vs native {tok_native} ({:.1}% saved); \
         vs native+reference-time {tok_native_ref} ({:.1}% saved)",
        pct(tok_compact, tok_native),
        pct(tok_compact, tok_native_ref),
    );
    Ok(())
}

/// Convert native tool JSON through the router's own IR and seam conversion.
fn req_tools(native: &[Value]) -> Vec<tc::ToolDef> {
    native
        .iter()
        .filter_map(|t| serde_json::from_value(t.clone()).ok())
        .filter_map(|t| compact_tools::to_compact_def(&t).ok())
        .collect()
}

fn stream_decode(chunks: &[&str], tools: &[tc::ToolDef]) -> tc::Result<Vec<tc::ToolCall>> {
    let mut d = StreamDecoder::new(tools)?;
    for c in chunks {
        d.push(c)?;
    }
    d.finish().map(|d| d.calls)
}

fn calls_json(r: tc::Result<Vec<tc::ToolCall>>) -> Value {
    match r {
        Ok(calls) => json!({
            "calls": calls
                .iter()
                .map(|c| json!({"name": c.name, "arguments": c.arguments_value()}))
                .collect::<Vec<_>>()
        }),
        Err(e) => json!({"error": e.kind(), "detail": e.to_string()}),
    }
}

/// Send one request at temperature 0. Returns `(raw_output, calls)`; native `tool_calls` in the
/// reply (bypassed or baseline requests) are reported as calls too, so both arms compare.
async fn live_call(live: &Live, body: &Value, tools: &[tc::ToolDef]) -> (Value, Value) {
    let mut body = body.clone();
    body["model"] = json!(live.model);
    body["temperature"] = json!(0);
    let mut rq = live
        .http
        .post(format!("{}/chat/completions", live.base))
        .json(&body);
    if let Some(k) = &live.key {
        rq = rq.bearer_auth(k);
    }
    let resp: Value = match rq.send().await {
        Ok(r) => {
            let status = r.status();
            match r.json::<Value>().await {
                Ok(v) if status.is_success() => v,
                Ok(v) => return (Value::Null, json!({"error": "http", "detail": v})),
                Err(e) => {
                    return (
                        Value::Null,
                        json!({"error": "http", "detail": e.to_string()}),
                    );
                }
            }
        }
        Err(e) => {
            return (
                Value::Null,
                json!({"error": "http", "detail": e.to_string()}),
            );
        }
    };
    let msg = &resp["choices"][0]["message"];
    let text = msg["content"].as_str().unwrap_or_default().to_owned();
    if let Some(native) = msg["tool_calls"].as_array().filter(|a| !a.is_empty()) {
        let calls: Vec<Value> = native
            .iter()
            .map(|c| {
                let args = c["function"]["arguments"].as_str().unwrap_or("null");
                json!({
                    "name": c["function"]["name"],
                    "arguments": serde_json::from_str::<Value>(args).unwrap_or(Value::Null),
                })
            })
            .collect();
        return (json!(text), json!({"calls": calls, "native": true}));
    }
    (json!(text), calls_json(tc::decode_calls(&text, tools)))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}
