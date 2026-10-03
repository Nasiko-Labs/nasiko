use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolCall, ToolDef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs::File;
use std::io::{BufReader, Write};

#[derive(Deserialize)]
struct EvalDataset {
    tools: Option<Vec<ToolDef>>,
    cases: Option<Vec<Case>>,
    decoder_cases: Option<Vec<DecoderCase>>,
}

#[derive(Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[allow(dead_code)]
    expected: Option<Vec<ExpectedCall>>,
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
    decoded: DecoderDecoded,
}

#[derive(Serialize)]
struct DecoderDecoded {
    #[serde(skip_serializing_if = "Option::is_none")]
    calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let file = File::open(&eval_set_path)?;
    let dataset: EvalDataset = serde_json::from_reader(BufReader::new(file))?;
    let catalog = dataset.tools.unwrap_or_default();

    let mut out_file = File::create(&out_path)?;

    if let Some(cases) = dataset.cases {
        for case in cases {
            let active_tools: Vec<ToolDef> = catalog
                .iter()
                .filter(|t| case.tools.contains(&t.function.name))
                .cloned()
                .collect();

            let compact_res = encode_tools(&active_tools);
            let (compacted, defs, instructions) = match compact_res {
                Ok(c) => (true, c.compact_definitions, c.instructions),
                Err(_) => (false, String::new(), String::new()),
            };

            let mut injected_messages = case.messages.clone();
            if compacted {
                injected_messages.insert(
                    0,
                    serde_json::json!({
                        "role": "system",
                        "content": format!("Available Tools:\n{}\n\n{}", defs, instructions)
                    }),
                );
            }

            let compact_request = serde_json::json!({
                "model": "router-eval",
                "messages": injected_messages,
            });

            let mut rendered_calls = String::new();
            if let Some(expected_list) = &case.expected {
                for exp in expected_list {
                    rendered_calls.push_str(&format!(
                        "<<call {} {}>> ",
                        exp.name,
                        serde_json::to_string(&exp.arguments).unwrap_or_default()
                    ));
                }
            }

            let roundtrip_calls = decode_calls(&rendered_calls, &active_tools).unwrap_or_default();

            let row = CaseOutput {
                id: case.id,
                compact_request,
                compacted,
                rendered_calls: rendered_calls.trim().to_string(),
                roundtrip_calls,
            };

            writeln!(out_file, "{}", serde_json::to_string(&row)?)?;
        }
    }

    if let Some(decoder_cases) = dataset.decoder_cases {
        for dc in decoder_cases {
            let active_tools: Vec<ToolDef> = catalog
                .iter()
                .filter(|t| dc.tools.contains(&t.function.name))
                .cloned()
                .collect();

            let mut decoder = StreamDecoder::new(active_tools);
            for chunk in &dc.chunks {
                decoder.push_chunk(chunk);
            }

            let decoded = match decoder.finish() {
                Ok(calls) => DecoderDecoded {
                    calls: Some(calls),
                    error: None,
                },
                Err(err) => DecoderDecoded {
                    calls: None,
                    error: Some(match err {
                        nasiko_tool_compact::CompactError::UnknownTool(_) => "unknown_tool".to_string(),
                        nasiko_tool_compact::CompactError::MissingRequired(_)
                        | nasiko_tool_compact::CompactError::InvalidArgument { .. } => "invalid_arguments".to_string(),
                        _ => "invalid_arguments".to_string(),
                    }),
                },
            };

            let row = DecoderOutput {
                id: dc.id,
                decoded,
            };

            writeln!(out_file, "{}", serde_json::to_string(&row)?)?;
        }
    }

    Ok(())
}
