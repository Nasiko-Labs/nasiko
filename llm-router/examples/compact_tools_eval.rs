//! Offline JSONL harness for Compact Tools Track P1 evaluation sets.
//!
//! ```sh
//! EVAL_SET=path/to/eval.json OUT=path/to/results.jsonl \
//!   cargo run -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! It calls the public `nasiko_tool_compact` encoder and decoders directly.

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use nasiko_tool_compact::{
    CompactError, CompactTools, Result as CompactResult, StreamDecoder, ToolCall, ToolDef,
    decode_calls, encode_tools, render_call,
};
use serde_json::{Map, Value, json};
use tiktoken_rs::{CoreBPE, o200k_base};

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
    let measure_out = env::var("MEASURE_OUT").ok();
    let tokenizer = measure_out
        .as_ref()
        .map(|_| o200k_base().map_err(|error| format!("could not load o200k_base: {error}")))
        .transpose()?;

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
    let mut measurements = Vec::new();

    for (index, case) in cases.iter().enumerate() {
        let (record, measurement) =
            eval_record(case, index, live_model.as_deref(), tokenizer.as_ref())?;
        let line = canonical_json_line(&record)
            .map_err(|error| format!("could not serialize case {index}: {error}"))?;
        writer
            .write_all(&line)
            .map_err(|error| format!("could not write case {index}: {error}"))?;
        if let Some(measurement) = measurement {
            measurements.push(measurement);
        }
    }
    writer
        .flush()
        .map_err(|error| format!("could not flush '{}': {error}", out_path.display()))?;

    if let Some(measure_out) = measure_out {
        write_measurements(Path::new(&measure_out), &measurements)?;
    }
    Ok(())
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

/// Local-only measurement output. These values are deliberately kept out of the
/// organizer-facing `OUT` file, whose scoring is recomputed by the organizers.
#[derive(serde::Serialize)]
struct TokenMeasurement {
    id: Value,
    native_tokens: usize,
    compact_tokens: usize,
    token_reduction: f64,
}

impl TokenMeasurement {
    fn from_requests(
        id: &Value,
        native_request: &Value,
        compact_request: &Value,
        tokenizer: &CoreBPE,
    ) -> AppResult<Self> {
        let native_tokens = request_token_count(native_request, tokenizer)?;
        let compact_tokens = request_token_count(compact_request, tokenizer)?;
        let token_reduction = if native_tokens == 0 {
            0.0
        } else {
            1.0 - compact_tokens as f64 / native_tokens as f64
        };
        Ok(Self {
            id: id.clone(),
            native_tokens,
            compact_tokens,
            token_reduction,
        })
    }
}

/// Counts the complete canonicalized JSON request body with the o200k_base BPE.
fn request_token_count(request: &Value, tokenizer: &CoreBPE) -> AppResult<usize> {
    let body = serde_json::to_string(&canonicalize(request))
        .map_err(|error| format!("could not serialize request for token counting: {error}"))?;
    Ok(tokenizer.encode_with_special_tokens(&body).len())
}

fn write_measurements(path: &Path, measurements: &[TokenMeasurement]) -> AppResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create measurement directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    let file = File::create(path).map_err(|error| {
        format!(
            "could not create measurement report '{}': {error}",
            path.display()
        )
    })?;
    let mut writer = BufWriter::new(file);
    for measurement in measurements {
        let value = serde_json::to_value(measurement)
            .map_err(|error| format!("could not serialize measurement: {error}"))?;
        let line = canonical_json_line(&value)?;
        writer.write_all(&line).map_err(|error| {
            format!(
                "could not write measurement report '{}': {error}",
                path.display()
            )
        })?;
    }
    writer.flush().map_err(|error| {
        format!(
            "could not flush measurement report '{}': {error}",
            path.display()
        )
    })
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
            if document.contains_key("tools")
                && (document.contains_key("cases") || document.contains_key("decoder_cases"))
            {
                return official_cases(&mut document);
            }
            let mut tool_registry = std::collections::HashMap::new();
            if let Some(Value::Array(tools)) = document.get("tools") {
                for tool in tools {
                    let function = tool.get("function").unwrap_or(tool);
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        tool_registry.insert(name.to_string(), tool.clone());
                    }
                }
            }

            let mut all_cases = Vec::new();
            let mut found = false;
            for key in ["cases", "decoder_cases", "evaluations", "data", "items"] {
                if let Some(Value::Array(cases)) = document.remove(key) {
                    found = true;
                    all_cases.extend(cases);
                }
            }

            if !found {
                all_cases.push(Value::Object(document));
            }

            if !tool_registry.is_empty() {
                for case in &mut all_cases {
                    if let Value::Object(case_obj) = case
                        && let Some(Value::Array(tools)) = case_obj.get_mut("tools")
                    {
                        for tool_val in tools.iter_mut() {
                            if let Value::String(tool_name) = tool_val
                                && let Some(resolved) = tool_registry.get(tool_name)
                            {
                                *tool_val = resolved.clone();
                            }
                        }
                    }
                }
            }

            Ok(all_cases)
        }
        _ => Err("evaluation dataset must be a JSON array, object, or JSONL objects".into()),
    }
}

/// Resolve official public-eval tool-name references through its top-level full
/// OpenAI tool catalog before normal/decoder case processing starts.
fn official_cases(document: &mut Map<String, Value>) -> AppResult<Vec<Value>> {
    let catalog = tool_catalog(
        document
            .remove("tools")
            .and_then(|tools| tools.as_array().cloned())
            .ok_or_else(|| "official eval tools must be an array".to_string())?,
    )?;
    let normal_cases = match document.remove("cases") {
        Some(Value::Array(cases)) => cases,
        Some(_) => return Err("official eval cases must be an array".into()),
        None => Vec::new(),
    };
    let decoder_cases = match document.remove("decoder_cases") {
        Some(Value::Array(cases)) => cases,
        Some(_) => return Err("official eval decoder_cases must be an array".into()),
        None => Vec::new(),
    };

    normal_cases
        .into_iter()
        .chain(decoder_cases)
        .map(|case| resolve_tool_references(case, &catalog))
        .collect()
}

fn tool_catalog(tools: Vec<Value>) -> AppResult<BTreeMap<String, Value>> {
    let mut catalog = BTreeMap::new();
    for tool in tools {
        let name = native_tool_def(&tool).map_err(compact_error_message)?.name;
        if catalog.insert(name.clone(), tool).is_some() {
            return Err(format!(
                "official eval tool catalog contains duplicate tool '{name}'"
            ));
        }
    }
    Ok(catalog)
}

fn resolve_tool_references(case: Value, catalog: &BTreeMap<String, Value>) -> AppResult<Value> {
    let mut case = case
        .as_object()
        .cloned()
        .ok_or_else(|| "official eval case must be an object".to_string())?;
    let references = case
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "official eval case tools must be an array of tool names".to_string())?;
    let resolved = references
        .iter()
        .map(|reference| {
            let name = reference
                .as_str()
                .ok_or_else(|| "official eval tool references must be strings".to_string())?;
            catalog
                .get(name)
                .cloned()
                .ok_or_else(|| format!("official eval references unknown tool '{name}'"))
        })
        .collect::<AppResult<Vec<_>>>()?;
    case.insert("tools".into(), Value::Array(resolved));
    Ok(Value::Object(case))
}

fn eval_record(
    case: &Value,
    index: usize,
    live_model: Option<&str>,
    tokenizer: Option<&CoreBPE>,
) -> AppResult<(Value, Option<TokenMeasurement>)> {
    let object = case
        .as_object()
        .ok_or_else(|| format!("case {index} must be a JSON object"))?;
    let id = case_value(object, &["id", "case_id", "name"])
        .cloned()
        .ok_or_else(|| format!("case {index} is missing id"))?;

    if is_decoder_case(object) {
        return decoder_record(id, object).map(|record| (record, None));
    }
    normal_record(id, object, live_model, tokenizer)
}

/// Normal cases have exactly the Track P1 organizer fields.
fn normal_record(
    id: Value,
    case: &Map<String, Value>,
    live_model: Option<&str>,
    tokenizer: Option<&CoreBPE>,
) -> AppResult<(Value, Option<TokenMeasurement>)> {
    let native_request = native_openai_request(case, live_model);
    let tools = tool_defs_from_request(&native_request).map_err(compact_error_message)?;
    let calls = expected_calls(case).map_err(compact_error_message)?;
    let rendered_calls = calls.iter().map(render_call).collect::<Vec<_>>().join("\n");

    let (compact_request, compacted) = match encode_tools(&tools) {
        Ok(compact) if tool_choice_allows_compaction(&native_request) => {
            (post_compaction_request(&native_request, &compact), true)
        }
        Ok(_) | Err(_) => (native_request.clone(), false),
    };
    let roundtrip_calls = decode_calls(&rendered_calls, &tools)
        .map_err(compact_error_message)
        .map(tool_calls_json)?;

    let measurement = tokenizer
        .map(|tokenizer| {
            TokenMeasurement::from_requests(&id, &native_request, &compact_request, tokenizer)
        })
        .transpose()?;
    Ok((
        json!({
            "id": id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        }),
        measurement,
    ))
}

/// Decoder cases do not run normal-case compaction or expected-call round trips.
fn decoder_record(id: Value, case: &Map<String, Value>) -> AppResult<Value> {
    let native_request = native_openai_request(case, None);
    let tools = tool_defs_from_request(&native_request).map_err(compact_error_message)?;
    let chunks = decoder_chunks(case)?;
    let decoded = match decode_stream_chunks(&chunks, &tools) {
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

    if !request.contains_key("model")
        && let Some(model) = live_model
    {
        request.insert("model".into(), Value::String(model.into()));
    }
    if !request.contains_key("tools")
        && let Some(tools) = case_value(case, &["tools", "tool_definitions"])
    {
        request.insert("tools".into(), tools.clone());
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
    let values = case_value(case, &["expected_calls", "calls", "tool_calls", "expected"])
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| case.get("expected").and_then(Value::as_array).cloned())
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
        .ok_or_else(|| String::from("decoder case is missing ordered chunks"))?
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

fn decode_stream_chunks(chunks: &[String], tools: &[ToolDef]) -> CompactResult<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools);
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish()
}

fn canonical_json_line(value: &Value) -> AppResult<Vec<u8>> {
    let mut line = serde_json::to_vec(&canonicalize(value))
        .map_err(|error| format!("could not serialize JSONL value: {error}"))?;
    line.push(b'\n');
    Ok(line)
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
    fn official_cases_resolve_tool_references_in_case_order() {
        let cases = cases_from_value(json!({
            "schema_version": "v1",
            "tools": [
                {"type": "function", "function": {"name": "first"}},
                {"type": "function", "function": {"name": "second"}}
            ],
            "cases": [{"tools": ["second", "first"]}],
            "decoder_cases": [{"tools": ["first"], "chunks": []}]
        }))
        .unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0]["tools"][0]["function"]["name"], "second");
        assert_eq!(cases[0]["tools"][1]["function"]["name"], "first");
        assert_eq!(cases[1]["tools"][0]["function"]["name"], "first");
    }

    #[test]
    fn canonical_jsonl_output_is_byte_stable() {
        let record = json!({
            "roundtrip_calls": [],
            "id": null,
            "compacted": true,
            "compact_request": {"messages": []},
            "rendered_calls": ""
        });
        assert_eq!(
            canonical_json_line(&record).unwrap(),
            canonical_json_line(&record).unwrap()
        );
    }

    #[test]
    fn successful_compaction_removes_native_tools_and_injects_instructions() {
        let native = json!({
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "lookup"}}],
            "tool_choice": "auto"
        });
        let compact = encode_tools(&tool_defs_from_request(&native).unwrap()).unwrap();
        let request = post_compaction_request(&native, &compact);
        assert!(request.get("tools").is_none());
        assert!(request.get("tool_choice").is_none());
        assert_eq!(request["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn stream_decoder_decodes_calls_across_ordered_chunks() {
        let tools = vec![ToolDef {
            name: "lookup".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"q": {"type": "string"}},
                "required": ["q"]
            })),
        }];
        let calls =
            decode_stream_chunks(&["<<call look".into(), "up {\"q\":\"x\"}>>".into()], &tools)
                .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup");
    }

    #[test]
    fn dataset_with_tools_catalog_and_decoder_cases_is_loaded_and_resolved() {
        let dataset = json!({
            "schema_version": "compact-tools-eval-v1",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Find info",
                    "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}
                }
            }],
            "cases": [{
                "id": "ct-001",
                "tools": ["lookup"],
                "messages": [{"role": "user", "content": "hi"}],
                "expected": [{"name": "lookup", "arguments": {"q": "test"}}]
            }],
            "decoder_cases": [{
                "id": "dc-001",
                "tools": ["lookup"],
                "chunks": ["<<call lookup {\"q\":\"test\"}>>"]
            }]
        });
        let cases = cases_from_value(dataset).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0]["id"], "ct-001");
        assert_eq!(cases[0]["tools"][0]["function"]["name"], "lookup");
        assert_eq!(cases[1]["id"], "dc-001");
        assert_eq!(cases[1]["tools"][0]["function"]["name"], "lookup");
    }
}
