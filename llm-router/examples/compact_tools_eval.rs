//! Compact tool schemas eval (`nasiko-tool-compact`).
//!
//! Run:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Reads `EVAL_SET` (`{tools, cases, decoder_cases}`) and writes one JSONL line of outputs per
//! case to `OUT`. It does not compute scores; the organisers' scorer does that.
//!
//! * `cases` → `compact_request` (the OpenAI chat body we would send), `compacted`,
//!   `rendered_calls` (expected calls in the compact call format), `roundtrip_calls` (that text
//!   decoded back) and `decoded_tools` (the schemas rebuilt from the compact text).
//! * `decoder_cases` → `decoded`: the `StreamDecoder` result, fed chunk by chunk. The call format
//!   is the brief's `<<call NAME {JSON}>>`, so chunks are used as given, with no conversion.
//!
//! Offline and deterministic by default. Live mode (format adherence) runs when both
//! `PROVIDER_BASE_URL` and `MODEL` are set: each `compact_request` is sent to
//! `{PROVIDER_BASE_URL}/chat/completions` at temperature 0 and the line gains `raw_output` and
//! `live_calls`. Optional: `PROVIDER_API_KEY` (bearer token), `LIVE_NATIVE=1` (also send the
//! native-tools request and add `native_calls`, for comparison), `LIVE_TIMEOUT_SECS` (default 60).
//!
//! A token summary (o200k_base, full request bodies) is printed to stderr.
//!
//! # Reference date: live mode only, on both sides
//!
//! The brief fixes "today" for live runs (`2026-10-02`, `Asia/Kolkata`) so relative dates resolve
//! the same for everyone. That line belongs to the *eval*, not to compaction, so it is added only
//! when a model is actually called, and then to the compact **and** the native request alike:
//!
//! * Offline (the token measurement), `compact_request` is the case's messages plus the compact
//!   definitions and nothing else, so it compares like-for-like with the native baseline the
//!   scorer builds from `tools` + `messages`. Adding the date there would charge the compact side
//!   ~16 tokens per case that the baseline never pays (public sample: 35.0% instead of 42.7%).
//! * Live, the same [`REFERENCE_TIME`] line leads the system message of every request sent,
//!   compact and native, and the reported `compact_request` is exactly the body that was sent.
//!
//! Note on the public sample: `ct-002` expects "tomorrow 10am" as `2026-10-04T10:00:00+05:30`,
//! one day after the brief's reference date. Every model we ran answers `2026-10-03`, natively and
//! compactly alike. Nothing here special-cases it.

use std::io::Write;
use std::time::Duration;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, StreamEvent, ToolCall, ToolDef, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde_json::{Map, Value, json};

/// Fixed reference time from the brief, so relative dates resolve the same for every run.
const REFERENCE_TIME: &str = "Today: Friday 2026-10-02, Asia/Kolkata.";
/// `model` in the request body when `MODEL` is unset (offline mode).
const DEFAULT_MODEL: &str = "default";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let catalog: Vec<Value> = data["tools"].as_array().cloned().unwrap_or_default();
    let model = std::env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let live = Live::from_env(&model);

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut tokens = TokenTally::new();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    for case in data["cases"].as_array().into_iter().flatten() {
        let native_tools = select_tools(&catalog, &case["tools"]);
        let messages = case["messages"].as_array().cloned().unwrap_or_default();
        let reference = live.as_ref().map(|_| REFERENCE_TIME);
        let mut line = eval_case(case, &native_tools, &messages, &model, reference);
        tokens.add(
            &native_request(&model, &messages, &native_tools),
            &line["compact_request"],
        );
        if let Some(live) = &live {
            runtime.block_on(live.run(&mut line, &native_tools, &messages));
        }
        writeln!(out, "{line}").expect("write OUT");
    }

    for case in data["decoder_cases"].as_array().into_iter().flatten() {
        let tools = to_tool_defs(&select_tools(&catalog, &case["tools"]));
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let line = match StreamDecoder::new(&tools) {
            Err(CompactError::Unsupported { .. }) => json!({
                "id": case["id"],
                "decoded": stream_decode(&chunks, &name_only(&tools)),
                "schema_validated": false,
            }),
            _ => json!({"id": case["id"], "decoded": stream_decode(&chunks, &tools)}),
        };
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    tokens.report();
}

/// One `cases[]` line. `reference` is the date line, set only in live mode (see module docs).
fn eval_case(
    case: &Value,
    native_tools: &[Value],
    messages: &[Value],
    model: &str,
    reference: Option<&str>,
) -> Value {
    let tools = to_tool_defs(native_tools);
    let compact = if native_tools.iter().all(is_function_tool) {
        encode_tools(&tools)
    } else {
        Err(CompactError::Unsupported {
            tool: String::new(),
            reason: "non-function tool".into(),
        })
    };
    let mut line = Map::new();
    line.insert("id".into(), case["id"].clone());
    match compact {
        Ok(compact) => {
            let system = match reference {
                Some(date) => format!("{date}\n{}", compact.system_prompt()),
                None => compact.system_prompt(),
            };
            let rendered = render_calls(&expected_calls(case));
            line.insert("compacted".into(), json!(true));
            line.insert(
                "compact_request".into(),
                json!({"model": model, "messages": with_system(system, messages)}),
            );
            line.insert("rendered_calls".into(), json!(rendered));
            line.insert(
                "roundtrip_calls".into(),
                calls_or_error(decode_calls(&rendered, &tools)),
            );
            line.insert(
                "decoded_tools".into(),
                match decode_tools(&compact) {
                    Ok(defs) => Value::Array(defs.iter().map(to_native).collect()),
                    Err(e) => json!({"error": e.kind()}),
                },
            );
        }
        Err(e) => {
            // Bypass: send native tools unchanged (scored as 0% savings).
            let mut request = native_request(model, messages, native_tools);
            if let Some(date) = reference {
                request["messages"] = Value::Array(with_system(date.into(), messages));
            }
            line.insert("compacted".into(), json!(false));
            line.insert("bypass_reason".into(), json!(e.to_string()));
            line.insert("compact_request".into(), request);
            // The call format still round-trips; only names and JSON are checked, since the
            // schema could not be compiled. Flagged so the line never overstates.
            let rendered = render_calls(&expected_calls(case));
            let loose = name_only(&tools);
            line.insert("rendered_calls".into(), json!(rendered));
            line.insert(
                "roundtrip_calls".into(),
                calls_or_error(decode_calls(&rendered, &loose)),
            );
            line.insert("roundtrip_schema_validated".into(), json!(false));
        }
    }
    Value::Object(line)
}

fn with_system(system: String, messages: &[Value]) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": system})];
    out.extend_from_slice(messages);
    out
}

fn native_request(model: &str, messages: &[Value], native_tools: &[Value]) -> Value {
    json!({"model": model, "messages": messages, "tools": native_tools})
}

/// The case's tools, looked up by name in the file's `tools` array, in the case's order.
fn select_tools(catalog: &[Value], names: &Value) -> Vec<Value> {
    names
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| {
            catalog
                .iter()
                .find(|t| t["function"]["name"] == *name)
                .cloned()
        })
        .collect()
}

fn is_function_tool(tool: &Value) -> bool {
    tool.get("type")
        .and_then(Value::as_str)
        .unwrap_or("function")
        == "function"
}

fn to_tool_defs(native: &[Value]) -> Vec<ToolDef> {
    native
        .iter()
        .map(|t| ToolDef {
            name: t["function"]["name"].as_str().unwrap_or_default().into(),
            description: t["function"]["description"].as_str().map(String::from),
            parameters: t["function"].get("parameters").cloned(),
        })
        .collect()
}

/// The same tools accepting any arguments object: for cases whose schema cannot be compiled,
/// so names and JSON syntax are still checked (and reported as not schema-validated).
fn name_only(tools: &[ToolDef]) -> Vec<ToolDef> {
    tools
        .iter()
        .map(|t| ToolDef {
            name: t.name.clone(),
            description: None,
            parameters: Some(json!({"type": "object"})),
        })
        .collect()
}

fn to_native(def: &ToolDef) -> Value {
    let mut function = json!({"name": def.name});
    if let Some(d) = &def.description {
        function["description"] = json!(d);
    }
    if let Some(p) = &def.parameters {
        function["parameters"] = p.clone();
    }
    json!({"type": "function", "function": function})
}

fn expected_calls(case: &Value) -> Vec<ToolCall> {
    case["expected"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| ToolCall {
            name: c["name"].as_str().unwrap_or_default().into(),
            arguments: c["arguments"].as_object().cloned().unwrap_or_default(),
        })
        .collect()
}

fn call_json(c: &ToolCall) -> Value {
    json!({"name": c.name, "arguments": c.arguments})
}

fn calls_or_error(result: Result<Vec<ToolCall>, CompactError>) -> Value {
    match result {
        Ok(calls) => Value::Array(calls.iter().map(call_json).collect()),
        Err(e) => json!({"error": e.kind()}),
    }
}

/// Feeds the chunks to a `StreamDecoder` one by one.
fn stream_decode(chunks: &[&str], tools: &[ToolDef]) -> Value {
    let run = || -> Result<Vec<ToolCall>, CompactError> {
        let mut decoder = StreamDecoder::new(tools)?;
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.push(chunk)?);
        }
        events.extend(decoder.finish()?);
        Ok(events
            .into_iter()
            .filter_map(|e| match e {
                StreamEvent::Call(c) => Some(c),
                StreamEvent::Text(_) => None,
            })
            .collect())
    };
    match run() {
        Ok(calls) => json!({"calls": calls.iter().map(call_json).collect::<Vec<_>>()}),
        Err(e) => json!({"error": e.kind()}),
    }
}

// ─── Live mode ──────────────────────────────────────────────────────────────

struct Live {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: String,
    native: bool,
}

impl Live {
    fn from_env(model: &str) -> Option<Live> {
        let base = std::env::var("PROVIDER_BASE_URL").ok()?;
        std::env::var("MODEL").ok()?;
        let base = base.trim_end_matches('/');
        let url = if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        };
        let timeout = std::env::var("LIVE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .expect("http client");
        eprintln!("live mode: {url} model={model}");
        Some(Live {
            client,
            url,
            api_key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
            model: model.into(),
            native: std::env::var("LIVE_NATIVE").is_ok_and(|v| v == "1"),
        })
    }

    async fn run(&self, line: &mut Value, native_tools: &[Value], messages: &[Value]) {
        let tools = to_tool_defs(native_tools);
        let compacted = line["compacted"] == json!(true);
        match self.send(&line["compact_request"]).await {
            Ok(message) => {
                let text = message["content"].as_str().unwrap_or_default().to_string();
                line["live_calls"] = if compacted {
                    calls_or_error(decode_calls(&text, &tools))
                } else {
                    native_calls(&message)
                };
                line["raw_output"] = json!(text);
            }
            Err(e) => line["live_error"] = json!(e),
        }
        if self.native {
            let mut request = native_request(&self.model, messages, native_tools);
            request["messages"] = Value::Array(with_system(REFERENCE_TIME.into(), messages));
            line["native_calls"] = match self.send(&request).await {
                Ok(message) => native_calls(&message),
                Err(e) => json!({"error": e}),
            };
        }
    }

    /// POSTs the body at temperature 0; returns `choices[0].message`.
    async fn send(&self, body: &Value) -> Result<Value, String> {
        let mut body = body.clone();
        body["temperature"] = json!(0);
        let mut request = self.client.post(&self.url).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        let payload: Value = response.json().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}: {payload}"));
        }
        Ok(payload["choices"][0]["message"].clone())
    }
}

/// Native `tool_calls` as `[{name, arguments}]` (arguments parsed from the JSON string).
fn native_calls(message: &Value) -> Value {
    let calls = message["tool_calls"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args = c["function"]["arguments"].as_str().unwrap_or("{}");
                json!({
                    "name": c["function"]["name"],
                    "arguments": serde_json::from_str::<Value>(args).unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

// ─── Token summary (stderr; the scorer counts its own) ──────────────────────

struct TokenTally {
    bpe: tiktoken_rs::CoreBPE,
    native: usize,
    compact: usize,
    cases: usize,
}

impl TokenTally {
    fn new() -> TokenTally {
        TokenTally {
            bpe: tiktoken_rs::o200k_base().expect("o200k_base"),
            native: 0,
            compact: 0,
            cases: 0,
        }
    }

    fn add(&mut self, native: &Value, compact: &Value) {
        self.native += self.count(native);
        self.compact += self.count(compact);
        self.cases += 1;
    }

    fn count(&self, body: &Value) -> usize {
        self.bpe.encode_ordinary(&body.to_string()).len()
    }

    fn report(&self) {
        if self.native == 0 {
            return;
        }
        let saving = 1.0 - self.compact as f64 / self.native as f64;
        eprintln!(
            "o200k_base over {} cases: native {} tokens, compact {} tokens, saving {:.1}%",
            self.cases,
            self.native,
            self.compact,
            saving * 100.0
        );
    }
}
