use nasiko_tool_compact::{
    decode_calls, encode_tools, Error as CompactError, StreamDecoder, ToolDef,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
use tiktoken_rs::o200k_base;

#[derive(Debug, Deserialize)]
struct EvalFile {
    schema_version: String,
    tools: Vec<NativeTool>,
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize, Clone)]
struct NativeTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: NativeFunction,
}

#[derive(Debug, Deserialize, Clone)]
struct NativeFunction {
    name: String,
    description: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: Value,
}

fn token_count(value: &Value) -> usize {
    let text = serde_json::to_string(value).expect("serialize request");
    o200k_base()
        .expect("initialize o200k_base")
        .encode_ordinary(&text)
        .len()
}

fn resolve_tools(all: &[NativeTool], names: &[String]) -> Result<Vec<ToolDef>, String> {
    names
        .iter()
        .map(|name| {
            let native = all
                .iter()
                .find(|tool| tool.function.name == *name)
                .ok_or_else(|| format!("tool not found: {name}"))?;

            if native.tool_type != "function" {
                return Err(format!("unsupported tool type: {}", native.tool_type));
            }

            Ok(ToolDef {
                name: native.function.name.clone(),
                description: native.function.description.clone(),
                parameters: native.function.parameters.clone(),
            })
        })
        .collect()
}

fn resolve_native_tools(all: &[NativeTool], names: &[String]) -> Result<Vec<Value>, String> {
    names
        .iter()
        .map(|name| {
            let native = all
                .iter()
                .find(|tool| tool.function.name == *name)
                .ok_or_else(|| format!("tool not found: {name}"))?;

            Ok(json!({
                "type": native.tool_type,
                "function": {
                    "name": native.function.name,
                    "description": native.function.description,
                    "parameters": native.function.parameters
                }
            }))
        })
        .collect()
}

fn render_expected(expected: &[ExpectedCall]) -> String {
    expected
        .iter()
        .map(|call| {
            format!(
                "<<call {} {}>>",
                call.name,
                serde_json::to_string(&call.arguments).expect("serialize arguments")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn calls_json(calls: &[nasiko_tool_compact::ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "name": call.name,
                    "arguments": call.arguments
                })
            })
            .collect(),
    )
}

fn error_kind(error: &CompactError) -> &'static str {
    match error {
        CompactError::UnknownTool(_) => "unknown_tool",
        CompactError::InvalidArguments { .. } => "invalid_arguments",
        CompactError::MalformedCall | CompactError::InvalidJson(_) => "malformed_output",
        _ => "error",
    }
}

fn decode_text(text: &str, tools: &[ToolDef]) -> Value {
    match decode_calls(text, tools) {
        Ok(calls) => json!({ "calls": calls_json(&calls) }),
        Err(error) => json!({ "error": error_kind(&error) }),
    }
}

fn decode_chunks(chunks: &[String], tools: &[ToolDef]) -> Value {
    let mut decoder = match StreamDecoder::new(tools) {
        Ok(value) => value,
        Err(error) => return json!({ "error": error_kind(&error) }),
    };

    for chunk in chunks {
        if let Err(error) = decoder.push(chunk) {
            return json!({ "error": error_kind(&error) });
        }
    }

    match decoder.finish() {
        Ok(calls) => json!({ "calls": calls_json(&calls) }),
        Err(error) => json!({ "error": error_kind(&error) }),
    }
}

fn main() {
    let eval_path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let raw = std::fs::read_to_string(&eval_path).expect("read EVAL_SET");
    let data: EvalFile = serde_json::from_str(&raw).expect("valid eval JSON");

    assert_eq!(data.schema_version, "compact-tools-eval-v1");

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    for case in data.cases {
        let tools = resolve_tools(&data.tools, &case.tools).expect("resolve tools");
        let native_tools =
            resolve_native_tools(&data.tools, &case.tools).expect("resolve native tools");

        let native_request = json!({
            "model": "native-tools-baseline",
            "messages": case.messages,
            "tools": native_tools,
            "temperature": 0
        });

        let compact = match encode_tools(&tools) {
            Ok(value) => value,
            Err(error) => {
                let line = json!({
                    "id": case.id,
                    "compact_request": native_request,
                    "compacted": false,
                    "rendered_calls": "",
                    "roundtrip_calls": [],
                    "bypass_reason": error.to_string()
                });
                writeln!(out, "{line}").expect("write OUT");
                continue;
            }
        };

        let mut compact_messages = Vec::with_capacity(case.messages.len() + 1);
        compact_messages.push(json!({
            "role": "system",
            "content": compact.as_str()
        }));
        compact_messages.extend(case.messages);

        let compact_request = json!({
            "model": "compact-tools-offline",
            "messages": compact_messages,
            "temperature": 0
        });

        let rendered_calls = render_expected(&case.expected);
        let roundtrip = decode_text(&rendered_calls, &tools);

        let line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": true,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip.get("calls").cloned().unwrap_or_else(|| json!([])),
            "baseline_tokens": token_count(&native_request),
            "compact_tokens": token_count(&compact_request)
        });

        writeln!(out, "{line}").expect("write OUT");
    }

    for case in data.decoder_cases {
        let tools = resolve_tools(&data.tools, &case.tools).expect("resolve decoder tools");
        let decoded = decode_chunks(&case.chunks, &tools);

        let line = json!({
            "id": case.id,
            "decoded": decoded,
            "expected": case.expected
        });

        writeln!(out, "{line}").expect("write OUT");
    }

    out.flush().expect("flush OUT");
}
