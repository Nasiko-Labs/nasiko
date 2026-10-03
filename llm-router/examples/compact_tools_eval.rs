//! Offline compact-tools evaluation.
//!
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Writes one JSONL line per case. Does not score. Set `COUNT_TOKENS=1` to print
//! `token_reduction=` for the `cases` lines. Live mode is not part of this example.

use std::fs::File;
use std::io::Write;
use std::process::ExitCode;

use nasiko_tool_compact::{CompactError, StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde_json::{Map, Value, json};

const TODAY: &str = "Today is 2026-10-02. The timezone is Asia/Kolkata.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let eval_set = std::env::var("EVAL_SET").map_err(|_| "EVAL_SET is required".to_string())?;
    let out_path = std::env::var("OUT").map_err(|_| "OUT is required".to_string())?;
    let file =
        std::fs::read_to_string(&eval_set).map_err(|err| format!("read {eval_set}: {err}"))?;
    let spec: Value =
        serde_json::from_str(&file).map_err(|err| format!("parse {eval_set}: {err}"))?;
    let catalog = spec
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut out = File::create(&out_path).map_err(|err| format!("create {out_path}: {err}"))?;
    let mut counted = Vec::new();
    if let Some(cases) = spec.get("cases").and_then(Value::as_array) {
        for case in cases {
            let (line, row) = case_line(case, &catalog)?;
            writeln!(out, "{line}").map_err(|err| err.to_string())?;
            counted.push(row);
        }
    }
    if let Some(cases) = spec.get("decoder_cases").and_then(Value::as_array) {
        for case in cases {
            writeln!(out, "{}", decoder_line(case, &catalog)?).map_err(|err| err.to_string())?;
        }
    }
    if std::env::var("COUNT_TOKENS").ok().as_deref() == Some("1") {
        println!("token_reduction={}", token_reduction(&counted)?);
    }
    Ok(())
}

struct Counted {
    compacted: bool,
    compact: Value,
    native: Value,
}

fn case_line(case: &Value, catalog: &[Value]) -> Result<(String, Counted), String> {
    let id = case.get("id").and_then(Value::as_str).unwrap_or("");
    let tools = selected_tools(case, catalog)?;
    let messages = case
        .get("messages")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    let expected = case
        .get("expected")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let native = native_request(&messages, &tools);
    match encode_tools(&tools) {
        Ok(compact) => {
            let rendered = render_calls(&expected);
            let roundtrip = decode_calls(&rendered, &tools).map_err(|err| err.to_string())?;
            let compact_body = compact_request(&compact.text, &messages);
            let line = json!({
                "id": id,
                "compact_request": compact_body,
                "compacted": true,
                "rendered_calls": rendered,
                "roundtrip_calls": roundtrip
                    .into_iter()
                    .map(|call| json!({
                        "name": call.name,
                        "arguments": parse_json(&call.arguments),
                    }))
                    .collect::<Vec<_>>(),
            });
            Ok((
                line.to_string(),
                Counted { compacted: true, compact: compact_body, native },
            ))
        }
        Err(CompactError::UnsupportedSchema { .. }) => {
            let line = json!({
                "id": id,
                "compact_request": native,
                "compacted": false,
                "rendered_calls": "",
                "roundtrip_calls": [],
            });
            Ok((
                line.to_string(),
                Counted { compacted: false, compact: native.clone(), native },
            ))
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Fraction of native `o200k_base` tokens removed. A bypassed case adds nothing
/// to the numerator, even when its two bodies differ.
fn token_reduction(rows: &[Counted]) -> Result<f64, String> {
    let bpe = tiktoken_rs::o200k_base().map_err(|err| err.to_string())?;
    let mut saved = 0i64;
    let mut native_total = 0i64;
    for row in rows {
        let native = bpe.encode_with_special_tokens(&row.native.to_string()).len() as i64;
        let compact = bpe.encode_with_special_tokens(&row.compact.to_string()).len() as i64;
        native_total += native;
        if row.compacted {
            saved += native - compact;
        }
    }
    if native_total == 0 {
        return Ok(0.0);
    }
    Ok(saved as f64 / native_total as f64)
}

fn decoder_line(case: &Value, catalog: &[Value]) -> Result<String, String> {
    let id = case.get("id").and_then(Value::as_str).unwrap_or("");
    let tools = selected_tools(case, catalog)?;
    let chunks = case
        .get("chunks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let chunk_text: Vec<String> = chunks
        .iter()
        .map(|chunk| chunk.as_str().unwrap_or("").to_string())
        .collect();
    let pieces = match case.get("expected").and_then(|expected| expected.get("calls")) {
        Some(Value::Array(calls)) => split_like(&render_calls(calls), &chunk_text),
        _ => chunk_text,
    };
    Ok(json!({"id": id, "decoded": decode_chunks(&pieces, &tools)}).to_string())
}

fn decode_chunks(pieces: &[String], tools: &[ToolDef]) -> Value {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();
    for piece in pieces {
        match decoder.push(piece) {
            Ok(found) => calls.extend(found),
            Err(err) => return error_value(&err),
        }
    }
    if let Err(err) = decoder.finish() {
        return error_value(&err);
    }
    json!({
        "calls": calls
            .into_iter()
            .map(|call| json!({
                "name": call.name,
                "arguments": parse_json(&call.arguments),
            }))
            .collect::<Vec<_>>()
    })
}

fn error_value(err: &CompactError) -> Value {
    let code = match err {
        CompactError::UnknownTool { .. } => "unknown_tool",
        CompactError::InvalidArguments { .. } | CompactError::UnsupportedSchema { .. } => {
            "invalid_arguments"
        }
    };
    json!({"error": code})
}

fn compact_request(compact_text: &str, messages: &Value) -> Value {
    let mut body = Vec::new();
    body.push(json!({
        "role": "system",
        "content": format!("{TODAY}\n{compact_text}"),
    }));
    if let Some(items) = messages.as_array() {
        body.extend(items.iter().cloned());
    }
    json!({"messages": body})
}

fn native_request(messages: &Value, tools: &[ToolDef]) -> Value {
    json!({
        "messages": messages,
        "tools": tools.iter().map(native_tool).collect::<Vec<_>>(),
    })
}

fn native_tool(tool: &ToolDef) -> Value {
    let mut function = Map::new();
    function.insert("name".into(), Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        function.insert("description".into(), Value::String(description.clone()));
    }
    if let Some(parameters) = &tool.parameters {
        function.insert("parameters".into(), parameters.clone());
    }
    json!({"type": "function", "function": function})
}

fn selected_tools(case: &Value, catalog: &[Value]) -> Result<Vec<ToolDef>, String> {
    let names = case.get("tools").and_then(Value::as_array);
    let mut tools = Vec::new();
    for entry in catalog {
        let tool = to_tool(entry);
        let wanted = names.is_none_or(|names| {
            names
                .iter()
                .any(|name| name.as_str() == Some(tool.name.as_str()))
        });
        if wanted {
            tools.push(tool);
        }
    }
    Ok(tools)
}

fn to_tool(value: &Value) -> ToolDef {
    let function = value.get("function").unwrap_or(value);
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
        parameters: function.get("parameters").cloned(),
    }
}

fn render_calls(calls: &[Value]) -> String {
    let mut rendered = String::new();
    for call in calls {
        let name = call.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = match call.get("arguments") {
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => "{}".to_string(),
        };
        rendered.push_str(&format!("<<call {name} {arguments}>>"));
    }
    rendered
}

fn split_like(rendered: &str, chunks: &[String]) -> Vec<String> {
    let published: usize = chunks.iter().map(|chunk| chunk.chars().count()).sum();
    let chars: Vec<char> = rendered.chars().collect();
    if published == 0 || chunks.is_empty() {
        return vec![rendered.to_string()];
    }
    let mut bounds = vec![0usize];
    let mut covered = 0usize;
    for (index, chunk) in chunks.iter().enumerate() {
        covered += chunk.chars().count();
        let at = if index + 1 == chunks.len() {
            chars.len()
        } else {
            ((covered as f64) * (chars.len() as f64) / (published as f64)).round() as usize
        };
        let prev = bounds.last().copied().unwrap_or(0);
        bounds.push(at.max(prev).min(chars.len()));
    }
    bounds
        .windows(2)
        .map(|pair| chars[pair[0]..pair[1]].iter().collect())
        .collect()
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bypassed_case_adds_nothing_to_the_numerator() {
        let native = json!({
            "messages": [{"role": "user", "content": "book it"}],
            "tools": [{"type": "function", "function": {"name": "create_calendar_event", "parameters": {"type": "object"}}}]
        });
        let compact = json!({
            "messages": [{"role": "system", "content": "create_calendar_event(title:str)"}]
        });
        let compacted = Counted {
            compacted: true,
            compact: compact.clone(),
            native: native.clone(),
        };
        let bypass = Counted {
            compacted: false,
            compact: json!({"short": true}),
            native: json!({
                "tools": [{"description": "x".repeat(4000), "name": "lookup", "parameters": {"$ref": "#/$defs/Id"}}]
            }),
        };
        let only = token_reduction(std::slice::from_ref(&compacted)).expect("tokens");
        let with_bypass = token_reduction(&[compacted, bypass]).expect("tokens");
        assert!(only >= 0.30, "compacted case should clear the bar, got {only}");
        assert!(
            with_bypass < only,
            "a bypassed case must not increase savings: {with_bypass} vs {only}"
        );
    }
}
