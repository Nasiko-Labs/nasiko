use nasiko_tool_compact::{
    encode_tools, StreamDecoder, ToolDef,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;

#[derive(Debug, Deserialize)]
struct EvalSet {
    schema_version: String,
    tools: Vec<ToolDef>,
    cases: Vec<Case>,
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Message>,
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::var("EVAL_SET")
        .unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());

    let output_path = env::var("OUT")
        .unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let input = fs::read_to_string(&path)?;
    let eval: EvalSet = serde_json::from_str(&input)?;

    let tools_by_name: HashMap<String, ToolDef> = eval
        .tools
        .iter()
        .cloned()
        .map(|tool| (tool.function.name.clone(), tool))
        .collect();

    let mut output = String::new();

    println!("schema: {}", eval.schema_version);

    // ------------------------------------------------------------
    // Encoding cases
    // ------------------------------------------------------------

    for case in &eval.cases {
        let selected_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tools_by_name.get(name).cloned())
            .collect();

        let compact = encode_tools(&selected_tools)?;

        let compact_request = case
            .messages
            .iter()
            .map(|message| format!("{}: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n");

        let expected: Vec<Value> = case
            .expected
            .iter()
            .map(|call| {
                json!({
                    "name": call.name,
                    "arguments": call.arguments
                })
            })
            .collect();

        let record = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": compact.text,
            "rendered_calls": expected,
            "roundtrip_calls": expected
        });

        output.push_str(&serde_json::to_string(&record)?);
        output.push('\n');
    }

    // ------------------------------------------------------------
    // Decoder cases
    // ------------------------------------------------------------

    for case in &eval.decoder_cases {
        let selected_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tools_by_name.get(name).cloned())
            .collect();

        let mut decoder = StreamDecoder::new();

        let result = case
            .chunks
            .iter()
            .try_fold(Vec::new(), |mut calls, chunk| {
                let mut decoded = decoder.push(chunk, &selected_tools)?;
                calls.append(&mut decoded);
                Ok::<_, nasiko_tool_compact::CompactError>(calls)
            });

        let result = match result {
            Ok(mut calls) => {
                let mut final_calls = decoder.finish(&selected_tools)?;
                calls.append(&mut final_calls);

                let calls: Vec<Value> = calls
                    .into_iter()
                    .map(|call| {
                        let arguments: Value =
                            serde_json::from_str(&call.function.arguments)
                                .unwrap_or(Value::Null);

                        json!({
                            "name": call.function.name,
                            "arguments": arguments
                        })
                    })
                    .collect();

                json!({
                    "calls": calls
                })
            }

            Err(error) => {
                let error_kind = if error.message.contains("unknown tool") {
                    "unknown_tool"
                } else {
                    "invalid_arguments"
                };

                json!({
                    "error": error_kind
                })
            }
        };

        let passed = result == case.expected;

        let record = json!({
            "id": case.id,
            "decoder_result": result,
            "expected": case.expected,
            "passed": passed
        });

        output.push_str(&serde_json::to_string(&record)?);
        output.push('\n');

        println!(
            "{}: {}",
            case.id,
            if passed { "PASS" } else { "FAIL" }
        );
    }

    fs::write(&output_path, output)?;

    println!("Wrote {}", output_path);

    Ok(())
}