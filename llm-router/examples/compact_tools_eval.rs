//! Compact tool schemas eval.
//!
//! Run (offline, deterministic — no network):
//!   curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live mode (also sends every compact request — and its native-tools twin — to a model):
//!   PROVIDER_BASE_URL=https://…/v1 MODEL=… [PROVIDER_API_KEY=…] [LIVE_DATE_LINE="Today is …"]
//!
//! Writes one JSONL line per case to `OUT`:
//!   cases:          {id, compact_request, compacted, rendered_calls, roundtrip_calls[, raw_output, live_calls, native_calls]}
//!   decoder cases:  {id, decoded: {calls:[{name, arguments}]} | {error: "unknown_tool"|"invalid_arguments"}}
//! Token counts (tiktoken-rs, o200k_base) and pass counts go to stderr only — never into `OUT`.
//!
//! Decoder cases are fed to the streaming decoder chunk by chunk, unchanged: the call grammar is
//! exactly the brief's `<<call NAME {JSON}>>`.
use std::collections::BTreeMap;
use std::io::Write;

use nasiko_tool_compact::{self as tc, Event, ToolCall, ToolDef};
use serde_json::{Value, json};

/// Date line for live runs: the public cases resolve "tomorrow"/"Monday" against 2026-10-03.
/// Added to BOTH the compact and native arms, and never to the scored `compact_request`.
const DEFAULT_DATE_LINE: &str = "Today is 2026-10-03 (Saturday), timezone Asia/Kolkata.";

struct Live {
    base: String,
    model: String,
    key: Option<String>,
    date_line: String,
    http: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    // name → (native OpenAI tool JSON, crate ToolDef)
    let mut tools: BTreeMap<String, (Value, ToolDef)> = BTreeMap::new();
    for t in data["tools"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let f = &t["function"];
        let Some(name) = f["name"].as_str() else {
            continue;
        };
        let def = ToolDef {
            name: name.to_string(),
            description: f["description"].as_str().map(str::to_string),
            parameters: f.get("parameters").cloned(),
        };
        tools.insert(name.to_string(), (t.clone(), def));
    }
    let pick = |names: &Value| -> (Vec<Value>, Vec<ToolDef>) {
        let mut native = Vec::new();
        let mut defs = Vec::new();
        for n in names.as_array().map(Vec::as_slice).unwrap_or_default() {
            match n.as_str().and_then(|n| tools.get(n)) {
                Some((t, d)) => {
                    native.push(t.clone());
                    defs.push(d.clone());
                }
                None => eprintln!("warning: case references unknown tool {n}"),
            }
        }
        (native, defs)
    };

    let live = live_from_env();
    let bpe = tiktoken_rs::o200k_base().ok();
    let count = |v: &Value, pretty: bool| -> Option<usize> {
        let s = if pretty {
            serde_json::to_string_pretty(v).ok()?
        } else {
            serde_json::to_string(v).ok()?
        };
        Some(bpe.as_ref()?.encode_with_special_tokens(&s).len())
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut summary = Summary::default();

    for case in data["cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or_default();
        let (native_tools, defs) = pick(&case["tools"]);
        let messages = case["messages"].as_array().cloned().unwrap_or_default();

        let (compact_request, compacted) = match tc::encode_tools(&defs) {
            Ok(c) => {
                let mut msgs = vec![json!({"role": "system", "content": c.system_text()})];
                msgs.extend(messages.iter().cloned());
                (json!({ "messages": msgs }), true)
            }
            Err(e) => {
                eprintln!("{id}: bypass ({e}) — compact_request carries the native tools");
                summary.bypassed += 1;
                (
                    json!({ "messages": messages, "tools": native_tools }),
                    false,
                )
            }
        };
        let native_request = json!({ "messages": messages, "tools": native_tools });

        let expected: Vec<ToolCall> = case["expected"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|c| ToolCall {
                name: c["name"].as_str().unwrap_or_default().to_string(),
                arguments: c["arguments"].to_string(),
            })
            .collect();
        let rendered = tc::render_calls(&expected);
        let roundtrip = if compacted {
            match tc::decode_calls(&rendered, &defs) {
                Ok(calls) => calls_json(&calls),
                Err(e) => json!({ "error": e.code() }),
            }
        } else {
            // Bypassed: the request keeps native tools, so calls come back as native
            // `tool_calls` and are never transformed — they pass through unchanged.
            calls_json(&expected)
        };
        summary.cases += 1;
        if roundtrip == case["expected"] {
            summary.roundtrip_ok += 1;
        } else {
            eprintln!("{id}: round trip mismatch");
        }

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });

        if let Some(live) = &live {
            let (raw_output, live_calls) = live.compact_arm(&compact_request, &defs);
            line["raw_output"] = raw_output;
            line["live_calls"] = live_calls;
            line["native_calls"] = live.native_arm(&native_request);
        }

        if let (Some(n), Some(np), Some(c)) = (
            count(&native_request, false),
            count(&native_request, true),
            count(&compact_request, false),
        ) {
            summary.tokens.push((id.to_string(), n, np, c));
        }
        writeln!(out, "{line}").expect("write OUT");
    }

    for case in data["decoder_cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or_default();
        let (_, defs) = pick(&case["tools"]);
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let decoded = match decode_chunks(&defs, &chunks) {
            Ok(calls) => json!({ "calls": calls_json(&calls) }),
            Err(code) => json!({ "error": code }),
        };
        summary.decoder += 1;
        if case.get("expected").is_some_and(|e| *e == decoded) {
            summary.decoder_ok += 1;
        } else if case.get("expected").is_some() {
            eprintln!("{id}: decoder output differs from expected");
        }
        writeln!(out, "{}", json!({ "id": id, "decoded": decoded })).expect("write OUT");
    }
    out.flush().expect("flush OUT");
    summary.print(bpe.is_some(), live.as_ref().map(|l| l.model.as_str()));
}

/// Feed `chunks` to the streaming decoder in order; calls, or the external error code.
fn decode_chunks(defs: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, &'static str> {
    let mut decoder = tc::StreamDecoder::new(defs).map_err(|_| "invalid_arguments")?;
    let mut calls = Vec::new();
    let mut take = |events: Vec<Event>| {
        calls.extend(events.into_iter().filter_map(|e| match e {
            Event::Call(c) => Some(c),
            Event::Text(_) => None,
        }))
    };
    for chunk in chunks {
        take(decoder.feed(chunk).map_err(|e| e.code())?);
    }
    take(decoder.finish().map_err(|e| e.code())?);
    Ok(calls)
}

/// Calls as `[{name, arguments: {…object…}}]` — the eval's `expected` shape.
fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args: Value = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                json!({ "name": c.name, "arguments": args })
            })
            .collect(),
    )
}

fn live_from_env() -> Option<Live> {
    let base = std::env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty())?;
    Some(Live {
        base: base.trim_end_matches('/').to_string(),
        model,
        key: std::env::var("PROVIDER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        date_line: std::env::var("LIVE_DATE_LINE").unwrap_or_else(|_| DEFAULT_DATE_LINE.into()),
        http: reqwest::Client::new(),
        rt: tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    })
}

impl Live {
    /// POST `request` (+ model, temperature 0, date line) to `/chat/completions`.
    fn post(&self, request: &Value) -> Result<Value, String> {
        let mut body = request.clone();
        let mut msgs = vec![json!({"role": "system", "content": self.date_line})];
        msgs.extend(body["messages"].as_array().cloned().unwrap_or_default());
        body["messages"] = Value::Array(msgs);
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        self.rt.block_on(async {
            let mut req = self
                .http
                .post(format!("{}/chat/completions", self.base))
                .json(&body);
            if let Some(k) = &self.key {
                req = req.bearer_auth(k);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            let v: Value = resp.json().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {v}"));
            }
            Ok(v)
        })
    }

    fn compact_arm(&self, request: &Value, defs: &[ToolDef]) -> (Value, Value) {
        match self.post(request) {
            Ok(v) => {
                let text = v["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let calls = match tc::decode_calls(&text, defs) {
                    Ok(calls) => calls_json(&calls),
                    Err(e) => json!({ "error": e.code() }),
                };
                (json!(text), calls)
            }
            Err(e) => (
                Value::Null,
                json!({ "error": "request_failed", "detail": e }),
            ),
        }
    }

    fn native_arm(&self, request: &Value) -> Value {
        match self.post(request) {
            Ok(v) => Value::Array(
                v["choices"][0]["message"]["tool_calls"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|c| {
                        let args = c["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or(Value::Null);
                        json!({ "name": c["function"]["name"], "arguments": args })
                    })
                    .collect(),
            ),
            Err(e) => json!({ "error": "request_failed", "detail": e }),
        }
    }
}

#[derive(Default)]
struct Summary {
    cases: usize,
    roundtrip_ok: usize,
    bypassed: usize,
    decoder: usize,
    decoder_ok: usize,
    /// (id, native compact-JSON tokens, native pretty-JSON tokens, compact tokens)
    tokens: Vec<(String, usize, usize, usize)>,
}

impl Summary {
    fn print(&self, have_tokens: bool, live_model: Option<&str>) {
        eprintln!("── compact_tools_eval ──────────────────────────────────────");
        eprintln!("round trips    {}/{}", self.roundtrip_ok, self.cases);
        eprintln!("decoder cases  {}/{}", self.decoder_ok, self.decoder);
        eprintln!("bypassed       {}", self.bypassed);
        if let Some(m) = live_model {
            eprintln!("live model     {m} (raw_output / live_calls / native_calls in OUT)");
        }
        if !have_tokens {
            eprintln!("tokens         unavailable (tiktoken o200k_base failed to load)");
            return;
        }
        eprintln!("tokens (o200k_base, full request body; baseline = messages + native tools)");
        eprintln!(
            "  {:<12} {:>8} {:>8} {:>8} {:>9}",
            "case", "native", "pretty", "compact", "reduction"
        );
        let (mut n, mut np, mut c) = (0, 0, 0);
        for (id, a, b, d) in &self.tokens {
            eprintln!("  {id:<12} {a:>8} {b:>8} {d:>8} {:>8.1}%", pct(*a, *d));
            n += a;
            np += b;
            c += d;
        }
        eprintln!(
            "  {:<12} {n:>8} {np:>8} {c:>8} {:>8.1}%",
            "TOTAL",
            pct(n, c)
        );
        eprintln!("  vs pretty-printed native baseline: {:.1}%", pct(np, c));
    }
}

fn pct(native: usize, compact: usize) -> f64 {
    if native == 0 {
        0.0
    } else {
        100.0 * (1.0 - compact as f64 / native as f64)
    }
}
