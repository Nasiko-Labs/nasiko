//! Compact tool-schema eval.
//!
//! Offline (default, deterministic, no network, no keys):
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live mode (format adherence): when `PROVIDER_BASE_URL` and `MODEL` are set, each
//! `compact_request` is sent to `{PROVIDER_BASE_URL}/chat/completions` (OpenAI-compatible,
//! temperature 0) and the line gains `raw_output` and `live_calls`. Optional:
//!   PROVIDER_API_KEY        bearer token; read from the environment only, never logged
//!   LIVE_NATIVE_BASELINE=1  also send the native-tools request and add `native_live_calls`
//!
//! Output is one JSON line per case and per decoder case. The scorer recomputes everything;
//! numbers printed to stderr are informational only.
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use nasiko_tool_compact::{
    DecodeError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_calls,
    validate_call,
};
use serde_json::{Map, Value, json};
use tiktoken_rs::CoreBPE;

/// Fixed reference time so relative dates resolve the same way for everyone.
const REFERENCE_TIME: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

type AnyError = Box<dyn std::error::Error>;

struct Live {
    client: reqwest::Client,
    url: String,
    model: String,
    key: Option<String>,
    native_baseline: bool,
}

fn main() -> Result<(), AnyError> {
    let path = std::env::var("EVAL_SET")
        .map_err(|_| "set EVAL_SET to the path of the eval JSON (see the header of this file)")?;
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let data: Value = serde_json::from_str(&raw).map_err(|e| format!("invalid eval JSON: {e}"))?;

    let tools = load_tools(&data)?;
    let live = Live::from_env()?;
    let bpe = tiktoken_rs::o200k_base()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path).map_err(|e| format!("cannot create {out_path}: {e}"))?,
    );
    for case in array(&data, "cases") {
        let line = runtime.block_on(run_case(&bpe, case, &tools, live.as_ref()))?;
        writeln!(out, "{line}")?;
    }
    for case in array(&data, "decoder_cases") {
        writeln!(out, "{}", run_decoder_case(case, &tools)?)?;
    }
    out.flush()?;
    Ok(())
}

fn array<'a>(data: &'a Value, key: &str) -> &'a [Value] {
    data.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// A tool as published (native JSON) and as the codec sees it.
struct Tool {
    native: Value,
    def: ToolDef,
}

fn load_tools(data: &Value) -> Result<BTreeMap<String, Tool>, AnyError> {
    let mut tools = BTreeMap::new();
    for native in array(data, "tools") {
        let function = native
            .get("function")
            .ok_or("tool without a `function` object")?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tool without a name")?
            .to_string();
        let def = ToolDef {
            name: name.clone(),
            description: function
                .get("description")
                .and_then(Value::as_str)
                .map(String::from),
            parameters: function.get("parameters").cloned(),
        };
        tools.insert(
            name,
            Tool {
                native: native.clone(),
                def,
            },
        );
    }
    Ok(tools)
}

/// The tool definitions a case or decoder case refers to, in the order listed.
fn select<'a>(ids: &Value, tools: &'a BTreeMap<String, Tool>) -> Result<Vec<&'a Tool>, AnyError> {
    let names = ids.as_array().ok_or("`tools` must be an array of names")?;
    names
        .iter()
        .map(|n| {
            let name = n.as_str().ok_or("tool name must be a string")?;
            tools
                .get(name)
                .ok_or_else(|| format!("unknown tool {name:?} in eval set").into())
        })
        .collect()
}

async fn run_case(
    bpe: &CoreBPE,
    case: &Value,
    tools: &BTreeMap<String, Tool>,
    live: Option<&Live>,
) -> Result<Value, AnyError> {
    let id = case.get("id").and_then(Value::as_str).ok_or("case id")?;
    let selected = select(&case["tools"], tools)?;
    let defs: Vec<ToolDef> = selected.iter().map(|t| t.def.clone()).collect();
    let natives: Vec<Value> = selected.iter().map(|t| t.native.clone()).collect();
    let messages = case
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let encoded = encode_tools(&defs).map_err(|e| format!("{id}: {e}"))?;
    // Whole-request bypass: if any requested tool cannot be compacted, send all natively.
    let mut bypass_reasons: Vec<Value> = encoded
        .bypassed
        .iter()
        .map(|b| json!({"tool": b.tool, "reason": b.reason}))
        .collect();
    let mut compacted = bypass_reasons.is_empty();

    // Never worse than native: if compaction would not shrink the complete request body,
    // send the native request instead. Exact o200k_base counts, same body shapes as the report.
    if compacted {
        let user_messages = Value::Array(messages.clone());
        let native_body = json!({"messages": user_messages, "tools": natives});
        let mut compact_messages = vec![json!({"role": "system", "content": encoded.text})];
        compact_messages.extend(messages.iter().cloned());
        let compact_body = json!({"messages": compact_messages});
        if count_tokens(bpe, &compact_body) >= count_tokens(bpe, &native_body) {
            compacted = false;
            bypass_reasons.push(json!({"tool": "*", "reason": "no_saving"}));
        }
    }

    // The fixed reference time is added only in live mode, as the brief specifies; the offline
    // request carries just the compact tool text (or nothing, when bypassed).
    let mut system_parts: Vec<&str> = Vec::new();
    if live.is_some() {
        system_parts.push(REFERENCE_TIME);
    }
    if compacted {
        system_parts.push(&encoded.text);
    }
    let mut all_messages = Vec::new();
    if !system_parts.is_empty() {
        all_messages.push(json!({"role": "system", "content": system_parts.join("\n\n")}));
    }
    all_messages.extend(messages);
    let mut request = Map::new();
    request.insert("messages".into(), Value::Array(all_messages.clone()));
    if live.is_some() {
        request.insert("temperature".into(), json!(0));
    }
    if !compacted {
        request.insert("tools".into(), Value::Array(natives.clone()));
    }
    if let Some(live) = live {
        request.insert("model".into(), json!(live.model));
    }
    let request = Value::Object(request);

    let expected = expected_calls(case)?;
    let rendered = render_calls(&expected);
    let (roundtrip, roundtrip_error) = match decode_calls(&rendered, &defs) {
        Ok(calls) => (calls_json(&calls), None),
        Err(e) => (json!([]), Some(e.code())),
    };

    let mut line = Map::new();
    line.insert("id".into(), json!(id));
    line.insert("compacted".into(), json!(compacted));
    line.insert("compact_request".into(), request.clone());
    line.insert("rendered_calls".into(), json!(rendered));
    line.insert("roundtrip_calls".into(), roundtrip);
    if let Some(code) = roundtrip_error {
        line.insert("roundtrip_error".into(), json!(code));
    }
    if !compacted {
        line.insert("bypass".into(), Value::Array(bypass_reasons));
    }

    if let Some(live) = live {
        let reply = live.chat(&request).await;
        add_live(&mut line, &COMPACT_KEYS, reply, &defs, compacted);
        if live.native_baseline {
            let mut native = request.as_object().cloned().unwrap_or_default();
            native.insert("messages".into(), json!(native_messages(&all_messages)));
            native.insert("tools".into(), Value::Array(natives));
            let reply = live.chat(&Value::Object(native)).await;
            add_live(&mut line, &NATIVE_KEYS, reply, &defs, false);
        }
    }
    Ok(Value::Object(line))
}

/// The native-tools baseline keeps only the reference-time system note, not the compact text.
fn native_messages(all: &[Value]) -> Vec<Value> {
    let mut messages = vec![json!({"role": "system", "content": REFERENCE_TIME})];
    messages.extend(all.iter().filter(|m| m["role"] != "system").cloned());
    messages
}

fn expected_calls(case: &Value) -> Result<Vec<ToolCall>, AnyError> {
    let mut calls = Vec::new();
    for call in array(case, "expected") {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .ok_or("expected call without a name")?;
        let arguments = call.get("arguments").cloned().unwrap_or_else(|| json!({}));
        calls.push(ToolCall {
            name: name.into(),
            arguments: serde_json::to_string(&arguments)?,
        });
    }
    Ok(calls)
}

/// `[{"name":..,"arguments":{..}}]`, arguments as an object (the eval's shape).
fn calls_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                let args = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                json!({"name": c.name, "arguments": args})
            })
            .collect(),
    )
}

fn run_decoder_case(case: &Value, tools: &BTreeMap<String, Tool>) -> Result<Value, AnyError> {
    let id = case
        .get("id")
        .and_then(Value::as_str)
        .ok_or("decoder case id")?;
    let defs: Vec<ToolDef> = select(&case["tools"], tools)?
        .iter()
        .map(|t| t.def.clone())
        .collect();
    let chunks: Vec<&str> = array(case, "chunks")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let decoded = match decode_stream(&defs, &chunks) {
        Ok(calls) => json!({"calls": calls_json(&calls)}),
        Err(e) => json!({"error": e.code()}),
    };
    Ok(json!({"id": id, "decoded": decoded}))
}

/// Feed chunks one at a time, exactly as given. The first error ends the case with no calls.
fn decode_stream(defs: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(defs);
    let mut calls = Vec::new();
    for chunk in chunks {
        calls.extend(decoder.push(chunk)?);
    }
    calls.extend(decoder.finish()?);
    Ok(calls)
}

/// Attach the result of one live request to the output line.
/// Output field names for one live request (compact or native baseline).
struct LiveKeys {
    raw: &'static str,
    calls: &'static str,
    error: &'static str,
    tokens: &'static str,
}

const COMPACT_KEYS: LiveKeys = LiveKeys {
    raw: "raw_output",
    calls: "live_calls",
    error: "live_error",
    tokens: "live_prompt_tokens",
};

const NATIVE_KEYS: LiveKeys = LiveKeys {
    raw: "native_raw_output",
    calls: "native_live_calls",
    error: "native_live_error",
    tokens: "native_live_prompt_tokens",
};

fn add_live(
    line: &mut Map<String, Value>,
    keys: &LiveKeys,
    reply: Result<(Value, Option<i64>), String>,
    defs: &[ToolDef],
    compact: bool,
) {
    let (message, prompt_tokens) = match reply {
        Ok(ok) => ok,
        Err(class) => {
            line.insert(keys.error.into(), json!(class));
            return;
        }
    };
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    line.insert(keys.raw.into(), json!(text));
    if let Some(tokens) = prompt_tokens {
        line.insert(keys.tokens.into(), json!(tokens));
    }
    let decoded = if compact {
        decode_calls(&text, defs)
    } else {
        native_tool_calls(&message, defs)
    };
    let value = match decoded {
        Ok(calls) => json!({"calls": calls_json(&calls)}),
        Err(e) => json!({"error": e.code()}),
    };
    line.insert(keys.calls.into(), value);
}

/// Validate the provider's native `tool_calls` against the original schemas.
fn native_tool_calls(message: &Value, defs: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut calls = Vec::new();
    for call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let function = call.get("function").unwrap_or(&Value::Null);
        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("");
        let parsed: Value = serde_json::from_str(arguments)
            .map_err(|_| DecodeError::InvalidArguments("arguments are not JSON".into()))?;
        validate_call(name, &parsed, defs)?;
        calls.push(ToolCall {
            name: name.into(),
            arguments: serde_json::to_string(&parsed)
                .map_err(|_| DecodeError::InvalidArguments("not serialisable".into()))?,
        });
    }
    Ok(calls)
}

impl Live {
    /// `Some` only when both `PROVIDER_BASE_URL` and `MODEL` are set.
    fn from_env() -> Result<Option<Live>, AnyError> {
        let (Ok(base), Ok(model)) = (std::env::var("PROVIDER_BASE_URL"), std::env::var("MODEL"))
        else {
            return Ok(None);
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Some(Live {
            client,
            url: format!("{}/chat/completions", base.trim_end_matches('/')),
            model,
            key: std::env::var("PROVIDER_API_KEY")
                .ok()
                .filter(|k| !k.is_empty()),
            native_baseline: std::env::var("LIVE_NATIVE_BASELINE").as_deref() == Ok("1"),
        }))
    }

    /// Returns the assistant message, or a short error class. Error text never includes the
    /// request, headers or key.
    async fn chat(&self, request: &Value) -> Result<(Value, Option<i64>), String> {
        let mut body = request.clone();
        body["model"] = json!(self.model);
        let mut call = self.client.post(&self.url).json(&body);
        if let Some(key) = &self.key {
            call = call.bearer_auth(key);
        }
        let response = call.send().await.map_err(|e| {
            if e.is_timeout() {
                "timeout".to_string()
            } else {
                "connect".to_string()
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("http_{}", status.as_u16()));
        }
        let parsed: Value = response.json().await.map_err(|_| "bad_json".to_string())?;
        let prompt_tokens = parsed
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_i64);
        parsed
            .pointer("/choices/0/message")
            .cloned()
            .map(|message| (message, prompt_tokens))
            .ok_or_else(|| "no_message".to_string())
    }
}

fn count_tokens(bpe: &CoreBPE, body: &Value) -> usize {
    bpe.encode_with_special_tokens(&body.to_string()).len()
}
