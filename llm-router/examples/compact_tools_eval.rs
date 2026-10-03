use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    tools: Value,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExpectedCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    chunks: Vec<String>,
    #[serde(default)]
    expected: Value,
}

/// Parse tools from the dataset (handles array of ToolDef or key-value map).
fn parse_tools_catalog(tools_val: &Value) -> HashMap<String, ToolDef> {
    let mut catalog = HashMap::new();

    if let Some(arr) = tools_val.as_array() {
        for item in arr {
            if let Ok(tool) = serde_json::from_value::<ToolDef>(item.clone()) {
                catalog.insert(tool.function.name.clone(), tool);
            } else if let Some(obj) = item.as_object() {
                // If nested or shorthand
                if let Some(name) = obj.get("name").and_then(Value::as_str) {
                    let tool = ToolDef::new_function(
                        name,
                        obj.get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        obj.get("parameters").cloned(),
                    );
                    catalog.insert(name.to_string(), tool);
                }
            }
        }
    } else if let Some(map) = tools_val.as_object() {
        for (name, val) in map {
            if let Ok(tool) = serde_json::from_value::<ToolDef>(val.clone()) {
                catalog.insert(name.clone(), tool);
            } else if let Some(obj) = val.as_object() {
                let tool = ToolDef::new_function(
                    name.as_str(),
                    obj.get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    obj.get("parameters").cloned(),
                );
                catalog.insert(name.clone(), tool);
            }
        }
    }

    catalog
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path =
        env::var("EVAL_SET").unwrap_or_else(|_| "compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "out.jsonl".to_string());

    let dataset: EvalDataset = if Path::new(&eval_set_path).exists() {
        let content = fs::read_to_string(&eval_set_path)?;
        serde_json::from_str(&content)?
    } else {
        // Fallback default sample dataset if EVAL_SET is not yet downloaded
        eprintln!(
            "Warning: EVAL_SET ({}) not found. Using default embedded sample.",
            eval_set_path
        );
        let sample = json!({
            "schema_version": "v1",
            "purpose": "Sample tool compaction test",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "create_calendar_event",
                        "description": "Create an event in the user's calendar.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string", "description": "Event title"},
                                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                                "visibility": {"type": "string", "enum": ["public", "private"]}
                            },
                            "required": ["title", "start"]
                        }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "send_email",
                        "description": "Send an email.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "to": {"type": "array", "items": {"type": "string"}},
                                "subject": {"type": "string"},
                                "body": {"type": "string"}
                            },
                            "required": ["to", "subject", "body"]
                        }
                    }
                }
            ],
            "cases": [
                {
                    "id": "ct-001",
                    "tools": ["create_calendar_event", "send_email"],
                    "messages": [{"role": "user", "content": "Book a design review Monday 3pm IST with riya@example.com"}],
                    "expected": [{"name": "create_calendar_event", "arguments": {"title": "Design review", "start": "2026-10-05T15:00:00+05:30", "attendees": ["riya@example.com"]}}]
                }
            ],
            "decoder_cases": [
                {
                    "id": "dc-002",
                    "note": "marker split across stream chunks",
                    "tools": ["create_calendar_event"],
                    "chunks": ["<<ca", "ll create_calendar_event {\"title\":\"Ret", "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>", ">"],
                    "expected": {"calls": [{"name": "create_calendar_event", "arguments": {"title": "Retro", "start": "2026-10-04T10:00:00+05:30"}}]}
                }
            ]
        });
        serde_json::from_value(sample)?
    };

    let tools_catalog = parse_tools_catalog(&dataset.tools);

    let mut out_file = File::create(&out_path)?;

    // 1. Process regular evaluation cases
    for case in &dataset.cases {
        let case_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tools_catalog.get(name).cloned())
            .collect();

        let compact = encode_tools(&case_tools)?;

        // Build compact_request (OpenAI shaped request with system instructions)
        let mut messages = Vec::new();
        let system_content = format!(
            "Today is 2026-10-02, timezone Asia/Kolkata.\n\n{}",
            compact.prompt_section
        );
        messages.push(json!({
            "role": "system",
            "content": system_content
        }));
        for msg in &case.messages {
            messages.push(msg.clone());
        }

        let compact_request = json!({
            "model": "gpt-4o",
            "messages": messages,
            "temperature": 0.0
        });

        // Render expected calls in compact syntax
        let rendered_calls = case
            .expected
            .iter()
            .map(|exp| {
                let args_str =
                    serde_json::to_string(&exp.arguments).unwrap_or_else(|_| "{}".to_string());
                format!("<<call {} {}>>", exp.name, args_str)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Decode back
        let roundtrip_calls = decode_calls(&rendered_calls, &case_tools).unwrap_or_default();

        let out_line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": true,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls
        });

        writeln!(out_file, "{}", serde_json::to_string(&out_line)?)?;
    }

    // 2. Process decoder test cases
    for dc in &dataset.decoder_cases {
        let case_tools: Vec<ToolDef> = dc
            .tools
            .iter()
            .filter_map(|name| tools_catalog.get(name).cloned())
            .collect();

        let mut stream = StreamDecoder::new(case_tools);
        for chunk in &dc.chunks {
            stream.push_chunk(chunk);
        }

        let decoded_obj = match stream.finish() {
            Ok(calls) => json!({ "calls": calls }),
            Err(e) => json!({ "error": e.error_code() }),
        };

        let out_line = json!({
            "id": dc.id,
            "decoded": decoded_obj
        });

        writeln!(out_file, "{}", serde_json::to_string(&out_line)?)?;
    }

    out_file.flush()?;
    println!(
        "Evaluation completed successfully. Output written to: {}",
        out_path
    );

    Ok(())
}
