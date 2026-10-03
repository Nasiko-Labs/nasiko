//! Evaluation harness for Nasiko compact tool schemas.
//! Runs offline deterministically or live against OpenAI-compatible proxy.
//! Usage: EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl cargo run --release -p nasiko-llm-router --example compact_tools_eval

use nasiko_tool_compact::{
    decode_calls, encode_tools, StreamDecoder, ToolCompactError, ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    chunks: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StandardCaseOutput {
    id: String,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct DecoderCaseOutput {
    id: String,
    decoded: DecodedResult,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum DecodedResult {
    Calls { calls: Vec<Value> },
    Error { error: String },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    println!("Running compact tools evaluation...");
    println!("Reading eval set from: {}", eval_set_path);
    println!("Writing JSONL outputs to: {}", out_path);

    let eval_file = match File::open(&eval_set_path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            serde_json::from_reader::<_, EvalFile>(reader)?
        }
        Err(_) => {
            eprintln!("Warning: {} not found. Generating default sample cases for local smoke test.", eval_set_path);
            sample_eval_file()
        }
    };

    let tools_by_name: HashMap<String, ToolDef> = eval_file
        .tools
        .iter()
        .map(|t| (t.function.name.clone(), t.clone()))
        .collect();

    let mut out_file = File::create(&out_path)?;

    // 1. Process Standard Evaluation Cases
    for case in &eval_file.cases {
        let active_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tools_by_name.get(name).cloned())
            .collect();

        let compact = encode_tools(&active_tools)?;

        // Render expected calls in compact syntax
        let mut rendered_parts = Vec::new();
        for exp in &case.expected {
            rendered_parts.push(format!("<<call {} {}>>", exp.name, exp.arguments));
        }
        let rendered_calls = rendered_parts.join("\\n");

        // Decode calls back to verify roundtrip fidelity
        let roundtrip_calls = match decode_calls(&rendered_calls, &active_tools) {
            Ok(calls) => calls
                .into_iter()
                .map(|c| {
                    json!({
                        "name": c.function.name,
                        "arguments": serde_json::from_str::<Value>(&c.function.arguments).unwrap_or(Value::Null)
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        // Construct standard OpenAI-shaped request body
        let mut messages = case.messages.clone();
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": format!(
                    "Available tools:\\n{}\\n\\n{}",
                    compact.compact_definitions,
                    compact.call_instructions
                )
            }),
        );

        let compact_request = json!({
            "model": "gpt-4o",
            "messages": messages,
            "temperature": 0
        });

        let line = StandardCaseOutput {
            id: case.id.clone(),
            compact_request,
            compacted: true,
            rendered_calls,
            roundtrip_calls,
        };

        writeln!(out_file, "{}", serde_json::to_string(&line)?)?;
    }

    // 2. Process Decoder Stream Cases
    for dcase in &eval_file.decoder_cases {
        let active_tools: Vec<ToolDef> = dcase
            .tools
            .iter()
            .filter_map(|name| tools_by_name.get(name).cloned())
            .collect();

        let mut decoder = StreamDecoder::new();
        for chunk in &dcase.chunks {
            decoder.feed(chunk);
        }

        let decoded = match decoder.finish(&active_tools) {
            Ok(calls) => {
                let formatted = calls
                    .into_iter()
                    .map(|c| {
                        json!({
                            "name": c.function.name,
                            "arguments": serde_json::from_str::<Value>(&c.function.arguments).unwrap_or(Value::Null)
                        })
                    })
                    .collect();
                DecodedResult::Calls { calls: formatted }
            }
            Err(ToolCompactError::UnknownTool(_)) => DecodedResult::Error {
                error: "unknown_tool".to_string(),
            },
            Err(ToolCompactError::InvalidArguments(_, _)) => DecodedResult::Error {
                error: "invalid_arguments".to_string(),
            },
            Err(other) => DecodedResult::Error {
                error: format!("{:?}", other),
            },
        };

        let line = DecoderCaseOutput {
            id: dcase.id.clone(),
            decoded,
        };

        writeln!(out_file, "{}", serde_json::to_string(&line)?)?;
    }

    println!("Evaluation complete! Successfully generated {}", out_path);
    Ok(())
}

fn sample_eval_file() -> EvalFile {
    EvalFile {
        schema_version: Some("v1-sample".to_string()),
        tools: vec![
            ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "create_calendar_event".to_string(),
                    description: Some("Create an event in the user's calendar.".to_string()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start": {"type": "string", "format": "date-time"},
                            "duration_min": {"type": "integer"},
                            "attendees": {"type": "array", "items": {"type": "string"}},
                            "visibility": {"type": "string", "enum": ["public", "private"]}
                        },
                        "required": ["title", "start"]
                    })),
                },
                extra: serde_json::Map::new(),
            },
        ],
        cases: vec![Case {
            id: "ct-001".to_string(),
            tools: vec!["create_calendar_event".to_string()],
            messages: vec![json!({"role": "user", "content": "Book Monday 3pm"})],
            expected: vec![ExpectedCall {
                name: "create_calendar_event".to_string(),
                arguments: json!({
                    "title": "Design review",
                    "start": "2026-10-05T15:00:00+05:30"
                }),
            }],
        }],
        decoder_cases: vec![DecoderCase {
            id: "dc-002".to_string(),
            note: Some("marker split across chunks".to_string()),
            tools: vec!["create_calendar_event".to_string()],
            chunks: vec![
                "<<ca".to_string(),
                "ll create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>".to_string(),
                ">".to_string(),
            ],
        }],
    }
}
