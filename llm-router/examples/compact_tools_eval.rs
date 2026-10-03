//! Official evaluation runner for nasiko-tool-compact.
//! Reads an evaluation dataset from EVAL_SET (default: /tmp/compact-tools-eval.json)
//! and emits streaming tool call outputs in JSONL format to OUT (default: /tmp/out.jsonl).

use nasiko_tool_compact::{
    compact_tools, StreamingToolCallDecoder,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: Option<String>,
    #[serde(default)]
    encoder_cases: Vec<EncoderCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EncoderCase {
    id: String,
    tools: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    stream: Vec<String>,
    #[allow(dead_code)]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvalOutputLine {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    compact_system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<serde_json::Value>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    println!("Running compact tools evaluation...");
    println!("Reading eval set from: {}", eval_set_path);
    println!("Writing JSONL outputs to: {}", out_path);

    let eval_file: EvalFile = if Path::new(&eval_set_path).exists() {
        let file = File::open(&eval_set_path)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader)?
    } else {
        println!("Warning: {} not found. Generating default sample cases for local smoke test.", eval_set_path);
        generate_sample_eval_file()
    };

    let mut out_file = File::create(&out_path)?;

    // 1. Process encoder cases
    for case in eval_file.encoder_cases {
        let mut functions = Vec::new();
        for t in case.tools {
            if let Some(f) = t.get("function") {
                let func_def: nasiko_tool_compact::types::FunctionDef = serde_json::from_value(f.clone())?;
                functions.push(func_def);
            }
        }

        let compact_repr = compact_tools(&functions);
        let out_line = EvalOutputLine {
            id: case.id,
            compact_system_prompt: Some(compact_repr),
            tool_calls: None,
        };
        writeln!(out_file, "{}", serde_json::to_string(&out_line)?)?;
    }

    // 2. Process decoder cases
    for case in eval_file.decoder_cases {
        let mut decoder = StreamingToolCallDecoder::new();
        let mut emitted_calls = Vec::new();

        for chunk in case.stream {
            let calls = decoder.push_chunk(&chunk)?;
            for call in calls {
                emitted_calls.push(serde_json::json!({
                    "id": call.id,
                    "name": call.name,
                    "arguments": call.arguments
                }));
            }
        }

        let out_line = EvalOutputLine {
            id: case.id,
            compact_system_prompt: None,
            tool_calls: Some(emitted_calls),
        };
        writeln!(out_file, "{}", serde_json::to_string(&out_line)?)?;
    }

    println!("Evaluation complete! Successfully generated {}", out_path);
    Ok(())
}

fn generate_sample_eval_file() -> EvalFile {
    EvalFile {
        schema_version: Some("v1".to_string()),
        encoder_cases: vec![
            EncoderCase {
                id: "smoke_enc_1".to_string(),
                tools: vec![
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": "lookup_user",
                            "description": "Finds a user by ID",
                            "parameters": {
                                "type": "object",
                                "properties": {
                                    "user_id": { "type": "string" }
                                },
                                "required": ["user_id"]
                            }
                        }
                    })
                ],
            }
        ],
        decoder_cases: vec![
            DecoderCase {
                id: "smoke_dec_1".to_string(),
                stream: vec![
                    "<<call:lookup_user{\"user_".to_string(),
                    "id\": \"usr_42\"}>>".to_string()
                ],
                note: None,
            }
        ],
    }
}
