//! Compact tool-schema eval.
//!
//! Offline (default, no network):
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Live, when both are set: `PROVIDER_BASE_URL` (OpenAI-compatible root, `/v1`)
//! and `MODEL`. Optional `OPENAI_API_KEY` is sent as a bearer token and never
//! written to the output. Reference clock in the system message is fixed:
//! 2026-10-02, Asia/Kolkata.

use std::io::Write;

use nasiko_tool_compact::{Error, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools};
use serde_json::{Map, Value, json};

const CLOCK: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

fn main() {
    let eval_path = match std::env::var("EVAL_SET") {
        Ok(path) if !path.is_empty() => path,
        _ => usage_exit(),
    };
    let out_path = match std::env::var("OUT") {
        Ok(path) if !path.is_empty() => path,
        _ => usage_exit(),
    };
    let raw = match std::fs::read_to_string(&eval_path) {
        Ok(raw) => raw,
        Err(err) => fail(&format!("read EVAL_SET: {err}")),
    };
    let data: Value = match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(err) => fail(&format!("eval JSON: {err}")),
    };
    let Some(catalog) = data.get("tools").and_then(Value::as_array).cloned() else {
        fail("eval JSON missing tools array");
    };
    let Some(cases) = data.get("cases").and_then(Value::as_array).cloned() else {
        fail("eval JSON missing cases array");
    };
    let decoder_cases = data
        .get("decoder_cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let live_url = live_url();
    let runtime = match &live_url {
        Some(_) => match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => Some(runtime),
            Err(err) => fail(&format!("tokio runtime: {err}")),
        },
        None => None,
    };
    let bpe = match tiktoken_rs::o200k_base() {
        Ok(bpe) => Some(bpe),
        Err(err) => {
            eprintln!("o200k_base: {err}");
            None
        }
    };

    let mut out = match std::fs::File::create(&out_path) {
        Ok(file) => std::io::BufWriter::new(file),
        Err(err) => fail(&format!("create OUT: {err}")),
    };
    let mut total_baseline = 0usize;
    let mut total_compact = 0usize;
    let mut definition_tokens = 0usize;
    let mut instruction_tokens = 0usize;
    for case in &cases {
        let (mut line, defs, compact_text) = case_line(case, &catalog);
        if let Some(bpe) = &bpe {
            let id = line
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let baseline_body = baseline_request(case, &catalog);
            let baseline = token_len(bpe, &baseline_body);
            let compact = line
                .get("compact_request")
                .map(|body| token_len(bpe, body))
                .unwrap_or(0);
            total_baseline += baseline;
            total_compact += compact;
            if let Some(text) = &compact_text {
                let (definitions, instruction) = split_defs_and_instruction(text);
                definition_tokens += bpe.encode_with_special_tokens(&definitions).len();
                instruction_tokens += bpe.encode_with_special_tokens(&instruction).len();
            }
            eprintln!(
                "{id} baseline={baseline} compact={compact} saving={}",
                saving_text(baseline, compact)
            );
        }
        if let (Some(url), Some(runtime)) = (&live_url, &runtime) {
            let request = line
                .get("compact_request")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (raw_output, live_calls) = runtime.block_on(live_once(url, &request, &defs));
            insert(&mut line, "raw_output", json!(raw_output));
            insert(&mut line, "live_calls", live_calls);
        }
        if writeln!(out, "{}", json_line(&line)).is_err() {
            fail("write OUT");
        }
    }
    for case in &decoder_cases {
        if writeln!(out, "{}", json_line(&decoder_line(case, &catalog))).is_err() {
            fail("write OUT");
        }
    }
    if out.flush().is_err() {
        fail("flush OUT");
    }
    if bpe.is_some() {
        eprintln!(
            "total baseline={total_baseline} compact={total_compact} saving={}",
            saving_text(total_baseline, total_compact)
        );
        if total_baseline > 0 && total_compact * 10 > total_baseline * 7 {
            eprintln!(
                "tool_definition_lines={definition_tokens} call_format_instruction={instruction_tokens}"
            );
        }
    }
}

fn usage_exit() -> ! {
    eprintln!(
        "usage: EVAL_SET=<eval.json> OUT=<out.jsonl> cargo run --release -p nasiko-llm-router --example compact_tools_eval"
    );
    std::process::exit(2);
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// Native body: case messages plus the full OpenAI tool JSON. Not written to OUT.
fn baseline_request(case: &Value, catalog: &[Value]) -> Value {
    let tools = select(catalog, case.get("tools"))
        .into_iter()
        .cloned()
        .collect();
    let mut request = Map::new();
    request.insert(
        "messages".to_string(),
        case.get("messages").cloned().unwrap_or_else(|| json!([])),
    );
    request.insert("tools".to_string(), Value::Array(tools));
    request.insert("temperature".to_string(), json!(0));
    if let Some(model) = nonempty_env("MODEL") {
        request.insert("model".to_string(), json!(model));
    }
    Value::Object(request)
}

fn token_len(bpe: &tiktoken_rs::CoreBPE, value: &Value) -> usize {
    match serde_json::to_string(value) {
        Ok(text) => bpe.encode_with_special_tokens(&text).len(),
        Err(_) => 0,
    }
}

fn saving_text(baseline: usize, compact: usize) -> String {
    if baseline == 0 {
        return "n/a".to_string();
    }
    format!("{:.3}", 1.0 - (compact as f64 / baseline as f64))
}

/// Tool lines, then the two call-format lines. See `tool-compact/GRAMMAR.md`.
fn split_defs_and_instruction(text: &str) -> (String, String) {
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return (text.to_string(), String::new());
    }
    let instruction = lines.split_off(lines.len() - 2).join("\n");
    (lines.join("\n"), instruction)
}

fn case_line(case: &Value, catalog: &[Value]) -> (Value, Vec<ToolDef>, Option<String>) {
    let selected = select(catalog, case.get("tools"));
    let defs: Vec<ToolDef> = selected.iter().copied().map(to_def).collect();
    let encoded = encode_tools(&defs);
    let compacted = encoded.is_ok();
    let compact_text = encoded.ok().map(|compact| compact.text);

    let mut messages = vec![json!({
        "role": "system",
        "content": match &compact_text {
            Some(text) => format!("{CLOCK}\n{text}"),
            None => CLOCK.to_string(),
        },
    })];
    if let Some(rows) = case.get("messages").and_then(Value::as_array) {
        messages.extend(rows.iter().cloned());
    }

    let mut request = Map::new();
    request.insert("messages".to_string(), Value::Array(messages));
    request.insert("temperature".to_string(), json!(0));
    if let Some(model) = nonempty_env("MODEL") {
        request.insert("model".to_string(), json!(model));
    }
    if !compacted {
        let tools = selected.into_iter().cloned().collect();
        request.insert("tools".to_string(), Value::Array(tools));
    }

    let rendered = render_expected(case.get("expected").and_then(Value::as_array));
    let (roundtrip, roundtrip_error) = roundtrip(&rendered, &defs);
    let mut line = Map::new();
    line.insert(
        "id".to_string(),
        case.get("id").cloned().unwrap_or(json!("")),
    );
    line.insert("compact_request".to_string(), Value::Object(request));
    line.insert("compacted".to_string(), json!(compacted));
    line.insert("rendered_calls".to_string(), json!(rendered));
    line.insert("roundtrip_calls".to_string(), roundtrip);
    if let Some(code) = roundtrip_error {
        line.insert("error".to_string(), json!(code));
    }
    (Value::Object(line), defs, compact_text)
}

fn decoder_line(case: &Value, catalog: &[Value]) -> Value {
    let defs: Vec<ToolDef> = select(catalog, case.get("tools"))
        .into_iter()
        .map(to_def)
        .collect();
    let chunks = case.get("chunks").and_then(Value::as_array);
    let mut line = Map::new();
    line.insert(
        "id".to_string(),
        case.get("id").cloned().unwrap_or(json!("")),
    );
    line.insert("decoded".to_string(), decode_chunks(chunks, &defs));
    Value::Object(line)
}

fn decode_chunks(chunks: Option<&Vec<Value>>, tools: &[ToolDef]) -> Value {
    let mut decoder = StreamDecoder::new(tools);
    let Some(chunks) = chunks else {
        return json!({"error": "invalid_arguments"});
    };
    for chunk in chunks {
        let Some(text) = chunk.as_str() else {
            return json!({"error": "invalid_arguments"});
        };
        if let Err(err) = decoder.push(text) {
            return json!({"error": err_name(err)});
        }
    }
    match decoder.finish() {
        Ok(calls) => match calls_json(calls) {
            Ok(rows) => json!({"calls": rows}),
            Err(()) => json!({"error": "invalid_arguments"}),
        },
        Err(err) => json!({"error": err_name(err)}),
    }
}

fn roundtrip(rendered: &str, tools: &[ToolDef]) -> (Value, Option<&'static str>) {
    match decode_calls(rendered, tools) {
        Ok(calls) => match calls_json(calls) {
            Ok(rows) => (Value::Array(rows), None),
            Err(()) => (json!([]), Some("invalid_arguments")),
        },
        Err(err) => (json!([]), Some(err_name(err))),
    }
}

fn calls_json(calls: Vec<ToolCall>) -> Result<Vec<Value>, ()> {
    let mut rows = Vec::with_capacity(calls.len());
    for call in calls {
        let arguments: Value = serde_json::from_str(&call.arguments).map_err(|_| ())?;
        rows.push(json!({"name": call.name, "arguments": arguments}));
    }
    Ok(rows)
}

fn render_expected(expected: Option<&Vec<Value>>) -> String {
    let Some(expected) = expected else {
        return String::new();
    };
    expected
        .iter()
        .filter_map(|call| {
            let name = call.get("name")?.as_str()?;
            let args = call.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let args = serde_json::to_string(&args).ok()?;
            let name = if is_bare(name) {
                name.to_string()
            } else {
                serde_json::to_string(name).ok()?
            };
            Some(format!("<<call {name} {args}>>"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn select<'a>(catalog: &'a [Value], names: Option<&Value>) -> Vec<&'a Value> {
    let Some(names) = names.and_then(Value::as_array) else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|name| {
            catalog
                .iter()
                .find(|tool| tool.pointer("/function/name").and_then(Value::as_str) == Some(name))
        })
        .collect()
}

fn to_def(tool: &Value) -> ToolDef {
    let function = tool.get("function").unwrap_or(tool);
    ToolDef {
        name: function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        description: function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        parameters: function
            .get("parameters")
            .filter(|value| !value.is_null())
            .cloned(),
    }
}

fn err_name(err: Error) -> &'static str {
    match err {
        Error::UnknownTool => "unknown_tool",
        Error::InvalidArguments => "invalid_arguments",
        Error::Unsupported | Error::InvalidCompact | Error::Unimplemented => "invalid_arguments",
    }
}

fn is_bare(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {
            chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        }
        _ => false,
    }
}

fn live_url() -> Option<String> {
    let base = nonempty_env("PROVIDER_BASE_URL")?;
    nonempty_env("MODEL")?;
    let base = base.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        Some(base.to_string())
    } else {
        Some(format!("{base}/chat/completions"))
    }
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

async fn live_once(url: &str, request: &Value, tools: &[ToolDef]) -> (String, Value) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
    {
        Ok(client) => client,
        Err(_) => return (String::new(), json!({"error": "request_failed"})),
    };
    let mut req = client.post(url).json(request);
    if let Some(key) = nonempty_env("OPENAI_API_KEY") {
        req = req.bearer_auth(key);
    }
    let response = match req.send().await {
        Ok(response) => response,
        Err(_) => return (String::new(), json!({"error": "request_failed"})),
    };
    // Drop the body. A 401/429/5xx must not be copied into OUT (it can echo a key).
    if !response.status().is_success() {
        return (String::new(), json!({"error": "request_failed"}));
    }
    let Ok(body) = response.json::<Value>().await else {
        return (String::new(), json!({"error": "request_failed"}));
    };
    let Some(raw) = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    else {
        return (String::new(), json!({"error": "request_failed"}));
    };
    let raw = raw.to_string();
    let live_calls = match decode_calls(&raw, tools) {
        Ok(calls) => match calls_json(calls) {
            Ok(rows) => json!({"calls": rows}),
            Err(()) => json!({"error": "invalid_arguments"}),
        },
        Err(err) => json!({"error": err_name(err)}),
    };
    (raw, live_calls)
}

fn insert(line: &mut Value, key: &str, value: Value) {
    if let Some(obj) = line.as_object_mut() {
        obj.insert(key.to_string(), value);
    }
}

fn json_line(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}
