//! Eval harness for compact tool schemas (`nasiko-tool-compact`).
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Offline and deterministic by default: writes one JSONL line per case to `OUT`, then prints a
//! summary to stdout (local token counts, one definition before and after, decoder results). The
//! summary is for a human reader; `OUT` holds outputs only and the scorer computes every metric.
//!
//! Live mode: set `PROVIDER_BASE_URL` and `MODEL` (and `PROVIDER_API_KEY` if the endpoint needs
//! one). Each `compact_request` is sent to `{PROVIDER_BASE_URL}/chat/completions` with `model`
//! and `temperature: 0` added, and the line gains `raw_output` and `live_calls`.
//!
//! The call grammar is the brief's own `<<call name {json}>>`, so `decoder_cases` chunks are fed
//! to the decoder exactly as given.

use std::io::Write;

use nasiko_tool_compact::{
    CompactError, CompactTools, Event, StreamDecoder, ToolCall, ToolDef, decode_calls,
    encode_tools, render_calls,
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
    let mut summary = Summary::new();

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
        summary.case(
            case["id"].as_str().unwrap_or_default(),
            &messages,
            &native,
            &compact_request,
            encoded.as_ref(),
        );

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
        let decoded = outcome_object(decode_chunks(chunks.filter_map(Value::as_str), &tools));
        summary.decoder_case(case, &decoded);
        lines.push(json!({"id": case["id"], "decoded": decoded}));
    }

    let written = std::fs::File::create(&out)
        .and_then(|mut file| lines.iter().try_for_each(|line| writeln!(file, "{line}")));
    if let Err(e) = written {
        eprintln!("cannot write {out}: {e}");
        std::process::exit(2);
    }
    summary.print(lines.len(), &out);
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

/// The human-readable report printed after `OUT` is written. Nothing here reaches `OUT`.
struct Summary {
    /// `None` if the tokenizer data cannot be loaded; token counts are then left out.
    bpe: Option<tiktoken_rs::CoreBPE>,
    cases: Vec<CaseTokens>,
    /// One tool as the client sent it, and the line that replaced it.
    example: Option<(String, String)>,
    decoder: Vec<DecoderRow>,
}

struct CaseTokens {
    id: String,
    /// `{messages, tools}` exactly as the case gives them.
    native: usize,
    /// The same, plus the reference-time system message the compact request also carries.
    native_timed: usize,
    compact: usize,
    /// Call-format text, tool lines, everything else. `None` for a bypassed case.
    parts: Option<(usize, usize, usize)>,
}

struct DecoderRow {
    id: String,
    pass: bool,
    expected: String,
    got: String,
    note: String,
}

impl Summary {
    fn new() -> Self {
        Self {
            bpe: tiktoken_rs::o200k_base().ok(),
            cases: Vec::new(),
            example: None,
            decoder: Vec::new(),
        }
    }

    fn case(
        &mut self,
        id: &str,
        messages: &[Value],
        native: &[Value],
        compact_request: &Value,
        compact: Option<&CompactTools>,
    ) {
        if self.example.is_none()
            && let (Some(tool), Some(compact)) = (native.first(), compact)
        {
            let line = compact.definitions.lines().next().unwrap_or_default();
            self.example = Some((tool.to_string(), line.to_string()));
        }
        let Some(bpe) = &self.bpe else {
            return;
        };
        let text = |s: &str| bpe.encode_ordinary(s).len();
        let count = |body: &Value| text(&body.to_string());
        let mut timed = vec![json!({"role": "system", "content": REFERENCE_TIME})];
        timed.extend(messages.iter().cloned());
        let total = count(compact_request);
        // Parts are counted on their own, so they are approximate; the remainder is the
        // reference time, the messages and the JSON around them.
        let parts = compact.map(|compact| {
            let lines = text(&compact.definitions);
            let fixed = text(&compact.prompt()).saturating_sub(lines);
            (fixed, lines, total.saturating_sub(fixed + lines))
        });
        self.cases.push(CaseTokens {
            id: id.to_string(),
            native: count(&json!({"messages": messages, "tools": native})),
            native_timed: count(&json!({"messages": timed, "tools": native})),
            compact: total,
            parts,
        });
    }

    fn decoder_case(&mut self, case: &Value, decoded: &Value) {
        let expected = &case["expected"];
        // An error case passes on the same label; a call case on the same names and arguments
        // (objects compare without regard to key order).
        let pass = match expected["error"].as_str() {
            Some(label) => decoded["error"] == label,
            None => decoded["calls"].is_array() && decoded["calls"] == expected["calls"],
        };
        let describe = |value: &Value| match (value["error"].as_str(), value["calls"].as_array()) {
            (Some(label), _) => format!("error: {label}"),
            (None, Some(calls)) => format!("{} call(s)", calls.len()),
            (None, None) => "nothing".to_string(),
        };
        self.decoder.push(DecoderRow {
            id: case["id"].as_str().unwrap_or_default().to_string(),
            pass,
            expected: describe(expected),
            got: describe(decoded),
            note: case["note"].as_str().unwrap_or_default().to_string(),
        });
    }

    fn print(&self, lines: usize, out: &str) {
        println!("wrote {lines} lines to {out}");
        self.print_tokens();
        self.print_example();
        self.print_decoder();
    }

    fn print_tokens(&self) {
        if self.cases.is_empty() {
            return;
        }
        let saved = |compact: usize, native: usize| {
            format!(
                "{:.1}%",
                100.0 * (1.0 - compact as f64 / native.max(1) as f64)
            )
        };
        println!("\nTokens per case (o200k_base, counted locally over the full request body)");
        println!("  native       {{messages, tools}} exactly as the case gives them");
        println!("  native+time  the same, plus the reference-time system message that the");
        println!("               compact request also carries (like for like)");
        println!(
            "\n  {:<10} {:>7} {:>12} {:>8} {:>10} {:>15}   compact = call text + tool lines + rest",
            "case", "native", "native+time", "compact", "vs native", "vs native+time"
        );
        let row = |id: &str, native: usize, timed: usize, compact: usize, detail: String| {
            println!(
                "  {id:<10} {native:>7} {timed:>12} {compact:>8} {:>10} {:>15}   {detail}",
                saved(compact, native),
                saved(compact, timed)
            );
        };
        for case in &self.cases {
            let detail = match case.parts {
                Some((fixed, lines, rest)) => format!("{fixed} + {lines} + {rest}"),
                None => "bypassed (compacted: false)".to_string(),
            };
            row(
                &case.id,
                case.native,
                case.native_timed,
                case.compact,
                detail,
            );
        }
        let sum = |pick: fn(&CaseTokens) -> usize| self.cases.iter().map(pick).sum::<usize>();
        let bypassed = self.cases.iter().filter(|c| c.parts.is_none()).count();
        row(
            "total",
            sum(|c| c.native),
            sum(|c| c.native_timed),
            sum(|c| c.compact),
            format!("{bypassed} of {} cases bypassed", self.cases.len()),
        );
    }

    fn print_example(&self) {
        let Some((native, compact)) = &self.example else {
            return;
        };
        let tokens = |s: &str| match &self.bpe {
            Some(bpe) => format!(" ({} tokens)", bpe.encode_ordinary(s).len()),
            None => String::new(),
        };
        println!("\nOne tool, before and after");
        println!("  native{}:\n    {native}", tokens(native));
        println!("  compact{}:\n    {compact}", tokens(compact));
    }

    fn print_decoder(&self) {
        if self.decoder.is_empty() {
            return;
        }
        println!("\nDecoder cases (chunks fed to StreamDecoder one at a time)");
        println!(
            "  {:<8} {:<6} {:<26} {:<26} note",
            "case", "result", "expected", "got"
        );
        for row in &self.decoder {
            let result = if row.pass { "pass" } else { "FAIL" };
            println!(
                "  {:<8} {result:<6} {:<26} {:<26} {}",
                row.id, row.expected, row.got, row.note
            );
        }
        let passed = self.decoder.iter().filter(|row| row.pass).count();
        println!("  {passed} of {} passed", self.decoder.len());
    }
}
