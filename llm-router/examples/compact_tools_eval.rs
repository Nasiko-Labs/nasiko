//! P1 Evaluation Harness: compact tool schemas and call decoding.
//!
//! # Usage
//! ```sh
//! EVAL_SET=/path/to/eval-set.json OUT=/path/to/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Modes
//! - **Default (Offline)**: Deterministic, no network, no LLM calls.
//!   Evaluates tool compaction, round-trip decoding, and incremental streaming.
//! - **Output Format**: Writes one JSONL line per case to `OUT`.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Input evaluation set file structure (`compact-tools-eval@v1-sample`).
#[derive(Debug, Clone, Deserialize)]
pub struct EvalSet {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub cases: Vec<TestCase>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

/// A regular evaluation case testing compaction and round-trip decoding.
#[derive(Debug, Clone, Deserialize)]
pub struct TestCase {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub expected: Vec<ExpectedCall>,
    #[serde(default)]
    pub r#match: Option<Value>,
}

/// An expected tool call with parsed arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedCall {
    pub name: String,
    pub arguments: Value,
}

/// A decoder evaluation case testing streaming chunks and error detection.
#[derive(Debug, Clone, Deserialize)]
pub struct DecoderCase {
    pub id: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub chunks: Vec<String>,
    #[serde(default)]
    pub expected: Option<Value>,
}

/// Output JSONL line for a regular case.
#[derive(Debug, Clone, Serialize)]
pub struct CaseOutput {
    pub id: String,
    pub compact_request: Value,
    pub compacted: bool,
    pub rendered_calls: String,
    pub roundtrip_calls: Vec<ExpectedCall>,
}

/// Output JSONL line for a decoder case.
#[derive(Debug, Clone, Serialize)]
pub struct DecoderCaseOutput {
    pub id: String,
    pub decoded: Value,
}

/// Fixed reference time required by the official P1 specification (line 119).
pub const SYSTEM_REFERENCE_TIME: &str = "Today is 2026-10-02, timezone Asia/Kolkata.";

/// Run the evaluation harness on the given input and output paths.
pub fn run_evaluation(eval_set_path: &Path, out_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(eval_set_path)
        .map_err(|e| format!("failed to open EVAL_SET at {}: {e}", eval_set_path.display()))?;
    let reader = BufReader::new(file);
    let eval_set: EvalSet = serde_json::from_reader(reader)
        .map_err(|e| format!("failed to parse EVAL_SET JSON: {e}"))?;

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let out_file = File::create(out_path)
        .map_err(|e| format!("failed to create OUT file at {}: {e}", out_path.display()))?;
    let mut writer = BufWriter::new(out_file);

    // 1. Process regular cases
    for case in &eval_set.cases {
        let output = process_case(case, &eval_set.tools);
        let line = serde_json::to_string(&output)?;
        writeln!(writer, "{line}")?;
    }

    // 2. Process streaming decoder cases
    for dec_case in &eval_set.decoder_cases {
        let output = process_decoder_case(dec_case, &eval_set.tools);
        let line = serde_json::to_string(&output)?;
        writeln!(writer, "{line}")?;
    }

    writer.flush()?;
    Ok(())
}

/// Process a single compaction test case.
pub fn process_case(case: &TestCase, all_tools: &[ToolDef]) -> CaseOutput {
    // Resolve tools referenced by name in this case
    let tool_names: HashSet<&str> = case.tools.iter().map(String::as_str).collect();
    let case_tools: Vec<ToolDef> = all_tools
        .iter()
        .filter(|t| tool_names.contains(t.function.name.as_str()))
        .cloned()
        .collect();

    // Check for unsupported schema features that require compaction bypass (line 155)
    let has_unsupported = has_unsupported_schema_features(&case_tools);

    let (compact_request, compacted) = if has_unsupported {
        // Bypass compaction: keep native uncompacted request (compacted: false)
        let req = json!({
            "messages": case.messages,
            "tools": case_tools,
        });
        (req, false)
    } else {
        match encode_tools(&case_tools) {
            Ok(compact) => {
                let system_prompt = format!("{SYSTEM_REFERENCE_TIME}\n\n{}", compact.prompt_text);
                let mut messages = Vec::with_capacity(case.messages.len() + 1);
                messages.push(json!({
                    "role": "system",
                    "content": system_prompt,
                }));
                messages.extend(case.messages.clone());

                let req = json!({
                    "messages": messages,
                });
                (req, true)
            }
            Err(_) => {
                // Fail-safe bypass if schema encoding encounters unsupported structures
                let req = json!({
                    "messages": case.messages,
                    "tools": case_tools,
                });
                (req, false)
            }
        }
    };

    // Render expected calls in compact syntax: <<call name {json args}>>
    let mut rendered_parts = Vec::with_capacity(case.expected.len());
    for exp in &case.expected {
        let args_str = serde_json::to_string(&exp.arguments).unwrap_or_else(|_| "{}".to_string());
        rendered_parts.push(format!("<<call {} {}>>", exp.name, args_str));
    }
    let rendered_calls = rendered_parts.join(" ");

    // Decode rendered calls back to standard tool calls
    let roundtrip_calls = if rendered_calls.is_empty() {
        Vec::new()
    } else {
        match decode_calls(&rendered_calls, &case_tools) {
            Ok(calls) => calls
                .into_iter()
                .map(|c| {
                    let parsed_args = serde_json::from_str(&c.function.arguments)
                        .unwrap_or(Value::Null);
                    ExpectedCall {
                        name: c.function.name,
                        arguments: parsed_args,
                    }
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    };

    CaseOutput {
        id: case.id.clone(),
        compact_request,
        compacted,
        rendered_calls,
        roundtrip_calls,
    }
}

/// Process a streaming decoder case.
pub fn process_decoder_case(dec_case: &DecoderCase, all_tools: &[ToolDef]) -> DecoderCaseOutput {
    let tool_names: HashSet<&str> = dec_case.tools.iter().map(String::as_str).collect();
    let case_tools: Vec<ToolDef> = all_tools
        .iter()
        .filter(|t| tool_names.contains(t.function.name.as_str()))
        .cloned()
        .collect();

    let decoded = match StreamDecoder::decode_chunks(&case_tools, &dec_case.chunks) {
        Ok(calls) => {
            let call_outputs: Vec<ExpectedCall> = calls
                .into_iter()
                .map(|c| {
                    let parsed_args = serde_json::from_str(&c.function.arguments)
                        .unwrap_or(Value::Null);
                    ExpectedCall {
                        name: c.function.name,
                        arguments: parsed_args,
                    }
                })
                .collect();
            json!({ "calls": call_outputs })
        }
        Err(err) => {
            json!({ "error": err.as_code() })
        }
    };

    DecoderCaseOutput {
        id: dec_case.id.clone(),
        decoded,
    }
}

/// Detect unsupported JSON Schema keywords that mandate compaction bypass (e.g. oneOf, anyOf, allOf, $ref).
fn has_unsupported_schema_features(tools: &[ToolDef]) -> bool {
    for tool in tools {
        if let Some(params) = &tool.function.parameters
            && contains_unsupported_keywords(params)
        {
            return true;
        }
    }
    false
}

fn contains_unsupported_keywords(val: &Value) -> bool {
    match val {
        Value::Object(map) => {
            for (key, v) in map {
                if key == "oneOf" || key == "anyOf" || key == "allOf" || key == "$ref" {
                    return true;
                }
                if contains_unsupported_keywords(v) {
                    return true;
                }
            }
            false
        }
        Value::Array(arr) => arr.iter().any(contains_unsupported_keywords),
        _ => false,
    }
}

fn main() {
    let eval_set_path = std::env::var("EVAL_SET").unwrap_or_default();
    let out_path = std::env::var("OUT").unwrap_or_default();

    if eval_set_path.is_empty() || out_path.is_empty() {
        eprintln!("usage: EVAL_SET=<input_path> OUT=<output_path> cargo run --release -p nasiko-llm-router --example compact_tools_eval");
        std::process::exit(1);
    }

    if let Err(e) = run_evaluation(Path::new(&eval_set_path), Path::new(&out_path)) {
        eprintln!("evaluation failed: {e}");
        std::process::exit(1);
    }

    println!("Evaluation completed successfully. Results written to {out_path}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(nasiko_tool_compact::FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create calendar event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string" }
                    },
                    "required": ["title", "start"]
                })),
            }),
            ToolDef::new(nasiko_tool_compact::FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" }
                    },
                    "required": ["to", "subject"]
                })),
            }),
        ]
    }

    #[test]
    fn test_process_case_compacted_success() {
        let tools = test_tools();
        let case = TestCase {
            id: "ct-001".into(),
            tools: vec!["create_calendar_event".into()],
            messages: vec![json!({"role": "user", "content": "Book a review"})],
            expected: vec![ExpectedCall {
                name: "create_calendar_event".into(),
                arguments: json!({"title": "Review", "start": "2026-10-05T15:00:00+05:30"}),
            }],
            r#match: None,
        };

        let out = process_case(&case, &tools);
        assert_eq!(out.id, "ct-001");
        assert!(out.compacted);
        assert!(out.rendered_calls.contains("<<call create_calendar_event"));
        assert_eq!(out.roundtrip_calls.len(), 1);
        assert_eq!(out.roundtrip_calls[0].name, "create_calendar_event");
        assert_eq!(out.roundtrip_calls[0].arguments["title"], "Review");
    }

    #[test]
    fn test_process_case_unsupported_schema_bypassed() {
        let mut tools = test_tools();
        tools.push(ToolDef::new(nasiko_tool_compact::FunctionDef {
            name: "unsupported_tool".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "val": {
                        "oneOf": [{"type": "string"}, {"type": "integer"}]
                    }
                }
            })),
        }));

        let case = TestCase {
            id: "ct-unsupported".into(),
            tools: vec!["unsupported_tool".into()],
            messages: vec![json!({"role": "user", "content": "hello"})],
            expected: vec![],
            r#match: None,
        };

        let out = process_case(&case, &tools);
        assert_eq!(out.id, "ct-unsupported");
        assert!(!out.compacted); // Bypassed!
        assert!(out.compact_request.get("tools").is_some());
    }

    #[test]
    fn test_process_decoder_case_success() {
        let tools = test_tools();
        let dec_case = DecoderCase {
            id: "dc-002".into(),
            note: Some("split chunks".into()),
            tools: vec!["create_calendar_event".into()],
            chunks: vec![
                "<<ca".into(),
                "ll create_calendar_event {\"title\":\"Ret".into(),
                "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>".into(),
                ">".into(),
            ],
            expected: None,
        };

        let out = process_decoder_case(&dec_case, &tools);
        assert_eq!(out.id, "dc-002");
        let calls = out.decoded.get("calls").and_then(Value::as_array).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "create_calendar_event");
        assert_eq!(calls[0]["arguments"]["title"], "Retro");
    }

    #[test]
    fn test_process_decoder_case_error() {
        let tools = test_tools();
        let dec_case = DecoderCase {
            id: "dc-err".into(),
            note: Some("unknown tool".into()),
            tools: vec!["create_calendar_event".into()],
            chunks: vec!["<<call unknown_fn {}>>".into()],
            expected: None,
        };

        let out = process_decoder_case(&dec_case, &tools);
        assert_eq!(out.id, "dc-err");
        assert_eq!(out.decoded["error"], "unknown_tool");
    }
}
