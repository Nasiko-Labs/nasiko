//! `compact_tools_eval` — the `[compact-tools]` (compact tool schemas) evaluation harness.
//!
//! Contract (see the track brief):
//!   * `EVAL_SET` — path to the eval JSON (`{tools, cases, decoder_cases}`). Required.
//!   * `OUT`      — path to write one JSONL line per case/decoder-case. Required.
//!   * Offline + deterministic by default: no network, no keys, no model call. Running it
//!     twice over the same `EVAL_SET` produces byte-identical `OUT`.
//!   * Live mode (format adherence): when `PROVIDER_BASE_URL` and `MODEL` are set, each
//!     `compact_request` is sent to that OpenAI-compatible endpoint at temperature 0, and the
//!     model's text (`raw_output`) plus the decoded result (`live_calls`) are added to the line.
//!     `PROVIDER_API_KEY` is used as a bearer token if present.
//!
//! The harness reports **outputs, not scores** — the grader recomputes every metric from `OUT`.
//!
//! Run:
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```

use std::collections::HashMap;
use std::env;
use std::fs;

use serde_json::{json, Map, Value};

use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolCall, ToolDef};

/// Fixed reference time from the brief, injected so relative dates resolve identically for
/// everyone in live mode. Harmless (and ignored by the model) in offline mode.
const REFERENCE_TIME_NOTE: &str =
    "Today is 2026-10-02, timezone Asia/Kolkata. Resolve relative dates against this.";

#[tokio::main]
async fn main() {
    let eval_path = env::var("EVAL_SET").expect("EVAL_SET must point to the eval JSON file");
    let out_path = env::var("OUT").expect("OUT must point to the JSONL output path");

    let raw = fs::read_to_string(&eval_path).expect("failed to read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("EVAL_SET is not valid JSON");

    let tool_index = index_tools(data.get("tools"));

    // Demo diagnostics: when EMIT_TOKENS is set, each case line also carries the native
    // baseline request and o200k_base token counts so a UI can show before/after. The grader
    // never sets this, so the scored output stays exactly as the contract specifies.
    let emit_tokens = env::var("EMIT_TOKENS").is_ok();
    let bpe = tiktoken_rs::o200k_base().ok();

    let live = LiveConfig::from_env();
    let mut lines: Vec<String> = Vec::new();

    for case in data.get("cases").and_then(Value::as_array).into_iter().flatten() {
        lines.push(run_case(case, &tool_index, live.as_ref(), bpe.as_ref(), emit_tokens).await);
    }

    for dc in data.get("decoder_cases").and_then(Value::as_array).into_iter().flatten() {
        lines.push(run_decoder_case(dc, &tool_index));
    }

    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&out_path, body).expect("failed to write OUT");
    eprintln!(
        "compact_tools_eval: wrote {} line(s) to {out_path}{}",
        lines.len(),
        if live.is_some() { " (live mode)" } else { " (offline)" }
    );

    report_token_savings(&data, &tool_index, bpe.as_ref());
}

/// Demo-only: local token measurement with `o200k_base` (the grader recomputes its own). For
/// each case, compares the native-tools baseline body against our `compact_request` body and
/// prints the aggregate reduction `1 - sum(compact)/sum(baseline)` to stderr.
fn report_token_savings(data: &Value, index: &HashMap<String, ToolDef>, bpe: Option<&tiktoken_rs::CoreBPE>) {
    let Some(bpe) = bpe else {
        return;
    };
    let count = |s: &str| bpe.encode_with_special_tokens(s).len();

    let mut baseline_total = 0usize;
    let mut compact_total = 0usize;
    for case in data.get("cases").and_then(Value::as_array).into_iter().flatten() {
        let tools = tools_for(case, index);
        let Ok(compact) = encode_tools(&tools) else { continue };
        let compact_request = build_compact_request(case, &compact.render(), None);
        let baseline_request = build_native_request(case, &tools);
        baseline_total += count(&baseline_request.to_string());
        compact_total += count(&compact_request.to_string());
    }
    if baseline_total > 0 {
        let reduction = 1.0 - (compact_total as f64 / baseline_total as f64);
        eprintln!(
            "compact_tools_eval: token reduction {:.1}% (baseline {baseline_total} -> compact {compact_total}, o200k_base)",
            reduction * 100.0
        );
    }
}

/// The native (uncompacted) baseline body: the case messages plus full OpenAI tool schemas.
fn build_native_request(case: &Value, tools: &[ToolDef]) -> Value {
    let native_tools: Vec<Value> = tools
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
        .collect();
    json!({
        "messages": case.get("messages").cloned().unwrap_or_else(|| json!([])),
        "tools": native_tools,
        "temperature": 0,
    })
}

/// Build `name -> ToolDef` from the eval file's `tools` array (OpenAI `{function:{...}}` shape).
fn index_tools(tools: Option<&Value>) -> HashMap<String, ToolDef> {
    let mut index = HashMap::new();
    for t in tools.and_then(Value::as_array).into_iter().flatten() {
        let func = t.get("function").unwrap_or(t);
        let Some(name) = func.get("name").and_then(Value::as_str) else {
            continue;
        };
        index.insert(
            name.to_string(),
            ToolDef {
                name: name.to_string(),
                description: func.get("description").and_then(Value::as_str).map(str::to_string),
                parameters: func.get("parameters").cloned(),
            },
        );
    }
    index
}

/// The tools a case references, in the case's declared order.
fn tools_for(case: &Value, index: &HashMap<String, ToolDef>) -> Vec<ToolDef> {
    case.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str())
        .filter_map(|n| index.get(n).cloned())
        .collect()
}

async fn run_case(
    case: &Value,
    index: &HashMap<String, ToolDef>,
    live: Option<&LiveConfig>,
    bpe: Option<&tiktoken_rs::CoreBPE>,
    emit_tokens: bool,
) -> String {
    let id = case.get("id").and_then(Value::as_str).unwrap_or("");
    let tools = tools_for(case, index);

    // Compaction can always be attempted for these schemas; a real router would set
    // `compacted=false` and fall back when a tool uses an unsupported schema feature.
    let compact = encode_tools(&tools).expect("encode_tools failed on eval tools");

    let compact_request = build_compact_request(case, &compact.render(), live);

    let expected = case.get("expected").and_then(Value::as_array).cloned().unwrap_or_default();
    let rendered_calls = render_calls(&expected);
    let roundtrip = decode_calls(&rendered_calls, &tools).unwrap_or_default();
    let roundtrip_calls = calls_to_json(&roundtrip);

    let mut line = Map::new();
    line.insert("id".to_string(), json!(id));
    line.insert("compact_request".to_string(), compact_request.clone());
    line.insert("compacted".to_string(), json!(true));
    line.insert("rendered_calls".to_string(), json!(rendered_calls));
    line.insert("roundtrip_calls".to_string(), roundtrip_calls);

    // Demo diagnostics (off for the grader): the native baseline body and token counts, so a
    // UI can render the real before/after transformation side by side.
    if emit_tokens {
        if let Some(bpe) = bpe {
            let count = |s: &str| bpe.encode_with_special_tokens(s).len();
            let baseline_request = build_native_request(case, &tools);
            let baseline_tokens = count(&baseline_request.to_string());
            let compact_tokens = count(&compact_request.to_string());
            let saved_pct = if baseline_tokens > 0 {
                100.0 * (1.0 - compact_tokens as f64 / baseline_tokens as f64)
            } else {
                0.0
            };
            line.insert("baseline_request".to_string(), baseline_request);
            line.insert("baseline_tokens".to_string(), json!(baseline_tokens));
            line.insert("compact_tokens".to_string(), json!(compact_tokens));
            line.insert("saved_pct".to_string(), json!((saved_pct * 10.0).round() / 10.0));
        }
    }

    if let Some(live) = live {
        let (raw_output, live_calls) = live.run(&compact_request, &tools).await;
        line.insert("raw_output".to_string(), json!(raw_output));
        line.insert("live_calls".to_string(), live_calls);
    }

    Value::Object(line).to_string()
}

fn run_decoder_case(dc: &Value, index: &HashMap<String, ToolDef>) -> String {
    let id = dc.get("id").and_then(Value::as_str).unwrap_or("");
    let tools = tools_for(dc, index);

    let mut decoder = StreamDecoder::new(&tools);
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut error: Option<&'static str> = None;

    for chunk in dc.get("chunks").and_then(Value::as_array).into_iter().flatten() {
        let Some(chunk) = chunk.as_str() else { continue };
        for result in decoder.push(chunk) {
            match result {
                Ok(call) => calls.push(call),
                Err(e) if error.is_none() => error = Some(e.as_wire()),
                Err(_) => {}
            }
        }
    }

    let decoded = match error {
        Some(wire) => json!({ "error": wire }),
        None => json!({ "calls": calls_to_json(&calls) }),
    };
    json!({ "id": id, "decoded": decoded }).to_string()
}

/// Render expected calls in the `<<call name {json}>>` grammar, one per line.
fn render_calls(expected: &[Value]) -> String {
    expected
        .iter()
        .map(|exp| {
            let name = exp.get("name").and_then(Value::as_str).unwrap_or("");
            let args = exp.get("arguments").cloned().unwrap_or_else(|| json!({}));
            format!("<<call {name} {}>>", serde_json::to_string(&args).unwrap_or_else(|_| "{}".into()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn calls_to_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| json!({ "name": c.name, "arguments": c.arguments }))
            .collect(),
    )
}

/// The full OpenAI-shaped request body we would send instead of native tools: the compact tool
/// definitions and call instructions injected as a leading system message. The fixed
/// reference-time note is added only in live mode (it only affects how a real model resolves
/// relative dates); omitting it offline keeps the token count honest.
fn build_compact_request(case: &Value, compact_system: &str, live: Option<&LiveConfig>) -> Value {
    let system_content = if live.is_some() {
        format!("{REFERENCE_TIME_NOTE}\n\n{compact_system}")
    } else {
        compact_system.to_string()
    };
    let mut messages = vec![json!({ "role": "system", "content": system_content })];
    for m in case.get("messages").and_then(Value::as_array).into_iter().flatten() {
        messages.push(m.clone());
    }

    let mut body = Map::new();
    if let Some(live) = live {
        body.insert("model".to_string(), json!(live.model));
    }
    body.insert("messages".to_string(), Value::Array(messages));
    body.insert("temperature".to_string(), json!(0));
    Value::Object(body)
}

/// Live-mode configuration, present only when both `PROVIDER_BASE_URL` and `MODEL` are set.
struct LiveConfig {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl LiveConfig {
    fn from_env() -> Option<LiveConfig> {
        let base_url = env::var("PROVIDER_BASE_URL").ok()?;
        let model = env::var("MODEL").ok()?;
        Some(LiveConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key: env::var("PROVIDER_API_KEY").ok(),
            client: reqwest::Client::new(),
        })
    }

    /// Send one compact request; return `(raw_output, live_calls)` where `live_calls` is either
    /// the decoded calls array or `{"error": wire}` on a decode failure or transport error.
    async fn run(&self, request: &Value, tools: &[ToolDef]) -> (Value, Value) {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(&url).json(request);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let text = match req.send().await {
            Ok(resp) => match resp.json::<Value>().await {
                Ok(body) => body
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                Err(e) => return (json!(null), json!({ "error": format!("decode_body: {e}") })),
            },
            Err(e) => return (json!(null), json!({ "error": format!("transport: {e}") })),
        };
        let live_calls = match decode_calls(&text, tools) {
            Ok(calls) => calls_to_json(&calls),
            Err(e) => json!({ "error": e.as_wire() }),
        };
        (json!(text), live_calls)
    }
}
