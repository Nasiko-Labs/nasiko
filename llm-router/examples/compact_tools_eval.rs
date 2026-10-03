use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, Write};

fn main() {
    if let Err(error) = run() {
        eprintln!("compact_tools_eval error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET").ok();

    let eval: Value = match eval_path {
        Some(path) => {
            let text = fs::read_to_string(path)?;
            serde_json::from_str(&text)?
        }

        None => builtin_eval_set(),
    };

    let cases = eval
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
let mut output: Box<dyn Write> = match env::var("OUT") {
    Ok(path) => Box::new(io::BufWriter::new(fs::File::create(path)?)),
    Err(_) => Box::new(io::BufWriter::new(io::stdout())),
};

    for case in &cases {
        let result = evaluate_case(case);

        serde_json::to_writer(&mut output, &result)?;
        writeln!(&mut output)?;
    }

    // Some evaluation sets contain decoder-specific cases outside
    // the normal request cases.
    if let Some(decoder_cases) = eval.get("decoder_cases").and_then(Value::as_array) {
        for case in decoder_cases {
            let result = evaluate_decoder_case(case);

            serde_json::to_writer(&mut output, &result)?;
            writeln!(&mut output)?;
        }
    }

    output.flush()?;

    Ok(())
}

fn evaluate_case(case: &Value) -> Value {
    let case_id = case.get("id").and_then(Value::as_str).unwrap_or("unknown");

    let tools_value = case.get("tools").or_else(|| case.get("tool_definitions"));

    let tools: Vec<ToolDef> = match tools_value {
        Some(value) => match parse_tools(value) {
            Ok(parsed) => parsed,
            Err(error) => {
                return json!({
                    "case_id": case_id,
                    "compact_request": null,
                    "compacted": false,
                    "rendered_calls": [],
                    "roundtrip_calls": [],
                    "decoded": false,
                    "error": error
                });
            }
        },

        None => Vec::new(),
    };

    let encoded = match encode_tools(&tools) {
        Ok(encoded) => encoded,

        Err(error) => {
            return json!({
                "case_id": case_id,
                "compact_request": null,
                "compacted": false,
                "rendered_calls": [],
                "roundtrip_calls": [],
                "decoded": false,
                "error": error.to_string()
            });
        }
    };

    let expected_calls = extract_expected_calls(case);

    let rendered_calls = render_calls(&expected_calls);

    let roundtrip = decode_calls(&rendered_calls, &tools);

    let (roundtrip_calls, decoded, decode_error) = match roundtrip {
        Ok(calls) => {
            let calls_json: Vec<Value> = calls
                .iter()
                .map(|call| {
                    json!({
                        "name": call.name,
                        "arguments": parse_json_or_string(
                            &call.arguments
                        )
                    })
                })
                .collect();

            (calls_json, true, Value::Null)
        }

        Err(error) => (Vec::new(), false, json!(error.to_string())),
    };

    let compact_request = build_compact_request(case, &encoded.text);

    let baseline_text = build_baseline_request(case, &tools);

    json!({
        "case_id": case_id,

        "compact_request": compact_request,

        "compacted": true,

        "baseline_chars": baseline_text.len(),

        "compact_chars": encoded.text.len(),

        "rendered_calls": rendered_calls,

        "roundtrip_calls": roundtrip_calls,

        "decoded": decoded,

        "decode_error": decode_error,

        "expected_calls": expected_calls,

        "tool_count": tools.len()
    })
}

fn evaluate_decoder_case(case: &Value) -> Value {
    let case_id = case
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("decoder-unknown");

    let tools = match case.get("tools") {
        Some(value) => match parse_tools(value) {
            Ok(tools) => tools,
            Err(error) => {
                return json!({
                    "case_id": case_id,
                    "decoder_case": true,
                    "decoded": false,
                    "error": error
                });
            }
        },

        None => Vec::new(),
    };

    let chunks = case
        .get("chunks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            case.get("raw")
                .and_then(Value::as_str)
                .map(|text| vec![Value::String(text.to_string())])
                .unwrap_or_default()
        });

    let mut combined = String::new();

    for chunk in chunks {
        if let Some(chunk) = chunk.as_str() {
            combined.push_str(chunk);
        }
    }

    let result = decode_calls(&combined, &tools);

    match result {
        Ok(calls) => {
            let decoded: Vec<Value> = calls
                .iter()
                .map(|call| {
                    json!({
                        "name": call.name,
                        "arguments": parse_json_or_string(
                            &call.arguments
                        )
                    })
                })
                .collect();

            json!({
                "case_id": case_id,
                "decoder_case": true,
                "decoded": true,
                "roundtrip_calls": decoded,
                "error": null
            })
        }

        Err(error) => {
            json!({
                "case_id": case_id,
                "decoder_case": true,
                "decoded": false,
                "roundtrip_calls": [],
                "error": error.to_string()
            })
        }
    }
}

fn parse_tools(value: &Value) -> Result<Vec<ToolDef>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "tools must be an array".to_string())?;

    let mut tools = Vec::new();

    for item in array {
        // Official evaluation format:
        // "tools": ["create_calendar_event", "send_email"]
        if let Some(name) = item.as_str() {
            let builtin =
                builtin_tool_by_name(name).ok_or_else(|| format!("unknown tool: {name}"))?;

            let description = builtin
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);

            let parameters = builtin.get("parameters").cloned();

            tools.push(ToolDef {
                name: name.to_string(),
                description,
                parameters,
            });

            continue;
        }

        // Local/built-in format:
        // {
        //   "type": "function",
        //   "function": {
        //       "name": "...",
        //       "description": "...",
        //       "parameters": {...}
        //   }
        // }
        let function = item.get("function").unwrap_or(item);

        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool missing function.name".to_string())?;

        let description = function
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);

        let parameters = function.get("parameters").cloned();

        tools.push(ToolDef {
            name: name.to_string(),
            description,
            parameters,
        });
    }

    Ok(tools)
}
fn builtin_tool_by_name(name: &str) -> Option<Value> {
    let eval = builtin_eval_set();

    let cases = eval.get("cases")?.as_array()?;

    for case in cases {
        let tools = case.get("tools")?.as_array()?;

        for tool in tools {
            let function = tool.get("function")?;

            if function.get("name").and_then(Value::as_str) == Some(name) {
                return Some(function.clone());
            }
        }
    }

    None
}

fn extract_expected_calls(case: &Value) -> Vec<Value> {
    let calls = case
        .get("expected")
        .or_else(|| case.get("expected_calls"))
        .or_else(|| case.get("expected_tool_calls"))
        .or_else(|| case.get("tool_calls"));

    match calls {
        Some(Value::Array(calls)) => calls.clone(),
        _ => Vec::new(),
    }
}

fn render_calls(calls: &[Value]) -> String {
    let mut output = String::new();

    for call in calls {
        let name = call
            .get("name")
            .or_else(|| call.get("function").and_then(|f| f.get("name")))
            .and_then(Value::as_str);

        let arguments = call
            .get("arguments")
            .or_else(|| call.get("function").and_then(|f| f.get("arguments")));

        let Some(name) = name else {
            continue;
        };

        let arguments = match arguments {
            Some(Value::String(arguments)) => arguments.clone(),

            Some(value) => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),

            None => "{}".to_string(),
        };

        output.push_str("<<call ");
        output.push_str(name);
        output.push(' ');
        output.push_str(&arguments);
        output.push_str(">>\n");
    }

    output
}

fn build_compact_request(case: &Value, compact_tools: &str) -> Value {
    let messages = case.get("messages").cloned().unwrap_or_else(|| {
        case.get("prompt")
            .map(|prompt| {
                json!([
                    {
                        "role": "user",
                        "content": prompt
                    }
                ])
            })
            .unwrap_or_else(|| json!([]))
    });

    json!({
        "compact_tools": compact_tools,
        "messages": messages
    })
}

fn build_baseline_request(case: &Value, tools: &[ToolDef]) -> String {
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                }
            })
        })
        .collect();

    let messages = case
        .get("messages")
        .cloned()
        .unwrap_or_else(|| case.get("prompt").cloned().unwrap_or(Value::Null));

    serde_json::to_string(&json!({
        "tools": tools_json,
        "messages": messages
    }))
    .unwrap_or_default()
}

fn parse_json_or_string(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// Small deterministic fallback evaluation set.
///
/// This means the example can run even when EVAL_SET is not
/// supplied, while the official evaluator can provide its own set.
fn builtin_eval_set() -> Value {
    json!({
        "schema_version": 1,
        "purpose": "compact tool schema evaluation",

        "cases": [
            {
                "id": "ct-local-001",

                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "create_calendar_event",
                            "description": "Create an event",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "title": {
                                        "type": "string"
                                    },
                                    "start": {
                                        "type": "string"
                                    },
                                    "duration_min": {
                                        "type": "integer"
                                    },
                                    "visibility": {
                                        "type": "string",
                                        "enum": [
                                            "public",
                                            "private"
                                        ]
                                    }
                                },
                                "required": [
                                    "title",
                                    "start"
                                ]
                            }
                        }
                    }
                ],

                "messages": [
                    {
                        "role": "user",
                        "content": "Create a private design review tomorrow."
                    }
                ],

                "expected_calls": [
                    {
                        "name": "create_calendar_event",
                        "arguments": {
                            "title": "Design review",
                            "start": "2026-10-03T10:00:00",
                            "visibility": "private"
                        }
                    }
                ]
            },

            {
                "id": "ct-local-002",

                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "create_calendar_event",
                            "description": "Create an event",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "title": {
                                        "type": "string"
                                    },
                                    "start": {
                                        "type": "string"
                                    }
                                },
                                "required": [
                                    "title",
                                    "start"
                                ]
                            }
                        }
                    },
                    {
                        "type": "function",
                        "function": {
                            "name": "send_email",
                            "description": "Send an email",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "to": {
                                        "type": "array",
                                        "items": {
                                            "type": "string"
                                        }
                                    },
                                    "subject": {
                                        "type": "string"
                                    },
                                    "body": {
                                        "type": "string"
                                    }
                                },
                                "required": [
                                    "to",
                                    "subject",
                                    "body"
                                ]
                            }
                        }
                    }
                ],

                "messages": [
                    {
                        "role": "user",
                        "content": "Schedule the review and email the attendees."
                    }
                ],

                "expected_calls": [
                    {
                        "name": "create_calendar_event",
                        "arguments": {
                            "title": "Review",
                            "start": "2026-10-03T10:00:00"
                        }
                    },
                    {
                        "name": "send_email",
                        "arguments": {
                            "to": [
                                "team@example.com"
                            ],
                            "subject": "Review",
                            "body": "The review is scheduled."
                        }
                    }
                ]
            },

            {
                "id": "ct-local-003",

                "tools": [
                    {
                        "type": "function",
                        "function": {
                            "name": "create_calendar_event",
                            "description": "Create an event",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "title": {
                                        "type": "string"
                                    }
                                },
                                "required": [
                                    "title"
                                ]
                            }
                        }
                    }
                ],

                "messages": [
                    {
                        "role": "user",
                        "content": "What is the weather today?"
                    }
                ],

                "expected_calls": []
            }
        ],

        "decoder_cases": [
            {
                "id": "decoder-local-001",

                "tools": [
                    {
                        "name": "create_calendar_event",
                        "description": "Create an event",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "title": {
                                    "type": "string"
                                },
                                "start": {
                                    "type": "string"
                                }
                            },
                            "required": [
                                "title",
                                "start"
                            ]
                        }
                    }
                ],

                "chunks": [
                    "Thinking... <<cal",
                    "l create_calendar_event ",
                    "{\"title\":\"A >> B\",",
                    "\"start\":\"2026-10-03\"}>>"
                ]
            }
        ]
    })
}
