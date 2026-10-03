//! Offline evaluation for compact tool schemas.
//!
//! Usage:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!     cargo run --release -p nasiko-llm-router --example compact_tools_eval
//!
//! Writes one JSONL line per case and per decoder case. Reports outputs only;
//! no scores. Deterministic: no clocks, no randomness, no network.
//! Live mode (PROVIDER_BASE_URL / MODEL) is not implemented in this version.

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::process::ExitCode;

use nasiko_tool_compact::{
    DecodeError, DecodeEvent, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
    render_call,
};
use serde_json::{Value, json};

const REFERENCE_TIME: &str = "Today is 2026-10-02 (Friday). Timezone: Asia/Kolkata (UTC+05:30). Resolve relative dates from this.";

/// Tool definitions as the eval file ships them: native JSON plus the parsed form.
struct Catalog(BTreeMap<String, (ToolDef, Value)>);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("compact_tools_eval: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let eval_path = env::var("EVAL_SET").map_err(|_| "EVAL_SET is required")?;
    let out_path = env::var("OUT").map_err(|_| "OUT is required")?;
    let set: Value = serde_json::from_str(&fs::read_to_string(eval_path)?)?;
    let catalog = parse_catalog(&set)?;

    let mut out = String::new();
    for case in array(&set, "cases") {
        out.push_str(&serde_json::to_string(&eval_case(case, &catalog))?);
        out.push('\n');
    }
    for case in array(&set, "decoder_cases") {
        out.push_str(&serde_json::to_string(&eval_decoder_case(case, &catalog))?);
        out.push('\n');
    }
    fs::write(out_path, out)?;
    Ok(())
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn tool_def(v: &Value) -> Option<ToolDef> {
    let f = v.get("function").unwrap_or(v);
    Some(ToolDef {
        name: f.get("name")?.as_str()?.to_string(),
        description: f
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        parameters: f.get("parameters").cloned(),
    })
}

fn parse_catalog(set: &Value) -> Result<Catalog, Box<dyn Error>> {
    let mut map = BTreeMap::new();
    for raw in array(set, "tools") {
        let def = tool_def(raw).ok_or("tool entry without a name")?;
        map.insert(def.name.clone(), (def, native_tool(raw)));
    }
    Ok(Catalog(map))
}

/// Native OpenAI-shaped tool entry (wraps bare definitions).
fn native_tool(raw: &Value) -> Value {
    if raw.get("function").is_some() {
        raw.clone()
    } else {
        json!({"type": "function", "function": raw})
    }
}

/// Resolve a case's `tools` (names, or inline definitions) against the catalog.
fn select(case: &Value, catalog: &Catalog) -> Vec<(ToolDef, Value)> {
    array(case, "tools")
        .iter()
        .filter_map(|t| match t.as_str() {
            Some(name) => catalog.0.get(name).cloned(),
            None => tool_def(t).map(|d| (d, native_tool(t))),
        })
        .collect()
}

fn error_code(err: &DecodeError) -> &'static str {
    match err {
        DecodeError::UnknownTool(_) => "unknown_tool",
        _ => "invalid_arguments",
    }
}

fn call_json(call: &ToolCall) -> Value {
    json!({"name": call.name, "arguments": call.arguments})
}

fn eval_case(case: &Value, catalog: &Catalog) -> Value {
    let id = case.get("id").cloned().unwrap_or(Value::Null);
    let selected = select(case, catalog);
    let defs: Vec<ToolDef> = selected.iter().map(|(d, _)| d.clone()).collect();
    let natives: Vec<Value> = selected.iter().map(|(_, n)| n.clone()).collect();
    let messages = array(case, "messages").to_vec();
    let expected: Vec<ToolCall> = array(case, "expected")
        .iter()
        .filter_map(|e| {
            Some(ToolCall {
                name: e.get("name")?.as_str()?.to_string(),
                arguments: e.get("arguments").cloned().unwrap_or_else(|| json!({})),
            })
        })
        .collect();

    let compact = if defs.is_empty() {
        None
    } else {
        encode_tools(&defs).ok()
    };
    let Some(compact) = compact else {
        // Bypass: send the native request unchanged (apart from the reference time).
        let mut msgs = vec![json!({"role": "system", "content": REFERENCE_TIME})];
        msgs.extend(messages);
        let mut request = json!({"messages": msgs, "temperature": 0});
        if !natives.is_empty() {
            request["tools"] = Value::Array(natives);
        }
        return json!({
            "id": id,
            "compact_request": request,
            "compacted": false,
            "rendered_calls": null,
            "roundtrip_calls": null
        });
    };

    let system = format!("{REFERENCE_TIME}\n\n{}", compact.system_prompt());
    let mut msgs = vec![json!({"role": "system", "content": system})];
    msgs.extend(messages);

    let rendered = expected
        .iter()
        .map(render_call)
        .collect::<Vec<_>>()
        .join("\n");
    let roundtrip = match decode_calls(&rendered, &defs) {
        Ok(calls) => Value::Array(calls.iter().map(call_json).collect()),
        Err(err) => json!({"error": error_code(&err)}),
    };
    json!({
        "id": id,
        "compact_request": {"messages": msgs, "temperature": 0},
        "compacted": true,
        "rendered_calls": rendered,
        "roundtrip_calls": roundtrip
    })
}

fn eval_decoder_case(case: &Value, catalog: &Catalog) -> Value {
    let id = case.get("id").cloned().unwrap_or(Value::Null);
    let defs: Vec<ToolDef> = select(case, catalog).into_iter().map(|(d, _)| d).collect();
    let mut decoder = StreamDecoder::new(&defs);
    let mut calls = Vec::new();
    let mut failure: Option<DecodeError> = None;

    let mut absorb = |result: Result<Vec<DecodeEvent>, DecodeError>| match result {
        Ok(events) => {
            for event in events {
                if let DecodeEvent::Call(call) = event {
                    calls.push(call_json(&call));
                }
            }
            true
        }
        Err(err) => {
            failure = Some(err);
            false
        }
    };

    let mut ok = true;
    for chunk in array(case, "chunks").iter().filter_map(Value::as_str) {
        if !absorb(decoder.push(chunk)) {
            ok = false;
            break;
        }
    }
    if ok {
        absorb(decoder.finish());
    }

    let decoded = match failure {
        Some(err) => json!({"error": error_code(&err)}),
        None => json!({"calls": calls}),
    };
    json!({"id": id, "decoded": decoded})
}
