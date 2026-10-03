use nasiko_tool_compact::{
    Schema, SchemaProperty, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
};
use tiktoken_rs::o200k_base;

#[derive(Debug, Deserialize)]
struct EvalFile {
    schema_version: String,
    tools: Vec<RawTool>,
    cases: Vec<EvalCase>,
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct RawTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: RawFunction,
}

#[derive(Debug, Deserialize)]
struct RawFunction {
    name: String,
    description: Option<String>,
    parameters: RawSchema,
}

#[derive(Debug, Deserialize)]
struct RawSchema {
    #[serde(rename = "type")]
    schema_type: Option<String>,
    properties: Option<HashMap<String, RawProperty>>,
    required: Option<Vec<String>>,
    #[serde(rename = "additionalProperties")]
    additional_properties: Option<bool>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct RawProperty {
    #[serde(rename = "type")]
    property_type: Option<String>,
    format: Option<String>,
    description: Option<String>,
    #[serde(rename = "enum")]
    enum_values: Option<Vec<Value>>,
    items: Option<Box<RawProperty>>,
    properties: Option<HashMap<String, RawProperty>>,
    required: Option<Vec<String>>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Message>,
    expected: Vec<ExpectedCall>,
    #[allow(dead_code)]
    r#match: Option<MatchRules>,
}

#[derive(Debug, Deserialize)]
struct MatchRules {
    #[allow(dead_code)]
    free_text_fields: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[allow(dead_code)]
    note: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: DecoderExpected,
}

#[derive(Debug, Deserialize)]
struct DecoderExpected {
    calls: Option<Vec<ExpectedCall>>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvalOutput {
    id: String,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<Value>,
    decoded: Value,
}

fn raw_to_tool(raw: &RawTool) -> Result<ToolDef, String> {
    if raw.tool_type != "function" {
        return Err(format!("unsupported tool type: {}", raw.tool_type));
    }

    Ok(ToolDef {
        name: raw.function.name.clone(),
        description: raw.function.description.clone(),
        parameters: raw_schema_to_schema(&raw.function.parameters)?,
    })
}

fn reject_unsupported(
    extra: &HashMap<String, Value>,
    supported: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some(key) = extra.keys().find(|key| !supported.contains(&key.as_str())) {
        return Err(format!(
            "unsupported schema keyword `{}` in {}",
            key, context
        ));
    }

    Ok(())
}

fn raw_schema_to_schema(raw: &RawSchema) -> Result<Schema, String> {
    reject_unsupported(
        &raw.extra,
        &["type", "properties", "required", "additionalProperties"],
        "root schema",
    )?;

    let properties = match &raw.properties {
        Some(props) => props
            .iter()
            .map(|(name, property)| Ok((name.clone(), raw_property_to_property(property)?)))
            .collect::<Result<BTreeMap<_, _>, String>>()?,
        None => BTreeMap::new(),
    };

    Ok(Schema {
        schema_type: raw
            .schema_type
            .clone()
            .unwrap_or_else(|| "object".to_string()),
        properties,
        required: raw.required.clone().unwrap_or_default(),
        additional_properties: raw.additional_properties,
    })
}

fn raw_property_to_property(raw: &RawProperty) -> Result<SchemaProperty, String> {
    reject_unsupported(
        &raw.extra,
        &[
            "type",
            "format",
            "description",
            "enum",
            "items",
            "properties",
            "required",
        ],
        "property",
    )?;

    let items = raw
        .items
        .as_ref()
        .map(|item| raw_property_to_property(item))
        .transpose()?
        .map(Box::new);

    let properties = match &raw.properties {
        Some(props) => Some(
            props
                .iter()
                .map(|(name, property)| Ok((name.clone(), raw_property_to_property(property)?)))
                .collect::<Result<BTreeMap<_, _>, String>>()?,
        ),
        None => None,
    };

    Ok(SchemaProperty {
        property_type: raw.property_type.clone(),
        format: raw.format.clone(),
        description: raw.description.clone(),
        enum_values: raw.enum_values.clone(),
        items,
        properties,
        required: raw.required.clone(),
    })
}

fn selected_tools(eval_tools: &[RawTool], names: &[String]) -> Result<Vec<ToolDef>, String> {
    let map: HashMap<&str, &RawTool> = eval_tools
        .iter()
        .map(|tool| (tool.function.name.as_str(), tool))
        .collect();

    names
        .iter()
        .map(|name| {
            let raw = map
                .get(name.as_str())
                .ok_or_else(|| format!("unknown evaluation tool: {name}"))?;

            raw_to_tool(raw)
        })
        .collect()
}

fn expected_to_value(call: &ExpectedCall) -> Value {
    json!({
        "name": call.name,
        "arguments": call.arguments
    })
}

fn render_expected_calls(calls: &[ExpectedCall]) -> Result<String, String> {
    calls
        .iter()
        .map(|call| {
            serde_json::to_string(&call.arguments)
                .map(|args| format!("<<call {} {}>>", call.name, args))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("\n"))
}

fn render_call(call: &ToolCall) -> Value {
    json!({
        "name": call.name,
        "arguments": call.arguments
    })
}

fn compact_request(messages: &[Message], compact_definitions: &str) -> Value {
    let instruction = format!(
        "Available tools:\n{}\n\n\
         Tool call grammar:\n\
         <<call TOOL_NAME {{JSON_ARGUMENTS}}>>\n\
         Emit one marker per tool call. JSON arguments must match the tool schema. \
         Do not invent tools or arguments. If no tool is needed, answer normally.",
        compact_definitions
    );

    let mut compact_messages = Vec::with_capacity(messages.len() + 1);

    compact_messages.push(json!({
        "role": "system",
        "content": instruction
    }));

    for message in messages {
        compact_messages.push(json!({
            "role": message.role,
            "content": message.content
        }));
    }

    json!({
        "messages": compact_messages,
        "tools": []
    })
}

fn stream_decode(chunks: &[String], tools: Vec<ToolDef>) -> Result<Vec<ToolCall>, String> {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();

    for chunk in chunks {
        match decoder.push(chunk) {
            Ok(mut chunk_calls) => calls.append(&mut chunk_calls),
            Err(error) => return Err(error.to_string()),
        }
    }

    match decoder.finish() {
        Ok(mut final_calls) => {
            calls.append(&mut final_calls);
            Ok(calls)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn decoded_value(result: Result<Vec<ToolCall>, String>) -> Value {
    match result {
        Ok(calls) => json!({
            "calls": calls.iter().map(render_call).collect::<Vec<_>>()
        }),
        Err(error) => json!({
            "error": error
        }),
    }
}
fn token_count(tokenizer: &tiktoken_rs::CoreBPE, text: &str) -> usize {
    tokenizer.encode_with_special_tokens(text).len()
}

fn native_tool_definitions(tools: &[ToolDef]) -> Result<String, String> {
    tools
        .iter()
        .map(|tool| {
            serde_json::to_string(tool)
                .map_err(|error| format!("failed to serialize native tool: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("\n"))
}

fn token_reduction(native_tokens: usize, compact_tokens: usize) -> f64 {
    if native_tokens == 0 {
        return 0.0;
    }

    ((native_tokens.saturating_sub(compact_tokens)) as f64 / native_tokens as f64) * 100.0
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET").map_err(|_| "EVAL_SET is required")?;

    let out_path = env::var("OUT").map_err(|_| "OUT is required")?;

    let input = fs::read_to_string(&eval_path)?;
    let eval: EvalFile = serde_json::from_str(&input)?;
    let tokenizer = o200k_base()
        .map_err(|error| format!("failed to initialize o200k_base tokenizer: {error}"))?;
    if eval.schema_version != "compact-tools-eval-v1" {
        return Err(format!("unsupported schema version: {}", eval.schema_version).into());
    }

    let mut output = String::new();
    let mut total_native_tokens = 0usize;
    let mut total_compact_tokens = 0usize;
    println!("=== Compact Tools Evaluation ===");
    println!("Dataset: {}", eval_path);
    println!("Cases: {}", eval.cases.len());
    println!();

    for case in &eval.cases {
        let tools = match selected_tools(&eval.tools, &case.tools) {
            Ok(tools) => tools,
            Err(error) => {
                eprintln!("{}: compaction bypassed: {}", case.id, error);

                let record = json!({
                    "id": case.id,
                    "compact_request": {
                        "messages": case.messages,
                        "tools": case.tools
                    },
                    "compacted": false,
                    "rendered_calls": "",
                    "roundtrip_calls": [],
                    "decoded": {
                        "error": error
                    }
                });

                output.push_str(&serde_json::to_string(&record)?);
                output.push('\n');

                println!("{}: BYPASS", case.id);
                continue;
            }
        };

        let compact = match encode_tools(&tools) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{}: compaction bypassed: {}", case.id, error);

                let record = json!({
                    "id": case.id,
                    "compact_request": {
                        "messages": case.messages,
                        "tools": case.tools
                    },
                    "compacted": false,
                    "rendered_calls": "",
                    "roundtrip_calls": [],
                    "decoded": {
                        "error": error.to_string()
                    }
                });

                output.push_str(&serde_json::to_string(&record)?);
                output.push('\n');

                println!("{}: BYPASS", case.id);
                continue;
            }
        };

        let rendered_calls = render_expected_calls(&case.expected)?;

        let roundtrip_result = decode_calls(&rendered_calls, &tools);

        let roundtrip_calls = match &roundtrip_result {
            Ok(calls) => calls.iter().map(render_call).collect::<Vec<_>>(),
            Err(error) => {
                eprintln!("{}: roundtrip decoder error: {}", case.id, error);
                Vec::new()
            }
        };

        let decoded_result = stream_decode(&[rendered_calls.clone()], tools.clone());

        let decoded = decoded_value(decoded_result);

        let expected_values = case
            .expected
            .iter()
            .map(expected_to_value)
            .collect::<Vec<_>>();

        let roundtrip_ok = roundtrip_calls == expected_values;

        println!(
            "{}: {}",
            case.id,
            if roundtrip_ok { "PASS" } else { "FAIL" }
        );

        let compact_request = compact_request(&case.messages, &compact.definitions);

        let native_definitions = native_tool_definitions(&tools)?;

        let compact_system_prompt = compact_request["messages"][0]["content"]
            .as_str()
            .ok_or("compact system prompt is not a string")?;

        let native_tokens = token_count(&tokenizer, &native_definitions);
        let compact_tokens = token_count(&tokenizer, compact_system_prompt);

        let reduction = token_reduction(native_tokens, compact_tokens);

        total_native_tokens += native_tokens;
        total_compact_tokens += compact_tokens;

        println!(
            "{}: tokens native={} compact={} reduction={:.1}%",
            case.id, native_tokens, compact_tokens, reduction
        );

        let record = EvalOutput {
            id: case.id.clone(),
            compact_request,
            compacted: true,
            rendered_calls,
            roundtrip_calls,
            decoded,
        };

        output.push_str(&serde_json::to_string(&record)?);
        output.push('\n');
    }
    let total_reduction = token_reduction(total_native_tokens, total_compact_tokens);

    println!();
    println!("=== Token Reduction ===");
    println!("Native tokens:  {}", total_native_tokens);
    println!("Compact tokens: {}", total_compact_tokens);
    println!("Reduction:      {:.1}%", total_reduction);
    println!();
    println!();
    println!("=== Decoder Cases ===");

    for case in &eval.decoder_cases {
        let tools = selected_tools(&eval.tools, &case.tools)?;

        // IMPORTANT:
        // Feed the original chunks directly into StreamDecoder.
        // This preserves marker splits such as:
        // "<<cal" + "l tool {...}>>"
        let result = stream_decode(&case.chunks, tools);

        let passed = match (&case.expected.calls, &case.expected.error, &result) {
            (Some(expected), None, Ok(actual)) => {
                let expected_values = expected.iter().map(expected_to_value).collect::<Vec<_>>();

                let actual_values = actual.iter().map(render_call).collect::<Vec<_>>();

                actual_values == expected_values
            }

            (None, Some(expected_error), Err(error)) => {
                let lower = error.to_lowercase();

                match expected_error.as_str() {
                    "unknown_tool" => lower.contains("unknown"),
                    "invalid_arguments" => {
                        lower.contains("invalid")
                            || lower.contains("required")
                            || lower.contains("enum")
                    }
                    _ => false,
                }
            }

            _ => false,
        };

        println!(
            "{}: {} - {}",
            case.id,
            if passed { "PASS" } else { "FAIL" },
            case.note
        );

        let decoded = decoded_value(result);

        let record = json!({
            "id": case.id,
            "decoded": decoded
        });

        output.push_str(&serde_json::to_string(&record)?);
        output.push('\n');
    }

    fs::write(&out_path, output)?;

    println!();
    println!("Output: {}", out_path);

    Ok(())
}
