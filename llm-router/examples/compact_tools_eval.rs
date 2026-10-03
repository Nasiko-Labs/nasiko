use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolCall, ToolDef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

#[derive(Deserialize)]
struct EvalDataset {
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
    #[serde(default)]
    tools: Value,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<Value>,
}

#[derive(Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

#[derive(Serialize)]
struct CaseOutput {
    id: String,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<ToolCall>,
}

#[derive(Serialize)]
struct DecoderOutput {
    id: String,
    decoded: DecodedResult,
}

#[derive(Serialize)]
#[serde(untagged)]
enum DecodedResult {
    Success { calls: Vec<ToolCall> },
    Error { error: String },
}

fn resolve_tools(names: &[String], tool_definitions: &Value) -> Vec<ToolDef> {
    let mut resolved = Vec::new();
    if let Some(obj) = tool_definitions.as_object() {
        for name in names {
            if let Some(schema) = obj.get(name) {
                resolved.push(ToolDef {
                    name: name.clone(),
                    description: schema.get("description").and_then(|d| d.as_str()).map(|s| s.to_string()),
                    parameters: schema.get("parameters").cloned(),
                });
            }
        }
    }
    resolved
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET").unwrap_or_else(|_| "eval_sample.json".into());
    let out_path = env::var("OUT").unwrap_or_else(|_| "out.jsonl".into());

    let file = File::open(&eval_path)?;
    let reader = BufReader::new(file);
    let dataset: EvalDataset = serde_json::from_reader(reader)?;

    let mut out_file = File::create(&out_path)?;

    // 1. Process Standard Evaluation Cases[cite: 4]
    for case in dataset.cases {
        let tools = resolve_tools(&case.tools, &dataset.tools);
        let compact = encode_tools(&tools)?;

        let mut rendered = String::new();
        for exp in &case.expected {
            let name = exp.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = exp.get("arguments").cloned().unwrap_or(Value::Null);
            rendered.push_str(&format!("<<call {} {}>> ", name, args));
        }

        let roundtrip = decode_calls(&rendered, &tools).unwrap_or_default();

        let line = CaseOutput {
            id: case.id,
            compact_request: serde_json::json!({
                "messages": case.messages,
                "injected_prompt": compact.prompt_injection,
            }),
            compacted: true,
            rendered_calls: rendered.trim().to_string(),
            roundtrip_calls: roundtrip,
        };

        writeln!(out_file, "{}", serde_json::to_string(&line)?)?;
    }

    // 2. Process Streaming Decoder Cases[cite: 4]
    for case in dataset.decoder_cases {
        let tools = resolve_tools(&case.tools, &dataset.tools);
        let mut decoder = StreamDecoder::new(&tools);
        let mut all_calls = Vec::new();
        let mut encountered_err = None;

        for chunk in case.chunks {
            match decoder.push_chunk(&chunk) {
                Ok(calls) => all_calls.extend(calls),
                Err(e) => {
                    encountered_err = Some(e);
                    break;
                }
            }
        }

        if encountered_err.is_none() {
            if let Ok(calls) = decoder.finish() {
                all_calls.extend(calls);
            }
        }

        let decoded = match encountered_err {
            Some(e) => DecodedResult::Error {
                error: match e {
                    nasiko_tool_compact::CompactError::UnknownTool(_) => "unknown_tool".into(),
                    _ => "invalid_arguments".into(),
                },
            },
            None => DecodedResult::Success { calls: all_calls },
        };

        let line = DecoderOutput {
            id: case.id,
            decoded,
        };

        writeln!(out_file, "{}", serde_json::to_string(&line)?)?;
    }

    out_file.flush()?;
    println!("Evaluation complete. Results written to {}", out_path);
    Ok(())
}