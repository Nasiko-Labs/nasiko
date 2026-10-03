//! Compact-tool-schema eval: writes one JSONL line per case. Offline and deterministic by default.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Env: `EVAL_SET` (required), `OUT` (default: stdout). Optional live mode, when both
//! `PROVIDER_BASE_URL` and `MODEL` are set: each `compact_request` is sent to
//! `{PROVIDER_BASE_URL}/chat/completions` at temperature 0 (bearer token from
//! `PROVIDER_API_KEY` if set) and the line gains `raw_output` and `live_calls`. Nothing is
//! printed to stdout when `OUT` is set; a local token-reduction estimate goes to stderr.
//!
//! Output lines (outputs only, no scores):
//! - cases:         `{id, compact_request, compacted, rendered_calls, roundtrip_calls}`
//! - decoder cases: `{id, decoded: {calls:[{name,arguments}]} | {error: code}}`
//!
//! Calls are reported as `{name, arguments: <object>}`, the same shape as the set's `expected`.
//! Calls to tools that had to bypass compaction have no compact form: they are omitted from
//! `rendered_calls`/`roundtrip_calls` and listed under `native_calls` instead.

use std::collections::HashMap;
use std::process::ExitCode;

use nasiko_llm_router::ir;
use nasiko_tool_compact as tc;
use serde_json::{Value, json};

/// Fixed reference time from the challenge brief, so relative dates resolve identically.
const CLOCK: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

type Res<T> = Result<T, String>;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("compact_tools_eval: {e}");
            ExitCode::from(2)
        }
    }
}

struct Live {
    base: String,
    model: String,
    key: Option<String>,
    http: reqwest::Client,
}

async fn run() -> Res<()> {
    let path = std::env::var("EVAL_SET")
        .map_err(|_| "EVAL_SET is required (path to the eval JSON)".to_string())?;
    let set: Value =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?)
            .map_err(|e| format!("parse {path}: {e}"))?;

    // Tools go through the router's own IR type, then across the seam into the crate's type.
    let mut defs: HashMap<String, tc::ToolDef> = HashMap::new();
    let mut originals: HashMap<String, Value> = HashMap::new();
    for raw in set["tools"].as_array().ok_or("`tools` missing")? {
        let t: ir::ToolDef =
            serde_json::from_value(raw.clone()).map_err(|e| format!("bad tool: {e}"))?;
        originals.insert(t.function.name.clone(), raw.clone());
        defs.insert(
            t.function.name.clone(),
            tc::ToolDef {
                name: t.function.name,
                description: t.function.description,
                parameters: t.function.parameters,
            },
        );
    }
    let pick = |v: &Value| -> Res<Vec<tc::ToolDef>> {
        v.as_array()
            .ok_or("case `tools` is not an array")?
            .iter()
            .map(|n| {
                let n = n.as_str().ok_or("tool name is not a string")?;
                defs.get(n)
                    .cloned()
                    .ok_or_else(|| format!("case references unknown tool `{n}`"))
            })
            .collect()
    };

    let live = match (std::env::var("PROVIDER_BASE_URL"), std::env::var("MODEL")) {
        (Ok(base), Ok(model)) if !base.is_empty() && !model.is_empty() => Some(Live {
            base: base.trim_end_matches('/').to_string(),
            model,
            key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?,
        }),
        _ => None,
    };

    let bpe = tiktoken_rs::o200k_base().map_err(|e| format!("tokenizer: {e}"))?;
    let count = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();
    let (mut baseline_tokens, mut compact_tokens) = (0usize, 0usize);

    let mut lines: Vec<Value> = Vec::new();
    for case in set["cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or("?").to_string();
        let tools = pick(&case["tools"])?;
        let compact = tc::encode_tools(&tools).map_err(|e| format!("{id}: {e}"))?;
        let compacted = compact.is_compacted();

        let messages = |system: String| {
            let mut m = vec![json!({"role": "system", "content": system})];
            m.extend(case["messages"].as_array().cloned().unwrap_or_default());
            m
        };
        let mut request = json!({"messages": messages(prefixed(&compact.prompt()))});
        if !compact.native.is_empty() {
            request["tools"] = compact
                .native
                .iter()
                .map(|t| originals[&t.name].clone())
                .collect();
        }
        let native: Vec<&Value> = tools.iter().map(|t| &originals[&t.name]).collect();
        let baseline = json!({"messages": messages(CLOCK.to_string()), "tools": native});
        baseline_tokens += count(&baseline);
        compact_tokens += count(&request);

        let bypassed: Vec<&str> = compact.bypassed.iter().map(|b| b.name.as_str()).collect();
        let (mut compact_calls, mut native_calls) = (vec![], vec![]);
        for c in case["expected"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let name = c["name"].as_str().unwrap_or_default();
            let bucket = if bypassed.contains(&name) {
                &mut native_calls
            } else {
                &mut compact_calls
            };
            bucket.push(c.clone());
        }
        let rendered: Vec<String> = compact_calls
            .iter()
            .map(|c| tc::render_call(c["name"].as_str().unwrap_or_default(), &c["arguments"]))
            .collect();
        let rendered = rendered.join("\n");

        let mut line = json!({
            "id": id,
            "compact_request": request,
            "compacted": compacted,
            "rendered_calls": rendered,
        });
        match tc::decode_calls(&rendered, &tools) {
            Ok(calls) => line["roundtrip_calls"] = call_objects(&calls),
            Err(e) => {
                line["roundtrip_calls"] = json!([]);
                line["roundtrip_error"] = json!(e.code());
            }
        }
        if !native_calls.is_empty() {
            line["native_calls"] = Value::Array(native_calls);
        }
        if let Some(live) = &live {
            match live.complete(&request).await {
                Ok(raw) => {
                    line["live_calls"] = decoded(&raw, &tools);
                    line["raw_output"] = json!(raw);
                }
                Err(e) => line["live_error"] = json!(e),
            }
        }
        lines.push(line);
    }

    for case in set["decoder_cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or("?");
        let tools = pick(&case["tools"])?;
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .ok_or_else(|| format!("{id}: `chunks` missing"))?
            .iter()
            .map(|c| {
                c.as_str()
                    .ok_or_else(|| format!("{id}: chunk is not a string"))
            })
            .collect::<Res<_>>()?;
        lines.push(json!({"id": id, "decoded": stream_decode(&chunks, &tools).map_err(|e| format!("{id}: {e}"))?}));
    }

    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    match std::env::var("OUT") {
        Ok(out) => std::fs::write(&out, body).map_err(|e| format!("write {out}: {e}"))?,
        Err(_) => print!("{body}"),
    }
    if baseline_tokens > 0 {
        eprintln!(
            "local o200k_base estimate over {} cases: baseline={baseline_tokens} compact={compact_tokens} reduction={:.1}%",
            set["cases"].as_array().map_or(0, Vec::len),
            100.0 * (1.0 - compact_tokens as f64 / baseline_tokens as f64)
        );
    }
    Ok(())
}

fn prefixed(prompt: &str) -> String {
    if prompt.is_empty() {
        CLOCK.to_string()
    } else {
        format!("{CLOCK}\n{prompt}")
    }
}

fn call_objects(calls: &[tc::ToolCall]) -> Value {
    calls
        .iter()
        .map(|c| json!({"name": c.name, "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap_or(Value::Null)}))
        .collect()
}

fn decoded(text: &str, tools: &[tc::ToolDef]) -> Value {
    match tc::decode_calls(text, tools) {
        Ok(calls) => json!({"calls": call_objects(&calls)}),
        Err(e) => json!({"error": e.code()}),
    }
}

/// Feed `chunks` through the real [`tc::StreamDecoder`], exactly as a streaming router would.
fn stream_decode(chunks: &[&str], tools: &[tc::ToolDef]) -> Res<Value> {
    let mut d = tc::StreamDecoder::new(tools).map_err(|e| e.to_string())?;
    let mut calls = Vec::new();
    for c in chunks {
        match d.push(c) {
            Ok(done) => calls.extend(done),
            Err(e) => return Ok(json!({"error": e.code()})),
        }
    }
    Ok(match d.finish() {
        Ok(_) => json!({"calls": call_objects(&calls)}),
        Err(e) => json!({"error": e.code()}),
    })
}

impl Live {
    async fn complete(&self, request: &Value) -> Result<String, String> {
        let mut body = request.clone();
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .json(&body);
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("HTTP {status}: unreadable body: {e}"))?;
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        Ok(v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}
