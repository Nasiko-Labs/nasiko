//! Compact tool calling eval (P1).
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Offline by default: no network, no keys, deterministic (`OUT` is byte-identical across
//! runs). Writes one JSONL line per case to `OUT`; it does not compute scores. Token figures
//! (`o200k_base`) go to stderr for local measurement only.
//!
//! Per `cases[]` entry:
//!   id, compact_request (full OpenAI-shaped body; the native body when bypassed),
//!   compacted, bypass_reason (when bypassed), rendered_calls (expected calls in the compact
//!   format), roundtrip_calls (rendered_calls decoded back, or {error, message}),
//!   decoded_tools (`decode_tools` output — the schemas rebuilt from the compact text).
//! Per `decoder_cases[]` entry:
//!   id, decoded ({calls} or {error, message}) from `StreamDecoder` fed chunk by chunk.
//!
//! Live mode (format adherence): set `PROVIDER_BASE_URL` (OpenAI-compatible, e.g.
//! `https://host/v1`) and `MODEL`; optionally `PROVIDER_API_KEY` (sent as a bearer token —
//! never commit one). Each `compact_request` is sent at temperature 0 and the line gains
//! `raw_output` and `live_calls` (decoded calls, or {error, message}). A bypassed case is sent
//! with native tools, and `live_calls` is then the provider's native `tool_calls`.
//! `LIVE_NATIVE=1` additionally sends every case with native tools on the same model and adds
//! `native_raw_output` / `native_live_calls`, as a format-adherence baseline.
use std::collections::HashMap;
use std::io::Write;

use nasiko_llm_router::compact_tools;
use nasiko_llm_router::ir::ChatRequest;
use nasiko_tool_compact as tc;
use serde_json::{Value, json};

/// Calls as the scorer reads them: `[{name, arguments: <object>}]`.
fn calls_json(calls: &[tc::ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "arguments": c.arguments_value().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

fn error_json(e: &tc::Error) -> Value {
    json!({"error": e.code(), "message": e.to_string()})
}

fn decode_result(r: tc::Result<Vec<tc::ToolCall>>) -> Value {
    match r {
        Ok(calls) => json!({"calls": calls_json(&calls)}),
        Err(e) => error_json(&e),
    }
}

/// Tools named by a case, looked up in the eval's shared pool.
fn select_tools(pool: &HashMap<String, Value>, names: &Value) -> Vec<Value> {
    names
        .as_array()
        .map(|ns| {
            ns.iter()
                .map(|n| {
                    let n = n.as_str().expect("tool names are strings");
                    pool.get(n)
                        .unwrap_or_else(|| panic!("case names unknown tool '{n}'"))
                        .clone()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn crate_tools(tools: &[Value]) -> Vec<tc::ToolDef> {
    tools
        .iter()
        .map(|t| serde_json::from_value(t.clone()).expect("tool definition"))
        .collect()
}

struct Live {
    url: String,
    model: String,
    key: Option<String>,
    client: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

impl Live {
    fn from_env() -> Option<Self> {
        let base = std::env::var("PROVIDER_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty())?;
        let base = base.trim_end_matches('/');
        let url = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        };
        Some(Self {
            url,
            model,
            key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("http client"),
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
        })
    }

    /// Send one request; returns the assistant message.
    fn send(&self, body: &Value) -> Result<Value, String> {
        let mut body = body.clone();
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        self.rt.block_on(async {
            let mut req = self.client.post(&self.url).json(&body);
            if let Some(k) = &self.key {
                req = req.bearer_auth(k);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            let v: Value = resp.json().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {v}"));
            }
            Ok(v["choices"][0]["message"].clone())
        })
    }
}

/// `raw_output` and `live_calls` for one case.
fn live_fields(live: &Live, body: &Value, compacted: bool, tools: &[tc::ToolDef]) -> Value {
    let message = match live.send(body) {
        Ok(m) => m,
        Err(e) => return json!({"live_error": e}),
    };
    let raw = message["content"].as_str().unwrap_or_default().to_string();
    let live_calls = if compacted {
        decode_result(tc::decode_calls(&raw, tools))
    } else {
        // Native tool calling: report the provider's own calls in the same shape.
        let calls: Vec<tc::ToolCall> = message["tool_calls"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| tc::ToolCall {
                        name: c["function"]["name"].as_str().unwrap_or_default().into(),
                        arguments: c["function"]["arguments"].as_str().unwrap_or("{}").into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({"calls": calls_json(&calls), "native": true})
    };
    json!({"raw_output": raw, "live_calls": live_calls})
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    let pool: HashMap<String, Value> = data["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| {
            let name = t["function"]["name"]
                .as_str()
                .expect("tool name")
                .to_string();
            (name, t.clone())
        })
        .collect();
    let live = Live::from_env();
    let native_baseline = matches!(std::env::var("LIVE_NATIVE").as_deref(), Ok("1" | "true"));
    let bpe = tiktoken_rs::o200k_base().expect("o200k_base");
    let tokens = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let (mut base_total, mut compact_total) = (0usize, 0usize);
    let mut stats = Vec::new();

    for case in data["cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().expect("id");
        let tools = select_tools(&pool, &case["tools"]);
        let ctools = crate_tools(&tools);

        // Native baseline: the case's messages and tools (plus tool_choice if the case sets one).
        let mut native = json!({"messages": case["messages"], "tools": tools});
        if !case["tool_choice"].is_null() {
            native["tool_choice"] = case["tool_choice"].clone();
        }
        let req: ChatRequest = serde_json::from_value(native.clone()).expect("chat request");

        let mut line = json!({"id": id});
        let (body, compacted) = match compact_tools::compact_request(&req) {
            Ok(c) => {
                let body = serde_json::to_value(&c.request).expect("serialize request");
                let rebuilt = tc::encode_tools(&ctools)
                    .and_then(|ct| tc::decode_tools(&ct))
                    .map(|ts| serde_json::to_value(ts).expect("serialize tools"))
                    .unwrap_or_else(|e| error_json(&e));
                line["decoded_tools"] = rebuilt;
                (body, true)
            }
            Err(bypass) => {
                line["bypass_reason"] = json!(bypass.as_label());
                (native.clone(), false)
            }
        };

        let expected: Vec<tc::ToolCall> = case["expected"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .map(|c| tc::ToolCall {
                        name: c["name"].as_str().expect("expected call name").into(),
                        arguments: c["arguments"].to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rendered = tc::render_calls(&expected).expect("expected calls render");
        let roundtrip = match tc::decode_calls(&rendered, &ctools) {
            Ok(calls) => calls_json(&calls),
            Err(e) => error_json(&e),
        };

        line["compact_request"] = body.clone();
        line["compacted"] = json!(compacted);
        line["rendered_calls"] = json!(rendered);
        line["roundtrip_calls"] = roundtrip;

        if let Some(live) = &live {
            let fields = live_fields(live, &body, compacted, &ctools);
            for (k, v) in fields.as_object().into_iter().flatten() {
                line[k] = v.clone();
            }
            // Same case with native tools on the same model, for an adherence baseline.
            if native_baseline {
                let fields = live_fields(live, &native, false, &ctools);
                for (k, v) in fields.as_object().into_iter().flatten() {
                    line[format!("native_{k}")] = v.clone();
                }
            }
        }

        let (b, c) = (tokens(&native), tokens(&body));
        base_total += b;
        compact_total += c;
        stats.push(format!(
            "{id}: baseline {b} → compact {c} tokens{}",
            if compacted { "" } else { " (bypassed)" }
        ));
        writeln!(out, "{line}").expect("write OUT");
    }

    for case in data["decoder_cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().expect("id");
        let ctools = crate_tools(&select_tools(&pool, &case["tools"]));
        let decoded = (|| {
            let mut d = tc::StreamDecoder::new(&ctools)?;
            let mut calls = Vec::new();
            let mut take = |events: Vec<tc::StreamEvent>| {
                for e in events {
                    if let tc::StreamEvent::Call { call, .. } = e {
                        calls.push(call);
                    }
                }
            };
            for chunk in case["chunks"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                take(d.push(chunk.as_str().unwrap_or_default())?);
            }
            take(d.finish()?);
            Ok(calls)
        })();
        writeln!(
            out,
            "{}",
            json!({"id": id, "decoded": decode_result(decoded)})
        )
        .expect("write OUT");
    }
    out.flush().expect("flush OUT");

    for s in &stats {
        eprintln!("{s}");
    }
    if base_total > 0 {
        let saved = 1.0 - compact_total as f64 / base_total as f64;
        eprintln!(
            "total: baseline {base_total} → compact {compact_total} tokens (o200k_base), reduction {:.1}%",
            saved * 100.0
        );
    }
}
