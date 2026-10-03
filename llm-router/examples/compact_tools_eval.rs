//! Offline (and optional live) evaluation for compact tool schemas.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json \
//! OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! The input JSON is either an object with a `cases` array or an array of cases.
//! Tools may be declared once at the top or on each case (or on `request.tools`).
//! A case with `chunks` (or `text` and no user message) is a decoder case. Every
//! other case is a compact-request case. Nothing in this file is hard-coded to
//! the public sample ids.
//!
//! Live calls run only when both `PROVIDER_BASE_URL` and `MODEL` are set. The
//! optional `PROVIDER_API_KEY` is sent as a bearer token. Temperature is 0.
//! This example never reads a key from the repository.

use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::process::ExitCode;

use nasiko_tool_compact::{
    StreamDecoder, StreamItem, ToolCall, ToolDef, decode_calls, encode_tools, render_call,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    evals: Vec<Case>,
    #[serde(default)]
    items: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: Value,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    tools: Option<Vec<ToolDef>>,
    #[serde(default)]
    messages: Option<Vec<Value>>,
    #[serde(default)]
    request: Option<Value>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    chunks: Option<Value>,
    #[serde(default)]
    expected_calls: Option<Vec<ExpectedCall>>,
    #[serde(default)]
    expected: Option<Expected>,
    /// When true, a decoder error is the correct outcome.
    #[serde(default)]
    expect_error: bool,
}

#[derive(Debug, Deserialize)]
struct Expected {
    #[serde(default)]
    calls: Vec<ExpectedCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExpectedCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Serialize)]
struct RequestLine {
    id: Value,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_error: Option<String>,
    tool_names: Vec<String>,
    baseline_tokens: usize,
    compact_tokens: usize,
    tokens_saved: i64,
    reduction_percent: f64,
    roundtrip_success: bool,
}

#[derive(Serialize)]
struct DecoderLine {
    id: Value,
    decoded: Decoded,
    tool_names: Vec<String>,
    decoder_ok: bool,
    expected_error: bool,
}

#[derive(Serialize)]
struct Decoded {
    #[serde(skip_serializing_if = "Option::is_none")]
    calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let eval_set = match env::var("EVAL_SET") {
        Ok(path) if !path.is_empty() => path,
        _ => {
            eprintln!("EVAL_SET must point at the evaluation JSON");
            return ExitCode::from(2);
        }
    };
    let out = match env::var("OUT") {
        Ok(path) if !path.is_empty() => path,
        _ => {
            eprintln!("OUT must be the JSONL path to write");
            return ExitCode::from(2);
        }
    };
    let raw = match fs::read_to_string(&eval_set) {
        Ok(raw) => raw,
        Err(err) => {
            eprintln!("failed to read {eval_set}: {err}");
            return ExitCode::from(1);
        }
    };
    let Loaded {
        model: file_model,
        tools: file_tools,
        cases,
    } = match load_cases(&raw) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("failed to parse {eval_set}: {err}");
            return ExitCode::from(1);
        }
    };

    let bpe = tokenizer();
    let mut lines = Vec::with_capacity(cases.len());
    let mut baseline_tokens = 0usize;
    let mut compact_tokens = 0usize;
    let mut measured = 0usize;

    for case in &cases {
        let tools = tools_for(case, &file_tools);
        if is_decoder(case) {
            lines.push(
                serde_json::to_string(&decode_case(case, &tools)).unwrap_or_else(|err| {
                    json!({"id": case.id, "decoded": {"error": err.to_string()}}).to_string()
                }),
            );
            continue;
        }
        let mut line = request_case(case, &tools, file_model.as_deref()).await;
        if line.compacted
            && let Some(bpe) = &bpe
        {
            let baseline = baseline_body(case, &tools, file_model.as_deref());
            line.baseline_tokens = bpe.encode_with_special_tokens(&baseline.to_string()).len();
            line.compact_tokens = bpe
                .encode_with_special_tokens(&line.compact_request.to_string())
                .len();
            line.tokens_saved = line.baseline_tokens as i64 - line.compact_tokens as i64;
            line.reduction_percent = if line.baseline_tokens == 0 {
                0.0
            } else {
                line.tokens_saved as f64 / line.baseline_tokens as f64 * 100.0
            };
            baseline_tokens += line.baseline_tokens;
            compact_tokens += line.compact_tokens;
            measured += 1;
        }
        lines
            .push(serde_json::to_string(&line).unwrap_or_else(|err| {
                json!({"id": case.id, "error": err.to_string()}).to_string()
            }));
    }

    if let Some(parent) = std::path::Path::new(&out).parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {err}", parent.display());
        return ExitCode::from(1);
    }
    let file = match fs::File::create(&out) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("failed to create {out}: {err}");
            return ExitCode::from(1);
        }
    };
    let mut writer = BufWriter::new(file);
    for line in &lines {
        if writeln!(writer, "{line}").is_err() {
            eprintln!("failed to write {out}");
            return ExitCode::from(1);
        }
    }
    if writer.flush().is_err() {
        eprintln!("failed to flush {out}");
        return ExitCode::from(1);
    }

    if measured == 0 {
        eprintln!("tokens not measured (no compacted request cases, or tokenizer unavailable)");
    } else {
        let pct = if baseline_tokens == 0 {
            0.0
        } else {
            (baseline_tokens - compact_tokens) as f64 / baseline_tokens as f64 * 100.0
        };
        eprintln!(
            "tokens baseline={baseline_tokens} compact={compact_tokens} reduction_pct={pct:.1} cases={measured}"
        );
    }
    ExitCode::SUCCESS
}

struct Loaded {
    model: Option<String>,
    tools: Vec<ToolDef>,
    cases: Vec<Case>,
}

fn load_cases(raw: &str) -> Result<Loaded, String> {
    let root: Value = serde_json::from_str(raw).map_err(|err| err.to_string())?;
    if let Some(arr) = root.as_array() {
        let cases: Vec<Case> =
            serde_json::from_value(Value::Array(arr.clone())).map_err(|err| err.to_string())?;
        return Ok(Loaded {
            model: None,
            tools: Vec::new(),
            cases,
        });
    }
    let file: EvalFile = serde_json::from_value(root).map_err(|err| err.to_string())?;
    let cases = if !file.cases.is_empty() {
        file.cases
    } else if !file.evals.is_empty() {
        file.evals
    } else {
        file.items
    };
    if cases.is_empty() {
        return Err("evaluation file has no cases".into());
    }
    Ok(Loaded {
        model: file.model,
        tools: file.tools,
        cases,
    })
}

fn is_decoder(case: &Case) -> bool {
    if case.chunks.is_some() {
        return case.messages.is_none() && case.request.is_none() && user_text(case).is_none();
    }
    case.text.is_some()
        && case.messages.is_none()
        && case.request.is_none()
        && user_text(case).is_none()
}

fn user_text(case: &Case) -> Option<&str> {
    case.prompt
        .as_deref()
        .or(case.query.as_deref())
        .or(case.input.as_deref())
        .or(case.user.as_deref())
}

fn tools_for(case: &Case, file_tools: &[ToolDef]) -> Vec<ToolDef> {
    if let Some(tools) = &case.tools {
        return tools.clone();
    }
    if let Some(request) = &case.request
        && let Some(tools) = request.get("tools")
        && let Ok(parsed) = serde_json::from_value::<Vec<ToolDef>>(tools.clone())
    {
        return parsed;
    }
    file_tools.to_vec()
}

fn decode_case(case: &Case, tools: &[ToolDef]) -> DecoderLine {
    let names = tool_names(tools);
    let mut decoder = StreamDecoder::new(tools);
    let mut items = Vec::new();
    for chunk in chunks_of(case) {
        match decoder.push(&chunk) {
            Ok(next) => items.extend(next),
            Err(err) => {
                return decoder_line(case, names, None, Some(err.to_string()));
            }
        }
    }
    match decoder.finish() {
        Ok(next) => {
            items.extend(next);
            decoder_line(case, names, Some(calls_of(&items)), None)
        }
        Err(err) => decoder_line(case, names, None, Some(err.to_string())),
    }
}

fn decoder_line(
    case: &Case,
    tool_names: Vec<String>,
    calls: Option<Vec<Value>>,
    error: Option<String>,
) -> DecoderLine {
    let rejected = error.is_some();
    DecoderLine {
        id: case.id.clone(),
        decoded: Decoded { calls, error },
        tool_names,
        decoder_ok: rejected == case.expect_error,
        expected_error: case.expect_error,
    }
}

fn tool_names(tools: &[ToolDef]) -> Vec<String> {
    tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect()
}

fn chunks_of(case: &Case) -> Vec<String> {
    if let Some(chunks) = &case.chunks {
        if let Some(parts) = chunks.as_array() {
            return parts
                .iter()
                .map(|part| match part {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect();
        }
        if let Some(text) = chunks.as_str() {
            return vec![text.to_string()];
        }
    }
    case.text.clone().into_iter().collect()
}

fn calls_of(items: &[StreamItem]) -> Vec<Value> {
    items
        .iter()
        .filter_map(|item| match item {
            StreamItem::Call(call) => serde_json::to_value(call).ok(),
            StreamItem::Text(_) => None,
        })
        .collect()
}

async fn request_case(case: &Case, tools: &[ToolDef], file_model: Option<&str>) -> RequestLine {
    let encoded = encode_tools(tools).unwrap_or_else(|err| nasiko_tool_compact::CompactTools {
        prompt: String::new(),
        compacted: false,
        bypass_reason: Some(err.to_string()),
    });
    let compact_request = compact_body(case, tools, &encoded, file_model);
    let expected = expected_calls(case);
    let rendered_calls = render_expected(&expected).unwrap_or_default();
    let (roundtrip_calls, roundtrip_success) = if rendered_calls.is_empty() {
        (Vec::new(), expected.is_empty())
    } else {
        match decode_calls(&rendered_calls, tools) {
            Ok(calls) => {
                let values: Vec<Value> = calls
                    .iter()
                    .filter_map(|call| serde_json::to_value(call).ok())
                    .collect();
                let ok = calls_match(&expected, &values);
                (values, ok)
            }
            Err(_) => (Vec::new(), false),
        }
    };
    let mut line = RequestLine {
        id: case.id.clone(),
        compact_request,
        compacted: encoded.compacted,
        rendered_calls,
        roundtrip_calls,
        raw_output: None,
        live_calls: None,
        live_error: None,
        tool_names: tool_names(tools),
        baseline_tokens: 0,
        compact_tokens: 0,
        tokens_saved: 0,
        reduction_percent: 0.0,
        roundtrip_success,
    };
    if encoded.compacted
        && let Some(live) = live_config()
    {
        match live_complete(&live, &line.compact_request).await {
            Ok(raw) => {
                line.live_calls = Some(match decode_calls(&raw, tools) {
                    Ok(calls) => calls_to_json(&calls),
                    Err(_) => Vec::new(),
                });
                line.raw_output = Some(raw);
            }
            Err(err) => line.live_error = Some(err),
        }
    }
    line
}

fn calls_to_json(calls: &[ToolCall]) -> Vec<Value> {
    calls
        .iter()
        .filter_map(|call| serde_json::to_value(call).ok())
        .collect()
}

fn expected_calls(case: &Case) -> Vec<ExpectedCall> {
    if let Some(calls) = &case.expected_calls {
        return calls.clone();
    }
    case.expected
        .as_ref()
        .map(|expected| expected.calls.clone())
        .unwrap_or_default()
}

fn calls_match(expected: &[ExpectedCall], calls: &[Value]) -> bool {
    if expected.len() != calls.len() {
        return false;
    }
    expected.iter().zip(calls).all(|(want, got)| {
        let Some(function) = got.get("function") else {
            return false;
        };
        function.get("name").and_then(Value::as_str) == Some(want.name.as_str())
            && args_match(&want.arguments, function.get("arguments"))
    })
}

fn args_match(want: &Value, got: Option<&Value>) -> bool {
    let want = if want.is_null() {
        json!({})
    } else {
        want.clone()
    };
    let Some(got) = got else {
        return false;
    };
    let parsed = match got {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        other => other.clone(),
    };
    parsed == want
}

fn render_expected(calls: &[ExpectedCall]) -> Result<String, nasiko_tool_compact::Error> {
    let mut lines = Vec::with_capacity(calls.len());
    for call in calls {
        let arguments = match &call.arguments {
            Value::String(text) => {
                serde_json::from_str(text).unwrap_or(Value::String(text.clone()))
            }
            Value::Null => json!({}),
            other => other.clone(),
        };
        lines.push(render_call(&call.name, &arguments)?);
    }
    Ok(lines.join("\n"))
}

fn compact_body(
    case: &Case,
    tools: &[ToolDef],
    encoded: &nasiko_tool_compact::CompactTools,
    file_model: Option<&str>,
) -> Value {
    let mut body = base_body(case, file_model);
    let Some(map) = body.as_object_mut() else {
        return body;
    };
    if encoded.compacted {
        let mut messages = messages_of(case);
        messages.insert(0, json!({"role": "system", "content": encoded.prompt}));
        map.insert("messages".into(), Value::Array(messages));
        map.remove("tools");
        map.remove("tool_choice");
        map.insert("temperature".into(), json!(0));
    } else if !map.contains_key("tools")
        && !tools.is_empty()
        && let Ok(value) = serde_json::to_value(tools)
    {
        map.insert("tools".into(), value);
    }
    body
}

fn baseline_body(case: &Case, tools: &[ToolDef], file_model: Option<&str>) -> Value {
    let mut body = base_body(case, file_model);
    if let Some(map) = body.as_object_mut()
        && !map.contains_key("tools")
        && !tools.is_empty()
        && let Ok(value) = serde_json::to_value(tools)
    {
        map.insert("tools".into(), value);
    }
    if let Some(map) = body.as_object_mut()
        && !map.contains_key("messages")
    {
        map.insert("messages".into(), Value::Array(messages_of(case)));
    }
    body
}

fn base_body(case: &Case, file_model: Option<&str>) -> Value {
    let mut body = case.request.clone().unwrap_or_else(|| json!({}));
    if !body.is_object() {
        body = json!({});
    }
    let Some(map) = body.as_object_mut() else {
        return body;
    };
    if !map.contains_key("messages") {
        map.insert("messages".into(), Value::Array(messages_of(case)));
    }
    if !map.contains_key("model")
        && let Some(model) = case.model.as_deref().or(file_model)
    {
        map.insert("model".into(), Value::String(model.to_string()));
    }
    body
}

fn messages_of(case: &Case) -> Vec<Value> {
    if let Some(messages) = &case.messages {
        return messages.clone();
    }
    if let Some(request) = &case.request
        && let Some(messages) = request.get("messages").and_then(Value::as_array)
    {
        return messages.clone();
    }
    if let Some(text) = user_text(case) {
        return vec![json!({"role": "user", "content": text})];
    }
    Vec::new()
}

struct Live {
    base: String,
    model: String,
    key: Option<String>,
}

fn live_config() -> Option<Live> {
    let base = env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())?;
    let model = env::var("MODEL").ok().filter(|v| !v.is_empty())?;
    let key = env::var("PROVIDER_API_KEY").ok().filter(|v| !v.is_empty());
    Some(Live { base, model, key })
}

async fn live_complete(live: &Live, body: &Value) -> Result<String, String> {
    let mut payload = body.clone();
    if let Some(map) = payload.as_object_mut() {
        map.insert("model".into(), Value::String(live.model.clone()));
        map.insert("temperature".into(), json!(0));
        map.insert("stream".into(), json!(false));
    }
    let url = if live.base.ends_with("/chat/completions") {
        live.base.clone()
    } else {
        format!("{}/chat/completions", live.base.trim_end_matches('/'))
    };
    let client = reqwest::Client::new();
    let mut request = client.post(url).json(&payload);
    if let Some(key) = &live.key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("provider status {status}: {text}"));
    }
    let parsed: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    parsed
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("provider response has no message content: {text}"))
}

fn tokenizer() -> Option<tiktoken_rs::CoreBPE> {
    tiktoken_rs::o200k_base().ok()
}
