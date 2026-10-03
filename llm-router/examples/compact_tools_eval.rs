use nasiko_llm_router::compact_tools::{StreamDecoder, StreamEvent, prepare_request};
use nasiko_llm_router::ir::{ChatRequest, FunctionDef, Message, ToolDef};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Write};

fn make_tool(index: usize) -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: format!("operation_{index}"),
            description: Some(format!(
                "Perform business operation number {index} using the supplied parameters"
            )),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "resource_id": {
                        "type": "string",
                        "description": "Unique identifier of the target resource"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["create", "read", "update", "delete"],
                        "description": "Action to perform"
                    },
                    "priority": {
                        "type": "integer",
                        "enum": [1, 2, 3, 4, 5],
                        "description": "Priority of the operation"
                    },
                    "options": {
                        "type": "object",
                        "properties": {
                            "dry_run": {
                                "type": "boolean",
                                "description": "Validate without applying changes"
                            },
                            "region": {
                                "type": "string",
                                "enum": ["us", "eu", "apac"]
                            }
                        },
                        "required": ["dry_run"]
                    }
                },
                "required": ["resource_id", "action"],
                "additionalProperties": false
            })),
        },
        extra: Map::new(),
    }
}

fn make_tools(count: usize) -> Vec<ToolDef> {
    (1..=count).map(make_tool).collect()
}

fn request_with_tools(tools: Vec<ToolDef>) -> ChatRequest {
    ChatRequest {
        model: Some("eval-model".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Perform the requested task.".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        }],
        tools: Some(tools),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: Map::new(),
    }
}

struct ResultRow {
    tools: usize,
    native_bytes: usize,
    compact_bytes: usize,
    schema_bytes: usize,
    overhead_bytes: usize,
    saved_bytes: isize,
    reduction: f64,
}

fn evaluate(tool_count: usize) -> ResultRow {
    let tools = make_tools(tool_count);

    let native_json = serde_json::to_string(&tools).expect("serialize native tools");

    let mut request = request_with_tools(tools);

    let context = prepare_request(&mut request)
        .expect("compact preparation should succeed")
        .expect("compact context should exist");

    let protocol = request
        .messages
        .first()
        .and_then(|message| message.content.as_ref())
        .and_then(Value::as_str)
        .expect("compact protocol system message");

    let native_bytes = native_json.len();
    let compact_bytes = protocol.len();
    let schema_bytes = context.compact_schema.as_str().len();

    let overhead_bytes = compact_bytes.saturating_sub(schema_bytes);
    let saved_bytes = native_bytes as isize - compact_bytes as isize;

    let reduction = if native_bytes == 0 {
        0.0
    } else {
        saved_bytes as f64 / native_bytes as f64 * 100.0
    };

    ResultRow {
        tools: tool_count,
        native_bytes,
        compact_bytes,
        schema_bytes,
        overhead_bytes,
        saved_bytes,
        reduction,
    }
}

fn error_kind(error: &impl std::fmt::Display) -> &'static str {
    let message = error.to_string().to_lowercase();

    if message.contains("unknown tool") {
        "unknown_tool"
    } else if message.contains("required")
        || message.contains("enum")
        || message.contains("argument")
        || message.contains("type")
    {
        "invalid_arguments"
    } else {
        "decoder_error"
    }
}

fn normalized_calls(events: &[StreamEvent]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ToolCall(call) => {
                let arguments: Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);

                Some(json!({
                    "name": call.function.name,
                    "arguments": arguments
                }))
            }
            StreamEvent::Text(_) => None,
        })
        .collect()
}

fn run_eval_set(path: &str, out_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    let root: Value = serde_json::from_str(&raw)?;

    let tool_values = root
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("eval set missing tools array")?;

    let all_tools: Vec<ToolDef> = tool_values
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;

    let tool_map: HashMap<String, ToolDef> = all_tools
        .into_iter()
        .map(|tool| (tool.function.name.clone(), tool))
        .collect();

    let decoder_cases = root
        .get("decoder_cases")
        .and_then(Value::as_array)
        .ok_or("eval set missing decoder_cases array")?;

    let file = fs::File::create(out_path)?;
    let mut writer = BufWriter::new(file);

    let mut passed = 0usize;
    let mut failed = 0usize;

    for case in decoder_cases {
        let id = case
            .get("id")
            .and_then(Value::as_str)
            .ok_or("decoder case missing id")?;

        let names = case
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("decoder case missing tools")?;

        let mut tools = Vec::new();

        for name in names {
            let name = name.as_str().ok_or("tool name must be string")?;

            let tool = tool_map
                .get(name)
                .ok_or_else(|| format!("eval references unknown tool {name}"))?;

            tools.push(tool.clone());
        }

        let chunks = case
            .get("chunks")
            .and_then(Value::as_array)
            .ok_or("decoder case missing chunks")?;

        let expected = case
            .get("expected")
            .ok_or("decoder case missing expected")?;

        let mut decoder = StreamDecoder::new();
        let mut events = Vec::new();
        let mut actual_error: Option<String> = None;

        for chunk in chunks {
            let chunk = chunk.as_str().ok_or("decoder chunk must be a string")?;

            match decoder.push(chunk, &tools) {
                Ok(mut produced) => events.append(&mut produced),
                Err(error) => {
                    actual_error = Some(error_kind(&error).to_string());
                    break;
                }
            }
        }

        if actual_error.is_none() {
            match decoder.finish() {
                Ok(mut produced) => events.append(&mut produced),
                Err(error) => {
                    actual_error = Some(error_kind(&error).to_string());
                }
            }
        }

        let actual_calls = normalized_calls(&events);

        let expected_error = expected
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string);

        let expected_calls = expected
            .get("calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let case_passed = match expected_error.as_deref() {
            Some(error) => actual_error.as_deref() == Some(error),
            None => actual_error.is_none() && actual_calls == expected_calls,
        };

        if case_passed {
            passed += 1;
        } else {
            failed += 1;
        }

        let record = json!({
            "id": id,
            "kind": "decoder",
            "passed": case_passed,
            "expected": expected,
            "actual": {
                "calls": actual_calls,
                "error": actual_error
            }
        });

        writeln!(writer, "{}", serde_json::to_string(&record)?)?;
    }

    let model_cases = root
        .get("cases")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let summary = json!({
        "kind": "summary",
        "decoder_total": decoder_cases.len(),
        "decoder_passed": passed,
        "decoder_failed": failed,
        "model_cases": model_cases,
        "model_cases_executed": 0,
        "note": "Natural-language model cases require a live model/provider evaluation."
    });

    writeln!(writer, "{}", serde_json::to_string(&summary)?)?;
    writer.flush()?;

    eprintln!(
        "Eval complete: {}/{} decoder cases passed; {} model cases not executed",
        passed,
        decoder_cases.len(),
        model_cases
    );

    if failed > 0 {
        return Err(format!("{failed} decoder case(s) failed").into());
    }

    Ok(())
}

fn main() {
    if let Ok(eval_set) = std::env::var("EVAL_SET") {
        let out =
            std::env::var("OUT").unwrap_or_else(|_| "/tmp/compact-tools-results.jsonl".to_string());

        if let Err(error) = run_eval_set(&eval_set, &out) {
            eprintln!("compact tools evaluation failed: {error}");
            std::process::exit(1);
        }

        return;
    }

    let counts = [1usize, 3, 5, 10, 20, 50];

    // Machine-readable mode for external tokenization/evaluation.
    // Each tool count is emitted as exactly one JSON object per line.
    if std::env::args().any(|arg| arg == "--json") {
        for count in counts {
            let tools = make_tools(count);

            let native = serde_json::to_string(&tools).expect("serialize native tools");

            let mut request = request_with_tools(tools);

            let context = prepare_request(&mut request)
                .expect("compact preparation should succeed")
                .expect("compact context should exist");

            let compact = request
                .messages
                .first()
                .and_then(|message| message.content.as_ref())
                .and_then(Value::as_str)
                .expect("compact protocol system message");

            println!(
                "{}",
                serde_json::json!({
                    "tools": count,
                    "native": native,
                    "compact": compact,
                    "compact_schema": context.compact_schema.as_str()
                })
            );
        }

        return;
    }

    println!();
    println!("Compact Tools Scaling Evaluation");
    println!("================================");
    println!();

    println!(
        "{:>5} | {:>12} | {:>13} | {:>12} | {:>10} | {:>11} | {:>9}",
        "Tools",
        "Native bytes",
        "Compact bytes",
        "Schema bytes",
        "Overhead",
        "Bytes saved",
        "Saving %"
    );

    println!("{}", "-".repeat(96));

    for count in counts {
        let result = evaluate(count);

        println!(
            "{:>5} | {:>12} | {:>13} | {:>12} | {:>10} | {:>11} | {:>8.2}%",
            result.tools,
            result.native_bytes,
            result.compact_bytes,
            result.schema_bytes,
            result.overhead_bytes,
            result.saved_bytes,
            result.reduction
        );
    }

    println!();
    println!("Notes:");
    println!("  Native bytes  = serialized native provider tool definitions");
    println!("  Compact bytes = complete provider-facing compact protocol");
    println!("  Schema bytes  = compact tool definitions only");
    println!("  Overhead      = fixed protocol/instruction cost");
    println!("  Saving %      = (native - compact) / native * 100");
}
