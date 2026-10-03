//! Offline JSONL harness for Compact Tools Track P1 evaluation sets.
//!
//! ```sh
//! EVAL_SET=path/to/eval.json OUT=path/to/results.jsonl \
//!   cargo run -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! The adapter functions at the end of this file are intentionally temporary.
//! Their signatures use the public `nasiko_tool_compact` types so replacing their
//! bodies after Checkpoint 2 does not alter evaluation I/O or JSONL output.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use nasiko_tool_compact::{
    CompactError, CompactTools, Result as CompactResult, StreamEvent, ToolCall, ToolDef,
    render_call,
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
    let live_model = configured_live_model();
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
        let record = eval_record(case, index, live_model.as_deref())?;
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

/// `MODEL` is used only when the live provider endpoint is configured. Offline
/// evaluations preserve a dataset model when present and otherwise omit it.
fn configured_live_model() -> Option<String> {
    env::var("PROVIDER_BASE_URL")
        .ok()
        .filter(|base_url| !base_url.is_empty())
        .and_then(|_| env::var("MODEL").ok())
        .filter(|model| !model.is_empty())
}

/// Accept a JSON array, an object containing a conventional case array, or JSONL.
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

fn eval_record(case: &Value, index: usize, live_model: Option<&str>) -> AppResult<Value> {
    let object = case
        .as_object()
        .ok_or_else(|| format!("case {index} must be a JSON object"))?;
    let id = case_value(object, &["id", "case_id", "name"])
        .cloned()
        .ok_or_else(|| format!("case {index} is missing id"))?;

    if is_decoder_case(object) {
        return decoder_record(id, object);
    }
    normal_record(id, object, live_model)
}

/// Normal cases have exactly the Track P1 organizer fields.
fn normal_record(
    id: Value,
    case: &Map<String, Value>,
    live_model: Option<&str>,
) -> AppResult<Value> {
    let native_request = native_openai_request(case, live_model);
    let tools = tool_defs_from_request(&native_request).map_err(compact_error_message)?;
    let calls = expected_calls(case).map_err(compact_error_message)?;
    let rendered_calls = calls.iter().map(render_call).collect::<Vec<_>>().join("\n");

    let (compact_request, compacted) = match compact_tools_adapter(&tools) {
        Ok(compact) if tool_choice_allows_compaction(&native_request) => {
            (post_compaction_request(&native_request, &compact), true)
        }
        Ok(_) | Err(_) => (native_request, false),
    };
    let roundtrip_calls = decode_calls_adapter(&rendered_calls, &tools)
        .map_err(compact_error_message)
        .map(tool_calls_json)?;

    Ok(json!({
        "id": id,
        "compact_request": compact_request,
        "compacted": compacted,
        "rendered_calls": rendered_calls,
        "roundtrip_calls": roundtrip_calls,
    }))
}

/// Decoder cases do not run normal-case compaction or expected-call round trips.
fn decoder_record(id: Value, case: &Map<String, Value>) -> AppResult<Value> {
    let native_request = native_openai_request(case, None);
    let tools = tool_defs_from_request(&native_request).map_err(compact_error_message)?;
    let chunks = decoder_chunks(case)?;
    let decoded = match decode_chunks_adapter(&chunks, &tools) {
        Ok(calls) => json!({ "calls": tool_calls_json(calls) }),
        Err(error) => json!({ "error": decoder_error_code(&error) }),
    };
    Ok(json!({ "id": id, "decoded": decoded }))
}

/// Build the native baseline request. It is only emitted unchanged on a failed
/// compaction; successful normal cases pass it through `post_compaction_request`.
fn native_openai_request(case: &Map<String, Value>, live_model: Option<&str>) -> Value {
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
        if let Some(model) = live_model {
            request.insert("model".into(), Value::String(model.into()));
        }
    }
    if !request.contains_key("tools") {
        if let Some(tools) = case_value(case, &["tools", "tool_definitions"]) {
            request.insert("tools".into(), tools.clone());
        }
    }
    request.entry("messages").or_insert_with(|| json!([]));
    Value::Object(request)
}

/// The post-compaction request preserves every original message and prepends the
/// compact definitions/instructions. Native tool controls are no longer valid.
fn post_compaction_request(native_request: &Value, compact: &CompactTools) -> Value {
    let mut request = native_request.as_object().cloned().unwrap_or_default();
    request.remove("tools");
    request.remove("tool_choice");
    request.remove("parallel_tool_calls");

    let mut messages = request
        .remove("messages")
        .and_then(|messages| messages.as_array().cloned())
        .unwrap_or_default();
    messages.insert(
        0,
        json!({
            "role": "system",
            "content": compact.render(),
        }),
    );
    request.insert("messages".into(), Value::Array(messages));
    Value::Object(request)
}

/// Compact mode can preserve OpenAI's default `auto` behavior, but it cannot
/// faithfully carry a forced tool selector or other native tool-choice control.
fn tool_choice_allows_compaction(request: &Value) -> bool {
    match request.get("tool_choice") {
        None => true,
        Some(Value::String(choice)) => choice == "auto",
        _ => false,
    }
}

fn tool_defs_from_request(request: &Value) -> CompactResult<Vec<ToolDef>> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| tools.iter().map(native_tool_def).collect())
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn native_tool_def(tool: &Value) -> CompactResult<ToolDef> {
    let function = tool.get("function").unwrap_or(tool);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CompactError::InvalidSchema {
            tool: "<unknown>".into(),
            reason: "function tool is missing a name".into(),
        })?;
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "function")
    {
        return Err(CompactError::UnsupportedSchema {
            tool: name.into(),
            reason: "only function tools can be compacted".into(),
        });
    }
    Ok(ToolDef {
        name: name.into(),
        description: function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: function.get("parameters").cloned(),
    })
}

fn expected_calls(case: &Map<String, Value>) -> CompactResult<Vec<ToolCall>> {
    let values = case_value(case, &["expected_calls", "calls", "tool_calls"])
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            case.get("expected")
                .and_then(Value::as_object)
                .and_then(|expected| case_value(expected, &["calls", "tool_calls"]))
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default();
    values.iter().map(native_tool_call).collect()
}

fn native_tool_call(call: &Value) -> CompactResult<ToolCall> {
    let function = call.get("function").unwrap_or(call);
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CompactError::InvalidArguments {
            tool: "<unknown>".into(),
            reason: "tool call is missing a name".into(),
        })?;
    let arguments = function
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let arguments = match arguments {
        Value::String(arguments) => {
            serde_json::from_str(&arguments).map_err(|error| CompactError::InvalidArguments {
                tool: name.into(),
                reason: error.to_string(),
            })?
        }
        value => value,
    };
    if !arguments.is_object() {
        return Err(CompactError::InvalidArguments {
            tool: name.into(),
            reason: "arguments must be a JSON object".into(),
        });
    }
    Ok(ToolCall {
        name: name.into(),
        arguments,
    })
}

fn decoder_chunks(case: &Map<String, Value>) -> AppResult<Vec<String>> {
    case_value(case, &["chunks", "stream_chunks"])
        .and_then(Value::as_array)
        .ok_or_else(|| "decoder case is missing ordered chunks".into())?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("decoder chunk {index} must be a string"))
        })
        .collect()
}

fn is_decoder_case(case: &Map<String, Value>) -> bool {
    ["case_type", "kind", "mode", "type"]
        .iter()
        .filter_map(|key| case.get(*key).and_then(Value::as_str))
        .any(|value| matches!(value.to_ascii_lowercase().as_str(), "decode" | "decoder"))
        || case.contains_key("chunks")
        || case.contains_key("stream_chunks")
}

fn case_value<'a>(case: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| case.get(*key))
}

fn tool_calls_json(calls: Vec<ToolCall>) -> Value {
    serde_json::to_value(calls).expect("ToolCall always serializes")
}

fn compact_error_message(error: CompactError) -> String {
    format!("{}: {error}", error.code())
}

fn decoder_error_code(error: &CompactError) -> &'static str {
    match error {
        CompactError::UnknownTool(_) => "unknown_tool",
        _ => "invalid_arguments",
    }
}

// --- Temporary A/B adapters. Replace bodies after Checkpoint 2. ---

/// Same result type as `nasiko_tool_compact::encode_tools`.
fn compact_tools_adapter(tools: &[ToolDef]) -> CompactResult<CompactTools> {
    let definitions =
        serde_json::to_string(tools).map_err(|error| CompactError::InvalidSchema {
            tool: "<tools>".into(),
            reason: error.to_string(),
        })?;
    Ok(CompactTools {
        definitions,
        instructions: "Emit each call as <<call NAME JSON_OBJECT>>.".into(),
    })
}

/// Same result type as `nasiko_tool_compact::decode_calls`.
fn decode_calls_adapter(text: &str, tools: &[ToolDef]) -> CompactResult<Vec<ToolCall>> {
    let mut decoder = FixtureStreamDecoder::new(tools);
    decoder.push(text)?;
    decoder.finish()
}

/// Drop-in-shaped temporary replacement for `StreamDecoder`.
struct FixtureStreamDecoder {
    tools: Vec<ToolDef>,
    pending: String,
    calls: Vec<ToolCall>,
}

impl FixtureStreamDecoder {
    fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            pending: String::new(),
            calls: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &str) -> CompactResult<Vec<StreamEvent>> {
        self.pending.push_str(chunk);
        let mut events = Vec::new();
        loop {
            let Some(start) = self.pending.find("<<call ") else {
                break;
            };
            let Some(end) = self.pending[start + 7..].find(">>") else {
                break;
            };
            let end = start + 7 + end;
            let body = &self.pending[start + 7..end];
            let (name, arguments) = body.split_once(char::is_whitespace).ok_or_else(|| {
                CompactError::InvalidArguments {
                    tool: "<unknown>".into(),
                    reason: "call is missing JSON arguments".into(),
                }
            })?;
            let arguments = serde_json::from_str(arguments.trim()).map_err(|error| {
                CompactError::InvalidArguments {
                    tool: name.into(),
                    reason: error.to_string(),
                }
            })?;
            if !arguments.is_object() {
                return Err(CompactError::InvalidArguments {
                    tool: name.into(),
                    reason: "arguments must be a JSON object".into(),
                });
            }
            if !self.tools.iter().any(|tool| tool.name == name) {
                return Err(CompactError::UnknownTool(name.into()));
            }
            let call = ToolCall {
                name: name.into(),
                arguments,
            };
            self.calls.push(call.clone());
            events.push(StreamEvent::Call(call));
            self.pending.drain(..end + 2);
        }
        Ok(events)
    }

    fn finish(self) -> CompactResult<Vec<ToolCall>> {
        if self.pending.contains("<<call ") {
            Err(CompactError::IncompleteStream)
        } else {
            Ok(self.calls)
        }
    }
}

fn decode_chunks_adapter(chunks: &[String], tools: &[ToolDef]) -> CompactResult<Vec<ToolCall>> {
    let mut decoder = FixtureStreamDecoder::new(tools);
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish()
}

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

    #[test]
    fn successful_compaction_removes_native_tools_and_injects_instructions() {
        let native = json!({
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "lookup"}}],
            "tool_choice": "auto"
        });
        let compact = compact_tools_adapter(&tool_defs_from_request(&native).unwrap()).unwrap();
        let request = post_compaction_request(&native, &compact);
        assert!(request.get("tools").is_none());
        assert!(request.get("tool_choice").is_none());
        assert_eq!(request["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn stream_adapter_decodes_calls_across_ordered_chunks() {
        let tools = vec![ToolDef {
            name: "lookup".into(),
            description: None,
            parameters: None,
        }];
        let calls =
            decode_chunks_adapter(&["<<call look".into(), "up {\"q\":\"x\"}>>".into()], &tools)
                .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup");
    }
}
