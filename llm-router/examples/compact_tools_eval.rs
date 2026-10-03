//! Offline JSONL harness for compact-tools evaluation sets.
//!
//! ```sh
//! EVAL_SET=path/to/eval.json OUT=path/to/results.jsonl \
//!   cargo run -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! The compact-tools encoder/decoder are intentionally represented by the small
//! fixture adapters below until their implementations land.  Keeping that seam
//! local makes the replacements mechanical: `fixture_compact`,
//! `fixture_render_calls`, and `fixture_decode` can each be swapped for the
//! corresponding `nasiko_tool_compact` call without changing dataset I/O.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use serde_json::{Map, Value, json};

type AppResult<T> = Result<T, String>;

fn main() {
    if let Err(error) = run() {
        eprintln!("compact_tools_eval: {error}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let eval_set = env::var("EVAL_SET").map_err(|_| "EVAL_SET must name an evaluation dataset")?;
    let out = env::var("OUT").map_err(|_| "OUT must name the JSONL output file")?;
    let cases = load_cases(Path::new(&eval_set))?;

    let out_path = Path::new(&out);
    if let Some(parent) = out_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create output directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    let file = File::create(out_path)
        .map_err(|error| format!("could not create '{}': {error}", out_path.display()))?;
    let mut writer = BufWriter::new(file);

    for (index, case) in cases.iter().enumerate() {
        let record = eval_record(case, index)?;
        serde_json::to_writer(&mut writer, &canonicalize(&record))
            .map_err(|error| format!("could not serialize case {index}: {error}"))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("could not write case {index}: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("could not flush '{}': {error}", out_path.display()))
}

/// Accept a JSON array, an object containing a conventional case array, or JSONL.
/// The cases themselves remain untyped so new evaluation fields pass through.
fn load_cases(path: &Path) -> AppResult<Vec<Value>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return cases_from_value(value);
    }

    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line_number, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "could not parse JSONL case at line {} in '{}': {error}",
                    line_number + 1,
                    path.display()
                )
            })
        })
        .collect()
}

fn cases_from_value(value: Value) -> AppResult<Vec<Value>> {
    match value {
        Value::Array(cases) => Ok(cases),
        Value::Object(mut document) => {
            for key in ["cases", "evaluations", "data", "items"] {
                if let Some(Value::Array(cases)) = document.remove(key) {
                    return Ok(cases);
                }
            }
            Ok(vec![Value::Object(document)])
        }
        _ => Err("evaluation dataset must be a JSON array, object, or JSONL objects".into()),
    }
}

fn eval_record(case: &Value, index: usize) -> AppResult<Value> {
    let object = case
        .as_object()
        .ok_or_else(|| format!("case {index} must be a JSON object"))?;
    let id = case_value(object, &["id", "case_id", "name"])
        .cloned()
        .ok_or_else(|| format!("case {index} is missing id"))?;
    let compact_request = native_openai_request(object);
    let calls = source_calls(object);

    let mut record = Map::new();
    record.insert("id".into(), id);
    record.insert("compact_request".into(), compact_request.clone());
    record.insert("compacted".into(), fixture_compact(&compact_request));
    record.insert("rendered_calls".into(), fixture_render_calls(&calls));
    record.insert("roundtrip_calls".into(), fixture_roundtrip_calls(&calls));

    if is_decoder_case(object) {
        record.insert("decoded".into(), fixture_decode(object));
    }
    Ok(Value::Object(record))
}

/// Make an OpenAI Chat Completions-shaped baseline request without making a network call.
fn native_openai_request(case: &Map<String, Value>) -> Value {
    let mut request = case_value(case, &["openai_request", "request", "baseline_request"])
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            let mut request = Map::new();
            for key in [
                "model",
                "messages",
                "tools",
                "tool_choice",
                "temperature",
                "max_tokens",
                "stream",
                "top_p",
                "frequency_penalty",
                "presence_penalty",
                "parallel_tool_calls",
                "response_format",
                "seed",
            ] {
                if let Some(value) = case.get(key) {
                    request.insert(key.into(), value.clone());
                }
            }
            request
        });

    if !request.contains_key("model") {
        request.insert("model".into(), Value::String("compact-tools-eval".into()));
    }
    if !request.contains_key("messages") {
        request.insert("messages".into(), Value::Array(Vec::new()));
    }
    if !request.contains_key("tools") {
        if let Some(tools) = case_value(case, &["tools", "tool_definitions"]) {
            request.insert("tools".into(), tools.clone());
        }
    }
    Value::Object(request)
}

fn source_calls(case: &Map<String, Value>) -> Vec<Value> {
    case_value(case, &["expected_calls", "calls", "tool_calls"])
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            case.get("expected")
                .and_then(Value::as_object)
                .and_then(|expected| case_value(expected, &["calls", "tool_calls"]))
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
}

fn is_decoder_case(case: &Map<String, Value>) -> bool {
    ["case_type", "kind", "mode", "type"]
        .iter()
        .filter_map(|key| case.get(*key).and_then(Value::as_str))
        .any(|value| matches!(value.to_ascii_lowercase().as_str(), "decode" | "decoder"))
        || case.contains_key("decode_input")
        || case.contains_key("compact_output")
}

fn case_value<'a>(case: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| case.get(*key))
}

// --- Temporary adapters: replace these after compact-tools Checkpoint 2. ---

fn fixture_compact(request: &Value) -> Value {
    let tools = request.get("tools").cloned().unwrap_or_else(|| json!([]));
    json!({
        "adapter": "fixture",
        "definitions": canonical_json(&tools),
        "instructions": "Offline fixture; replace with nasiko_tool_compact::encode_tools after Checkpoint 2."
    })
}

fn fixture_render_calls(calls: &[Value]) -> Value {
    Value::Array(calls.iter().map(render_call_fixture).collect())
}

fn render_call_fixture(call: &Value) -> Value {
    let name = call
        .get("function")
        .and_then(Value::as_object)
        .and_then(|function| function.get("name"))
        .or_else(|| call.get("name"))
        .and_then(Value::as_str);
    let arguments = call
        .get("function")
        .and_then(Value::as_object)
        .and_then(|function| function.get("arguments"))
        .or_else(|| call.get("arguments"));

    match (name, arguments) {
        (Some(name), Some(arguments)) => Value::String(format!(
            "<<call {name} {}>>",
            canonical_json(&arguments_as_value(arguments))
        )),
        _ => canonicalize(call),
    }
}

fn arguments_as_value(arguments: &Value) -> Value {
    match arguments {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| arguments.clone()),
        _ => arguments.clone(),
    }
}

fn fixture_roundtrip_calls(calls: &[Value]) -> Value {
    Value::Array(calls.iter().map(canonicalize).collect())
}

fn fixture_decode(case: &Map<String, Value>) -> Value {
    json!({
        "adapter": "fixture",
        "input": case_value(case, &["decode_input", "compact_output", "model_output", "output"])
            .cloned()
            .unwrap_or(Value::Null),
        "calls": []
    })
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&canonicalize(value)).expect("JSON values always serialize")
}

/// Sort object keys recursively so output does not depend on source map insertion order.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_a_native_request_from_case_fields() {
        let case = json!({
            "messages": [],
            "tools": []
        });
        let request = native_openai_request(case.as_object().unwrap());
        assert_eq!(request["model"], "compact-tools-eval");
        assert_eq!(request["messages"], json!([]));
        assert_eq!(request["tools"], json!([]));
    }

    #[test]
    fn recognizes_decoder_shape_without_a_fixture_answer() {
        let case = json!({"type": "decoder"});
        assert!(is_decoder_case(case.as_object().unwrap()));
        assert_eq!(
            fixture_decode(case.as_object().unwrap())["input"],
            Value::Null
        );
    }
}
