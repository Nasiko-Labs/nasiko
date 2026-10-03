//! Compact tool schemas eval.
//!
//! Run:
//!   curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case to `OUT`. It does
//! not score; the token counts and match checks printed to stderr are a local development aid.
//!
//! Offline and deterministic by default: no network, no API key, no model call.
//!
//! Live mode (format adherence): set `PROVIDER_BASE_URL` and `MODEL` to send every
//! `compact_request` to `{PROVIDER_BASE_URL}/chat/completions` at temperature 0 and add
//! `raw_output` and `live_calls` to its line. If `PROVIDER_API_KEY` is set it is sent as a bearer
//! token; it is never written to `OUT`. Live requests start with the brief's fixed reference time
//! (2026-10-02, Asia/Kolkata), the same line a native live run gets. Offline requests leave it
//! out, so token counts compare like with like against a baseline built from tools and messages.
//! Optional `LIVE_DELAY_MS` pauses between live requests (useful on free tiers); progress and
//! retries are printed to stderr as they happen.
//!
//! The call grammar is the brief's `<<call NAME {json}>>`, so decoder cases are fed to
//! `StreamDecoder` verbatim, chunk by chunk, with no conversion.
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use nasiko_llm_router::ir;
use nasiko_tool_compact::{self as compact, Error, Event, StreamDecoder, ToolCall, ToolDef};
use serde_json::{Map, Value, json};

/// The brief's fixed reference time, so relative dates resolve the same way for everyone.
const REFERENCE_TIME: &str = "Today is Friday, 2026-10-02. Timezone: Asia/Kolkata (UTC+05:30).";

struct Tool {
    native: Value,
    def: ToolDef,
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");

    let tools: BTreeMap<String, Tool> = data["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|native| {
            // Parse through the router's own type, then convert at the seam.
            let t: ir::ToolDef =
                serde_json::from_value(native.clone()).expect("OpenAI-shaped tool");
            let def = ToolDef {
                name: t.function.name.clone(),
                description: t.function.description,
                parameters: t.function.parameters,
            };
            (
                t.function.name,
                Tool {
                    native: native.clone(),
                    def,
                },
            )
        })
        .collect();

    let live = Live::from_env();
    let bpe = tiktoken_rs::o200k_base().expect("load o200k_base");
    let count = |v: &Value| bpe.encode_ordinary(&v.to_string()).len();
    let mut report = Report::default();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for case in data["cases"].as_array().into_iter().flatten() {
        let line = run_case(case, &tools, live.as_ref(), &count, &mut report);
        writeln!(out, "{line}").expect("write OUT");
        // Live runs are slow; keep every finished case on disk if the run is stopped.
        out.flush().expect("flush OUT");
    }
    for case in data["decoder_cases"].as_array().into_iter().flatten() {
        let line = run_decoder_case(case, &tools, &mut report);
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    report.print();
}

fn run_case(
    case: &Value,
    tools: &BTreeMap<String, Tool>,
    live: Option<&Live>,
    count: &dyn Fn(&Value) -> usize,
    report: &mut Report,
) -> Value {
    let id = case["id"].as_str().unwrap_or("?").to_string();
    let selected = match select_tools(case, tools) {
        Ok(s) => s,
        Err(e) => {
            report.notes.push(format!("{id}: {e}"));
            return json!({"id": id, "error": e});
        }
    };
    let defs: Vec<ToolDef> = selected.iter().map(|t| t.def.clone()).collect();
    let native_tools: Vec<Value> = selected.iter().map(|t| t.native.clone()).collect();
    let messages: Vec<Value> = case["messages"].as_array().cloned().unwrap_or_default();
    let expected: Vec<Value> = case["expected"].as_array().cloned().unwrap_or_default();
    let free_text: Vec<String> =
        serde_json::from_value(case["match"]["free_text_fields"].clone()).unwrap_or_default();

    let reference = live.map(|_| REFERENCE_TIME);
    let native_request = request(reference, None, &messages, Some(native_tools));
    // Earlier tool calls in the history would need rewriting into the compact grammar too;
    // not supported yet, so those conversations go out native.
    let has_tool_history = messages
        .iter()
        .any(|m| m.get("tool_calls").is_some() || m["role"] == "tool");
    let encoded = if has_tool_history {
        Err("conversation already contains tool calls".to_string())
    } else {
        compact::encode_tools(&defs).map_err(|e| e.to_string())
    };

    let mut line = Map::new();
    line.insert("id".into(), id.clone().into());
    let sent = match &encoded {
        Ok(ct) => {
            let compact_request = request(reference, Some(&ct.prompt()), &messages, None);
            let roundtrip = expected
                .iter()
                .map(to_call)
                .collect::<Result<Vec<_>, _>>()
                .map(|calls| compact::render_calls(&calls))
                .map(|rendered| {
                    let back = match compact::decode_calls(&rendered, &defs) {
                        Ok(calls) => calls_json(&calls),
                        Err(e) => error_json(&e),
                    };
                    (rendered, back)
                });
            let (rendered, back) = match roundtrip {
                Ok((r, b)) => (Value::String(r), b),
                Err(e) => (
                    Value::Null,
                    json!({"error": "bad_expected_call", "detail": e}),
                ),
            };
            report
                .roundtrip
                .record(calls_match(&expected, &back, &free_text));
            line.insert("compact_request".into(), compact_request.clone());
            line.insert("compacted".into(), true.into());
            line.insert("rendered_calls".into(), rendered);
            line.insert("roundtrip_calls".into(), back);
            compact_request
        }
        Err(reason) => {
            // Bypass: the native request goes out unchanged and native calls come back unchanged.
            report.notes.push(format!("{id}: bypassed ({reason})"));
            line.insert("compact_request".into(), native_request.clone());
            line.insert("compacted".into(), false.into());
            line.insert("rendered_calls".into(), Value::Null);
            line.insert("roundtrip_calls".into(), Value::Array(expected.clone()));
            native_request.clone()
        }
    };

    report.tokens.push(TokenRow {
        id: id.clone(),
        native: count(&native_request),
        sent: count(&sent),
        compacted: encoded.is_ok(),
    });

    if let Some(live) = live {
        let (raw_output, live_calls) = match live.complete(&id, &sent) {
            Ok(message) => {
                let content = message["content"].as_str().unwrap_or("").to_string();
                let calls = if encoded.is_ok() {
                    match compact::decode_calls(&content, &defs) {
                        Ok(calls) => calls_json(&calls),
                        Err(e) => error_json(&e),
                    }
                } else {
                    native_calls(&message)
                };
                (Value::String(content), calls)
            }
            Err(e) => (Value::Null, json!({"error": "request_failed", "detail": e})),
        };
        let matched = calls_match(&expected, &live_calls, &free_text);
        report.live.record(matched);
        eprintln!(
            "live {id}: {}",
            if matched {
                "matches expected"
            } else {
                "does not match"
            }
        );
        if !matched {
            report
                .notes
                .push(format!("{id}: live output did not match: {raw_output}"));
        }
        line.insert("raw_output".into(), raw_output);
        line.insert("live_calls".into(), live_calls);
    }
    Value::Object(line)
}

fn run_decoder_case(case: &Value, tools: &BTreeMap<String, Tool>, report: &mut Report) -> Value {
    let id = case["id"].as_str().unwrap_or("?").to_string();
    let defs: Vec<ToolDef> = match select_tools(case, tools) {
        Ok(s) => s.iter().map(|t| t.def.clone()).collect(),
        Err(e) => {
            report.notes.push(format!("{id}: {e}"));
            return json!({"id": id, "error": e});
        }
    };
    let chunks: Vec<&str> = case["chunks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    let mut line = json!({"id": id});
    match stream_decode(&defs, &chunks) {
        Ok(calls) => line["decoded"] = json!({"calls": calls_json(&calls)}),
        Err(e) => {
            line["decoded"] = json!({"error": eval_code(&e)});
            line["error_detail"] = e.to_string().into();
        }
    }
    let ok = line["decoded"] == case["expected"];
    report.decoder.record(ok);
    if !ok {
        report.notes.push(format!(
            "{id}: decoded {} (expected {})",
            line["decoded"], case["expected"]
        ));
    }
    line
}

fn select_tools<'a>(
    case: &Value,
    tools: &'a BTreeMap<String, Tool>,
) -> Result<Vec<&'a Tool>, String> {
    case["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| {
            n.as_str()
                .and_then(|name| tools.get(name))
                .ok_or_else(|| format!("case references unknown tool {n}"))
        })
        .collect()
}

/// An OpenAI chat request. The optional reference time and compact tool prompt share one leading
/// system message; with neither, there is no system message at all.
fn request(
    reference: Option<&str>,
    compact_prompt: Option<&str>,
    messages: &[Value],
    tools: Option<Vec<Value>>,
) -> Value {
    let system: Vec<&str> = reference.into_iter().chain(compact_prompt).collect();
    let mut all = Vec::new();
    if !system.is_empty() {
        all.push(json!({"role": "system", "content": system.join("\n\n")}));
    }
    all.extend(messages.iter().cloned());
    let mut req = json!({"messages": all});
    if let Some(tools) = tools {
        req["tools"] = Value::Array(tools);
    }
    req
}

fn stream_decode(defs: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, Error> {
    let mut decoder = StreamDecoder::new(defs)?;
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk)?);
    }
    events.extend(decoder.finish()?);
    Ok(events
        .into_iter()
        .filter_map(|e| match e {
            Event::Call(c) => Some(c),
            Event::Text(_) => None,
        })
        .collect())
}

fn to_call(v: &Value) -> Result<ToolCall, String> {
    match (v["name"].as_str(), v["arguments"].as_object()) {
        (Some(name), Some(args)) => Ok(ToolCall {
            name: name.into(),
            arguments: args.clone(),
        }),
        _ => Err(format!("expected call is not {{name, arguments}}: {v}")),
    }
}

fn calls_json(calls: &[ToolCall]) -> Value {
    calls
        .iter()
        .map(|c| json!({"name": c.name, "arguments": c.arguments}))
        .collect()
}

/// The brief's error vocabulary is `unknown_tool` / `invalid_arguments`; a structurally broken
/// call (cut off, no argument object, no `>>`) is reported as `invalid_arguments`.
fn eval_code(e: &Error) -> &'static str {
    match e {
        Error::Malformed(_) => "invalid_arguments",
        other => other.code(),
    }
}

fn error_json(e: &Error) -> Value {
    json!({"error": eval_code(e), "detail": e.to_string()})
}

fn native_calls(message: &Value) -> Value {
    message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let args = c["function"]["arguments"].as_str().unwrap_or("{}");
            json!({
                "name": c["function"]["name"],
                "arguments": serde_json::from_str::<Value>(args).unwrap_or_else(|_| args.into()),
            })
        })
        .collect()
}

/// Same calls as `expected`, in any order. Free-text fields only need to be present with the
/// same JSON type.
fn calls_match(expected: &[Value], got: &Value, free_text: &[String]) -> bool {
    let Some(got) = got.as_array() else {
        return false;
    };
    if expected.len() != got.len() {
        return false;
    }
    let mut used = vec![false; got.len()];
    expected.iter().all(|e| {
        let found = got
            .iter()
            .enumerate()
            .position(|(i, g)| !used[i] && call_matches(e, g, free_text));
        found.map(|i| used[i] = true).is_some()
    })
}

fn call_matches(e: &Value, g: &Value, free_text: &[String]) -> bool {
    e["name"] == g["name"]
        && e["arguments"].is_object()
        && value_matches(&e["arguments"], &g["arguments"], free_text)
}

/// Structural equality where free-text keys (at any depth) only need the same JSON type, and
/// numbers compare by value (`24` matches `24.0`).
fn value_matches(e: &Value, g: &Value, free_text: &[String]) -> bool {
    match (e, g) {
        (Value::Object(eo), Value::Object(go)) => {
            eo.len() == go.len()
                && eo.iter().all(|(k, v)| match go.get(k) {
                    Some(gv) if free_text.contains(k) => same_kind(v, gv),
                    Some(gv) => value_matches(v, gv, free_text),
                    None => false,
                })
        }
        (Value::Array(ea), Value::Array(ga)) => {
            ea.len() == ga.len()
                && ea
                    .iter()
                    .zip(ga)
                    .all(|(x, y)| value_matches(x, y, free_text))
        }
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => e == g,
    }
}

fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

struct Live {
    url: String,
    model: String,
    key: Option<String>,
    /// Pause between requests (`LIVE_DELAY_MS`), to stay under free-tier per-minute limits.
    delay: Duration,
    started: std::cell::Cell<bool>,
    client: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

impl Live {
    fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let base = var("PROVIDER_BASE_URL")?;
        let model = var("MODEL")?;
        let delay_ms = var("LIVE_DELAY_MS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        eprintln!("live mode: {model} via {base}, {delay_ms} ms between requests");
        Some(Self {
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            key: var("PROVIDER_API_KEY"),
            delay: Duration::from_millis(delay_ms),
            started: std::cell::Cell::new(false),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("HTTP client"),
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
        })
    }

    /// The assistant message. Rate limits (429) and server errors are retried with backoff;
    /// a rate limit waits longer, since free tiers count requests per minute. Every wait is
    /// printed, so a slow run never looks stuck.
    fn complete(&self, id: &str, request: &Value) -> Result<Value, String> {
        let mut body = request.clone();
        body["model"] = self.model.clone().into();
        body["temperature"] = 0.into();
        self.rt.block_on(async {
            if self.started.replace(true) && !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            eprintln!("live {id}: sending");
            let mut last = String::new();
            let mut rate_limited = false;
            for attempt in 0..4u32 {
                if attempt > 0 {
                    let wait = if rate_limited { 15 } else { 1 } * 2u64.pow(attempt - 1);
                    eprintln!("live {id}: {last:.120}");
                    eprintln!("live {id}: retrying in {wait}s (attempt {}/4)", attempt + 1);
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
                let mut req = self.client.post(&self.url).json(&body);
                if let Some(key) = &self.key {
                    req = req.bearer_auth(key);
                }
                let resp = match req.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        last = format!("request failed: {e}");
                        continue;
                    }
                };
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    let v: Value = serde_json::from_str(&text)
                        .map_err(|e| format!("response is not JSON: {e}"))?;
                    return v["choices"][0]
                        .get("message")
                        .cloned()
                        .ok_or_else(|| "response has no choices[0].message".to_string());
                }
                last = format!(
                    "HTTP {status}: {}",
                    text.chars().take(300).collect::<String>()
                );
                rate_limited = status.as_u16() == 429;
                if !rate_limited && !status.is_server_error() {
                    break;
                }
            }
            Err(last)
        })
    }
}

#[derive(Default)]
struct Tally {
    ok: usize,
    total: usize,
}

impl Tally {
    fn record(&mut self, ok: bool) {
        self.total += 1;
        self.ok += usize::from(ok);
    }
}

struct TokenRow {
    id: String,
    /// The native request: messages and tools (plus the reference time in live mode, as sent).
    native: usize,
    /// What we actually send (compact, or native when bypassed).
    sent: usize,
    compacted: bool,
}

#[derive(Default)]
struct Report {
    tokens: Vec<TokenRow>,
    roundtrip: Tally,
    decoder: Tally,
    live: Tally,
    notes: Vec<String>,
}

impl Report {
    fn print(&self) {
        let pct = |sent: usize, base: usize| 100.0 * (1.0 - sent as f64 / base.max(1) as f64);
        eprintln!("\ntokens (o200k_base, full request body)");
        eprintln!(
            "{:<10} {:>8} {:>8} {:>8}",
            "case", "native", "sent", "saved"
        );
        for r in &self.tokens {
            let tag = if r.compacted { "" } else { "  (bypassed)" };
            eprintln!(
                "{:<10} {:>8} {:>8} {:>7.1}%{tag}",
                r.id,
                r.native,
                r.sent,
                pct(r.sent, r.native)
            );
        }
        let sum = |f: fn(&TokenRow) -> usize| self.tokens.iter().map(f).sum::<usize>();
        let (native, sent) = (sum(|r| r.native), sum(|r| r.sent));
        eprintln!(
            "{:<10} {:>8} {:>8} {:>7.1}%",
            "TOTAL",
            native,
            sent,
            pct(sent, native)
        );
        eprintln!(
            "\nround trip:    {}/{} match expected",
            self.roundtrip.ok, self.roundtrip.total
        );
        eprintln!(
            "decoder cases: {}/{} match expected",
            self.decoder.ok, self.decoder.total
        );
        if self.live.total > 0 {
            eprintln!(
                "live calls:    {}/{} match expected",
                self.live.ok, self.live.total
            );
        }
        for n in &self.notes {
            eprintln!("  note: {n}");
        }
    }
}
