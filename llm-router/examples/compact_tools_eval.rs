//! Compact tool schemas eval.
//!
//! Run (offline, deterministic — no network, no keys):
//!   curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live mode (format adherence) — set all of:
//!   PROVIDER_BASE_URL  OpenAI-compatible base URL ending in `/v1`
//!   MODEL              model id sent in each request
//!   PROVIDER_API_KEY   bearer key (optional for keyless proxies)
//! Optional: LIVE_NATIVE=1 also sends the native-tools request per case and records the model's
//! native calls, for an adherence comparison on the same model.
//!
//! Writes one JSONL line per case to `OUT` and reports outputs, not scores. The request is built
//! by the router's own seam (`nasiko_llm_router::tool_compact::apply`), so the eval exercises the
//! code path the router runs. The call grammar is the brief's `<<call name {json}>>`, so decoder
//! cases are fed exactly as published — no conversion. Token counts (o200k_base) go to stderr.

use std::collections::HashMap;
use std::io::Write;

use nasiko_llm_router::ir::{ChatRequest, Message, ToolDef};
use nasiko_llm_router::tool_compact;
use nasiko_tool_compact as tc;
use serde_json::{Map, Value, json};

/// Fixed reference time from the brief, so relative dates resolve the same for every run.
const REFERENCE_TIME: &str = "Today: Fri 2026-10-02, Asia/Kolkata (+05:30).";

fn main() {
    let path = std::env::var("EVAL_SET").unwrap_or_else(|_| "compact-tools-eval.json".into());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| fail(&format!("read EVAL_SET {path}: {e}")));
    let data: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| fail(&format!("EVAL_SET is not JSON: {e}")));

    let all_tools: HashMap<String, ToolDef> = data["tools"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| serde_json::from_value::<ToolDef>(t.clone()).ok())
        .map(|t| (t.function.name.clone(), t))
        .collect();
    let select = |case: &Value| -> Vec<ToolDef> {
        case["tools"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|n| n.as_str().and_then(|n| all_tools.get(n)).cloned())
            .collect()
    };

    let live = Live::from_env();
    let bpe = tiktoken_rs::o200k_base().unwrap_or_else(|e| fail(&format!("load o200k_base: {e}")));
    let count = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path)
            .unwrap_or_else(|e| fail(&format!("create OUT {out_path}: {e}"))),
    );
    let (mut base_total, mut compact_total, mut bypassed) = (0usize, 0usize, 0usize);
    let mut adherence = Adherence::default();

    for case in data["cases"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or("?");
        let tools = select(case);
        let messages: Vec<Message> =
            serde_json::from_value(case["messages"].clone()).unwrap_or_default();

        // The baseline the scorer builds: native tools + the case's messages.
        let baseline = json!({ "messages": case["messages"], "tools": tools });

        let mut req = ChatRequest {
            model: None,
            messages: std::iter::once(system(REFERENCE_TIME))
                .chain(messages)
                .collect(),
            tools: (!tools.is_empty()).then(|| tools.clone()),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            extra: Map::new(),
        };
        let compaction = tool_compact::apply(&mut req);
        let compacted = compaction.is_ok();
        let compact_request = serde_json::to_value(&req).unwrap_or(Value::Null);

        let (b, c) = (count(&baseline), count(&compact_request));
        base_total += b;
        if compacted {
            compact_total += c;
        } else {
            bypassed += 1;
            compact_total += b;
        }

        let compact_tools = tool_compact::to_compact_tools(&tools);
        let expected: Vec<tc::ToolCall> = case["expected"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|e| tc::call(e["name"].as_str().unwrap_or(""), &e["arguments"]))
            .collect();
        let rendered = tc::render_calls(&expected);
        let roundtrip = match tc::decode_calls(&rendered, &compact_tools) {
            Ok(calls) => calls_json(&calls),
            Err(e) => json!({ "error": e.code() }),
        };

        let mut line = json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered,
            "roundtrip_calls": roundtrip,
        });
        if let Err(skipped) = &compaction {
            line["bypass_reason"] = json!(skipped.as_label());
        }

        if let Some(live) = &live {
            let result = live.complete(&compact_request);
            let live_calls = match &result {
                Ok(reply) if !reply.native_calls.is_empty() => {
                    // Only possible on a bypassed (native) request.
                    json!({ "calls": reply.native_calls })
                }
                Ok(reply) => match tc::decode_calls(&reply.text, &compact_tools) {
                    Ok(calls) => json!({ "calls": calls_json(&calls) }),
                    Err(e) => json!({ "error": e.code(), "detail": e.to_string() }),
                },
                Err(e) => json!({ "error": "request_failed", "detail": e }),
            };
            adherence.record(&live_calls, &expected, case);
            line["raw_output"] = match &result {
                Ok(reply) => json!(reply.text),
                Err(_) => Value::Null,
            };
            line["live_calls"] = live_calls;

            if live.native {
                let mut native = baseline.clone();
                native["messages"] = serde_json::to_value(
                    std::iter::once(system(REFERENCE_TIME))
                        .chain(
                            serde_json::from_value::<Vec<Message>>(case["messages"].clone())
                                .unwrap_or_default(),
                        )
                        .collect::<Vec<_>>(),
                )
                .unwrap_or(Value::Null);
                if tools.is_empty() {
                    native.as_object_mut().map(|o| o.remove("tools"));
                }
                let native_calls = match live.complete(&native) {
                    Ok(reply) => json!({ "calls": reply.native_calls, "text": reply.text }),
                    Err(e) => json!({ "error": "request_failed", "detail": e }),
                };
                adherence.record_native(&native_calls, &expected, case);
                line["native_calls"] = native_calls;
            }
        }

        eprintln!(
            "{id}: baseline {b} tok, compact {c} tok{}",
            if compacted { "" } else { " (bypassed)" }
        );
        writeln!(out, "{line}").unwrap_or_else(|e| fail(&format!("write OUT: {e}")));
    }

    for case in data["decoder_cases"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let id = case["id"].as_str().unwrap_or("?");
        let tools = tool_compact::to_compact_tools(&select(case));
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let decoded = match stream_decode(&tools, &chunks) {
            Ok(calls) => json!({ "calls": calls_json(&calls) }),
            Err(e) => json!({ "error": e.code() }),
        };
        eprintln!("{id}: {decoded}");
        writeln!(out, "{}", json!({ "id": id, "decoded": decoded }))
            .unwrap_or_else(|e| fail(&format!("write OUT: {e}")));
    }
    out.flush()
        .unwrap_or_else(|e| fail(&format!("flush OUT: {e}")));

    let saved = if base_total == 0 {
        0.0
    } else {
        1.0 - compact_total as f64 / base_total as f64
    };
    eprintln!(
        "\ntokens (o200k_base, full request body): baseline {base_total}, compact {compact_total}, \
         reduction {:.1}% ({bypassed} bypassed)",
        saved * 100.0
    );
    if live.is_some() {
        adherence.report();
    }
}

fn system(text: &str) -> Message {
    Message {
        role: "system".into(),
        content: Some(Value::String(text.into())),
        name: None,
        tool_calls: None,
        tool_call_id: None,
        extra: Map::new(),
    }
}

fn stream_decode(tools: &[tc::ToolDef], chunks: &[&str]) -> tc::Result<Vec<tc::ToolCall>> {
    let mut decoder = tc::StreamDecoder::new(tools)?;
    let mut calls = Vec::new();
    let mut collect = |events: Vec<tc::StreamEvent>| {
        calls.extend(events.into_iter().filter_map(|e| match e {
            tc::StreamEvent::Call(c) => Some(c),
            tc::StreamEvent::Text(_) => None,
        }))
    };
    for chunk in chunks {
        collect(decoder.push(chunk)?);
    }
    collect(decoder.finish()?);
    Ok(calls)
}

fn calls_json(calls: &[tc::ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| json!({ "name": c.name, "arguments": c.arguments_value().unwrap_or(Value::Null) }))
            .collect(),
    )
}

fn fail(msg: &str) -> ! {
    eprintln!("compact_tools_eval: {msg}");
    std::process::exit(2);
}

// ── live mode ────────────────────────────────────────────────────────────────────────────────

struct Live {
    url: String,
    model: String,
    key: Option<String>,
    native: bool,
    http: reqwest::Client,
    rt: tokio::runtime::Runtime,
}

struct Reply {
    text: String,
    native_calls: Vec<Value>,
}

impl Live {
    fn from_env() -> Option<Self> {
        let base = std::env::var("PROVIDER_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let model = std::env::var("MODEL").ok().filter(|s| !s.is_empty())?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .ok()?;
        eprintln!("live mode: {model} via {base}");
        Some(Self {
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            native: std::env::var("LIVE_NATIVE").is_ok_and(|v| v == "1" || v == "true"),
            http,
            rt,
        })
    }

    /// One completion at temperature 0. Models that reject an explicit temperature (some
    /// reasoning models only accept their default) are retried once without it.
    fn complete(&self, body: &Value) -> Result<Reply, String> {
        let mut body = body.clone();
        body["model"] = json!(self.model);
        body["temperature"] = json!(0);
        match self.send(&body) {
            Err(e) if e.contains("temperature") => {
                body.as_object_mut().map(|o| o.remove("temperature"));
                self.send(&body)
            }
            other => other,
        }
    }

    fn send(&self, body: &Value) -> Result<Reply, String> {
        self.rt.block_on(async {
            let mut req = self.http.post(&self.url).json(body);
            if let Some(key) = &self.key {
                req = req.bearer_auth(key);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!(
                    "HTTP {status}: {}",
                    text.chars().take(400).collect::<String>()
                ));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let msg = &v["choices"][0]["message"];
            let native_calls = msg["tool_calls"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|c| {
                    let args = c["function"]["arguments"].as_str().unwrap_or("null");
                    json!({
                        "name": c["function"]["name"],
                        "arguments": serde_json::from_str::<Value>(args).unwrap_or(Value::Null),
                    })
                })
                .collect();
            Ok(Reply {
                text: msg["content"].as_str().unwrap_or("").to_string(),
                native_calls,
            })
        })
    }
}

// ── local adherence summary (stderr only; the scorer recomputes from OUT) ──────────────────

#[derive(Default)]
struct Adherence {
    cases: usize,
    /// Replies that decoded cleanly (valid calls or a plain answer), right or wrong.
    compact_valid: usize,
    compact_ok: usize,
    native_ok: usize,
    native_ran: usize,
}

impl Adherence {
    fn record(&mut self, got: &Value, expected: &[tc::ToolCall], case: &Value) {
        self.cases += 1;
        if got.get("calls").is_some() {
            self.compact_valid += 1;
        }
        if matches_expected(got, expected, case) {
            self.compact_ok += 1;
        }
    }

    fn record_native(&mut self, got: &Value, expected: &[tc::ToolCall], case: &Value) {
        self.native_ran += 1;
        if matches_expected(got, expected, case) {
            self.native_ok += 1;
        }
    }

    fn report(&self) {
        eprintln!(
            "live: compact replies decoded cleanly in {}/{} cases",
            self.compact_valid, self.cases
        );
        eprintln!(
            "live: compact calls matched expected in {}/{} cases",
            self.compact_ok, self.cases
        );
        if self.native_ran > 0 {
            eprintln!(
                "live: native calls matched expected in {}/{} cases",
                self.native_ok, self.native_ran
            );
        }
    }
}

/// Same tools in order, same arguments; `free_text_fields` only need to be present strings.
fn matches_expected(got: &Value, expected: &[tc::ToolCall], case: &Value) -> bool {
    let Some(calls) = got["calls"].as_array() else {
        return false;
    };
    let free: Vec<&str> = case["match"]["free_text_fields"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    calls.len() == expected.len()
        && calls.iter().zip(expected).all(|(g, e)| {
            let want = e.arguments_value().unwrap_or(Value::Null);
            g["name"] == e.name.as_str()
                && match (g["arguments"].as_object(), want.as_object()) {
                    (Some(g), Some(w)) => {
                        g.len() == w.len()
                            && w.iter().all(|(k, v)| {
                                if free.contains(&k.as_str()) {
                                    g.get(k).is_some_and(Value::is_string)
                                } else {
                                    g.get(k) == Some(v)
                                }
                            })
                    }
                    _ => false,
                }
        })
}
