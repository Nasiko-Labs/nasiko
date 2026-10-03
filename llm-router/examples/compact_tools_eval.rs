//! Eval harness for compact tool schemas (`nasiko-tool-compact`).
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Offline and deterministic by default: writes one JSONL line per case to `OUT` and prints a
//! local token count to stderr. Outputs only — the scorer computes every metric.
//!
//! Live mode: set `PROVIDER_BASE_URL` and `MODEL` (and `PROVIDER_API_KEY` if the endpoint needs
//! one). Each `compact_request` is sent to `{PROVIDER_BASE_URL}/chat/completions` with `model`
//! and `temperature: 0` added, and the line gains `raw_output` and `live_calls`.
//!
//! The call grammar is the brief's own `<<call name {json}>>`, so `decoder_cases` chunks are fed
//! to the decoder exactly as given.

use std::io::Write;

use nasiko_tool_compact::{
    CompactError, Event, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
    render_calls,
};
use serde_json::{Value, json};

/// Fixed reference time for the eval, so relative dates resolve the same way on every run.
const REFERENCE_TIME: &str = "Today is 2026-10-02, timezone Asia/Kolkata.";

struct Live {
    http: reqwest::Client,
    url: String,
    model: String,
    key: Option<String>,
}

#[tokio::main]
async fn main() {
    let (Ok(eval_set), Ok(out)) = (std::env::var("EVAL_SET"), std::env::var("OUT")) else {
        eprintln!("EVAL_SET and OUT must be set");
        std::process::exit(2);
    };
    let set: Value = match std::fs::read_to_string(&eval_set)
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|e| e.to_string()))
    {
        Ok(set) => set,
        Err(e) => {
            eprintln!("cannot read {eval_set}: {e}");
            std::process::exit(2);
        }
    };
    let live = match (std::env::var("PROVIDER_BASE_URL"), std::env::var("MODEL")) {
        (Ok(base), Ok(model)) if !base.is_empty() && !model.is_empty() => Some(Live {
            http: reqwest::Client::new(),
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            key: std::env::var("PROVIDER_API_KEY").ok(),
        }),
        _ => None,
    };

    let native_tools = set["tools"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    let mut tokens = Tokens::default();

    for case in set["cases"].as_array().into_iter().flatten() {
        let native = select(&native_tools, &case["tools"]);
        let tools: Vec<ToolDef> = native.iter().filter_map(to_compact_def).collect();
        let messages = case["messages"].as_array().cloned().unwrap_or_default();

        // A tool that cannot be carried compactly bypasses compaction for the whole case.
        let encoded = (tools.len() == native.len())
            .then(|| encode_tools(&tools).ok())
            .flatten();
        let compact_request = match &encoded {
            Some(compact) => {
                let system = format!("{REFERENCE_TIME}\n{}", compact.prompt());
                let mut all = vec![json!({"role": "system", "content": system})];
                all.extend(messages.iter().cloned());
                json!({"messages": all})
            }
            None => {
                let mut all = vec![json!({"role": "system", "content": REFERENCE_TIME})];
                all.extend(messages.iter().cloned());
                json!({"messages": all, "tools": native})
            }
        };
        tokens.add(&messages, &native, &compact_request);

        let mut line = json!({
            "id": case["id"],
            "compact_request": compact_request,
            "compacted": encoded.is_some(),
        });
        if encoded.is_some() {
            let expected: Vec<ToolCall> = case["expected"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|call| ToolCall {
                    name: call["name"].as_str().unwrap_or_default().to_string(),
                    arguments: call["arguments"].to_string(),
                })
                .collect();
            match render_calls(&expected) {
                Ok(rendered) => {
                    line["roundtrip_calls"] = outcome(decode_calls(&rendered, &tools));
                    line["rendered_calls"] = Value::String(rendered);
                }
                Err(e) => line["roundtrip_calls"] = json!({"error": e.as_label()}),
            }
        }
        if let Some(live) = &live {
            match live.send(&line["compact_request"]).await {
                Ok(message) => {
                    let text = message["content"].as_str().unwrap_or_default();
                    line["live_calls"] = if encoded.is_some() {
                        outcome(decode_calls(text, &tools))
                    } else {
                        native_calls(&message)
                    };
                    line["raw_output"] = Value::String(text.to_string());
                }
                Err(e) => line["live_error"] = Value::String(e),
            }
        }
        lines.push(line);
    }

    for case in set["decoder_cases"].as_array().into_iter().flatten() {
        let tools: Vec<ToolDef> = select(&native_tools, &case["tools"])
            .iter()
            .filter_map(to_compact_def)
            .collect();
        let chunks = case["chunks"].as_array().into_iter().flatten();
        let decoded = decode_chunks(chunks.filter_map(Value::as_str), &tools);
        lines.push(json!({"id": case["id"], "decoded": outcome_object(decoded)}));
    }

    let written = std::fs::File::create(&out).and_then(|mut file| {
        lines
            .iter()
            .try_for_each(|line| writeln!(file, "{line}"))
    });
    if let Err(e) = written {
        eprintln!("cannot write {out}: {e}");
        std::process::exit(2);
    }
    tokens.report(lines.len(), &out);
}

/// The file's tool definitions named by a case, in the case's order.
fn select(all: &[Value], names: &Value) -> Vec<Value> {
    names
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| all.iter().find(|tool| tool["function"]["name"] == *name))
        .cloned()
        .collect()
}

/// OpenAI tool definition → the crate's `ToolDef`. `None` for anything that is not a plain
/// function tool, which the caller treats as "do not compact".
fn to_compact_def(tool: &Value) -> Option<ToolDef> {
    let function = tool.get("function")?;
    (tool["type"] == "function").then_some(())?;
    Some(ToolDef {
        name: function["name"].as_str()?.to_string(),
        description: function["description"].as_str().map(str::to_string),
        parameters: function.get("parameters").cloned(),
    })
}

fn decode_chunks<'a>(
    chunks: impl Iterator<Item = &'a str>,
    tools: &[ToolDef],
) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk)?);
    }
    events.extend(decoder.finish()?);
    Ok(events
        .into_iter()
        .filter_map(|event| match event {
            Event::Call(call) => Some(call),
            Event::Text(_) => None,
        })
        .collect())
}

/// Calls in the eval set's own shape (`arguments` as an object), or `{"error": label}`.
fn outcome(result: Result<Vec<ToolCall>, CompactError>) -> Value {
    match result {
        Ok(calls) => calls
            .iter()
            .map(|call| {
                let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
                json!({"name": call.name, "arguments": arguments})
            })
            .collect(),
        Err(e) => json!({"error": e.as_label()}),
    }
}

fn outcome_object(result: Result<Vec<ToolCall>, CompactError>) -> Value {
    match outcome(result) {
        calls @ Value::Array(_) => json!({"calls": calls}),
        error => error,
    }
}

/// Native `tool_calls` from a bypassed case, in the same shape as [`outcome`].
fn native_calls(message: &Value) -> Value {
    message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|call| {
            let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
            let arguments: Value = serde_json::from_str(arguments).unwrap_or_default();
            json!({"name": call["function"]["name"], "arguments": arguments})
        })
        .collect()
}

impl Live {
    /// Returns the assistant message of the first choice.
    async fn send(&self, compact_request: &Value) -> Result<Value, String> {
        let mut body = compact_request.clone();
        body["model"] = Value::String(self.model.clone());
        body["temperature"] = json!(0);
        let mut request = self.http.post(&self.url).json(&body);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{status}: {body}"));
        }
        Ok(body["choices"][0]["message"].clone())
    }
}

/// Local measurement only (`o200k_base` over the serialized request body).
#[derive(Default)]
struct Tokens {
    /// `{messages, tools}` exactly as the case gives them.
    baseline: usize,
    /// The same, plus the reference-time system message the compact request also carries.
    baseline_with_time: usize,
    compact: usize,
}

impl Tokens {
    fn add(&mut self, messages: &[Value], native: &[Value], compact_request: &Value) {
        let Ok(bpe) = tiktoken_rs::o200k_base() else {
            return;
        };
        let count = |body: &Value| bpe.encode_ordinary(&body.to_string()).len();
        let mut timed = vec![json!({"role": "system", "content": REFERENCE_TIME})];
        timed.extend(messages.iter().cloned());
        self.baseline += count(&json!({"messages": messages, "tools": native}));
        self.baseline_with_time += count(&json!({"messages": timed, "tools": native}));
        self.compact += count(compact_request);
    }

    fn report(&self, lines: usize, out: &str) {
        let reduction = |baseline: usize| 100.0 * (1.0 - self.compact as f64 / baseline as f64);
        eprintln!("wrote {lines} lines to {out}");
        eprintln!(
            "tokens (o200k_base, local): compact {} vs native {} = {:.1}% reduction",
            self.compact,
            self.baseline,
            reduction(self.baseline)
        );
        eprintln!(
            "  with the reference-time message in the native baseline too: {} = {:.1}% reduction",
            self.baseline_with_time,
            reduction(self.baseline_with_time)
        );
    }
}
