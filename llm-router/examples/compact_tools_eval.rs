use nasiko_tool_compact::{
    decode_calls, encode_tools, schema_from_def, StreamDecoder, StreamItem, ToolCompactError,
    ToolDef, ToolRegistry,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use tiktoken_rs::o200k_base;

#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<EvalCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
}

fn main() {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    println!("===========================================================");
    println!("   NASIKO COMPACT TOOLS EVALUATION HARNESS (TRACK P1)");
    println!("===========================================================");
    println!("Reading EVAL_SET: {}", eval_set_path);
    println!("Writing OUT:      {}\n", out_path);

    if !Path::new(&eval_set_path).exists() {
        eprintln!(
            "EVAL_SET file not found at '{}'. Please download it via:\n\
             curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json",
            eval_set_path
        );
        std::process::exit(1);
    }

    let file_content = fs::read_to_string(&eval_set_path).expect("Failed to read EVAL_SET file");
    let dataset: EvalDataset = serde_json::from_str(&file_content).expect("Failed to parse EVAL_SET JSON");

    let tool_map: HashMap<String, ToolDef> = dataset
        .tools
        .into_iter()
        .map(|t| (t.function.name.clone(), t))
        .collect();

    let out_file = File::create(&out_path).expect("Failed to create OUT file");
    let mut writer = BufWriter::new(out_file);

    let bpe = o200k_base().ok();

    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // Fixed reference time required by prompt brief
    let ref_time = "Reference time: Today is 2026-10-02, timezone Asia/Kolkata.";

    // 1. Process Cases
    for case in &dataset.cases {
        let selected_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| tool_map.get(name).cloned())
            .collect();

        // Baseline token calculation for request with standard tools
        let baseline_req = json!({
            "messages": case.messages,
            "tools": selected_tools,
        });
        let baseline_str = baseline_req.to_string();
        let baseline_tokens = bpe
            .as_ref()
            .map(|b| b.encode_with_special_tokens(&baseline_str).len())
            .unwrap_or(0);
        total_baseline_tokens += baseline_tokens;

        match encode_tools(&selected_tools) {
            Ok(compact_tools) => {
                // Build system prompt with compact definitions, call instructions, and reference time
                let system_prompt = format!(
                    "{}\n{}",
                    compact_tools.prompt(),
                    ref_time
                );

                let mut compact_messages = vec![json!({
                    "role": "system",
                    "content": system_prompt,
                })];
                compact_messages.extend(case.messages.clone());

                let compact_req = json!({
                    "messages": compact_messages,
                });

                let compact_str = compact_req.to_string();
                let compact_tokens = bpe
                    .as_ref()
                    .map(|b| b.encode_with_special_tokens(&compact_str).len())
                    .unwrap_or(0);
                total_compact_tokens += compact_tokens;

                // Render expected calls into compact format string
                let rendered_calls_list: Vec<String> = case
                    .expected
                    .iter()
                    .map(|exp| {
                        let name = exp.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let args = exp.get("arguments").cloned().unwrap_or(json!({}));
                        format!("<<call {} {}>>", name, args.to_string())
                    })
                    .collect();
                let rendered_calls = rendered_calls_list.join("\n");

                // Roundtrip decode rendered calls back to ToolCalls
                let roundtrip_calls = match decode_calls(&rendered_calls, &selected_tools) {
                    Ok(calls) => calls
                        .into_iter()
                        .map(|c| {
                            let parsed_args: Value =
                                serde_json::from_str(&c.function.arguments).unwrap_or(json!({}));
                            json!({
                                "name": c.function.name,
                                "arguments": parsed_args,
                            })
                        })
                        .collect::<Vec<Value>>(),
                    Err(_) => vec![],
                };

                let mut out_line = json!({
                    "id": case.id,
                    "compact_request": compact_req,
                    "compacted": true,
                    "rendered_calls": rendered_calls,
                    "roundtrip_calls": roundtrip_calls,
                });

                // Check optional live mode env vars
                if let (Ok(base_url), Ok(model)) = (env::var("PROVIDER_BASE_URL"), env::var("MODEL")) {
                    if let Some((raw_output, live_calls)) = execute_live_run(&base_url, &model, &compact_req, &selected_tools) {
                        if let Some(obj) = out_line.as_object_mut() {
                            obj.insert("raw_output".to_string(), Value::String(raw_output));
                            obj.insert("live_calls".to_string(), live_calls);
                        }
                    }
                }

                writeln!(writer, "{}", out_line.to_string()).expect("Failed to write line to OUT");
            }
            Err(_) => {
                // Bypassed compaction for unsupported schemas
                total_compact_tokens += baseline_tokens;
                let out_line = json!({
                    "id": case.id,
                    "compact_request": baseline_req,
                    "compacted": false,
                    "rendered_calls": "",
                    "roundtrip_calls": [],
                });
                writeln!(writer, "{}", out_line.to_string()).expect("Failed to write line to OUT");
            }
        }
    }

    // 2. Process Decoder Cases
    for dcase in &dataset.decoder_cases {
        let selected_tools: Vec<ToolDef> = dcase
            .tools
            .iter()
            .filter_map(|name| tool_map.get(name).cloned())
            .collect();

        let mut registry = ToolRegistry::new();
        for def in &selected_tools {
            if let Ok(schema) = schema_from_def(def) {
                registry.register(schema);
            }
        }

        let mut decoder = StreamDecoder::new(registry);
        let mut calls = Vec::new();
        let mut error_val: Option<String> = None;

        for chunk in &dcase.chunks {
            let items = decoder.feed(chunk);
            for item in items {
                match item {
                    StreamItem::Call { name, arguments } => {
                        calls.push(json!({
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                    StreamItem::Error(e) => {
                        let err_str = match e {
                            ToolCompactError::UnknownTool(_) => "unknown_tool",
                            ToolCompactError::InvalidArguments { .. } => "invalid_arguments",
                            ToolCompactError::Malformed(_) => "malformed",
                            ToolCompactError::SchemaError(_) => "invalid_arguments",
                            ToolCompactError::SerializationError(_) => "malformed",
                        };
                        error_val = Some(err_str.to_string());
                        break;
                    }
                    StreamItem::Text(_) => {}
                }
            }
            if error_val.is_some() {
                break;
            }
        }

        if error_val.is_none() {
            let finish_items = decoder.finish();
            for item in finish_items {
                match item {
                    StreamItem::Call { name, arguments } => {
                        calls.push(json!({
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                    StreamItem::Error(e) => {
                        let err_str = match e {
                            ToolCompactError::UnknownTool(_) => "unknown_tool",
                            ToolCompactError::InvalidArguments { .. } => "invalid_arguments",
                            ToolCompactError::Malformed(_) => "malformed",
                            _ => "invalid_arguments",
                        };
                        error_val = Some(err_str.to_string());
                        break;
                    }
                    StreamItem::Text(_) => {}
                }
            }
        }

        let decoded_result = match error_val {
            Some(err) => json!({ "error": err }),
            None => json!({ "calls": calls }),
        };

        let out_line = json!({
            "id": dcase.id,
            "decoded": decoded_result,
        });

        writeln!(writer, "{}", out_line.to_string()).expect("Failed to write line to OUT");
    }

    writer.flush().expect("Failed to flush OUT writer");

    println!("Completed processing evaluation dataset.");
    if total_baseline_tokens > 0 {
        let reduction = (1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)) * 100.0;
        println!("-----------------------------------------------------------");
        println!("Baseline Total Tokens: {}", total_baseline_tokens);
        println!("Compact Total Tokens:  {}", total_compact_tokens);
        println!("Token Savings:         {:.2}%", reduction);
        println!("-----------------------------------------------------------");
    }
}

fn execute_live_run(base_url: &str, model: &str, compact_req: &Value, tools: &[ToolDef]) -> Option<(String, Value)> {
    let client = ureq::AgentBuilder::new().build();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let mut req_body = compact_req.clone();
    if let Some(obj) = req_body.as_object_mut() {
        obj.insert("model".to_string(), json!(model));
        obj.insert("temperature".to_string(), json!(0));
    }

    match client.post(&url).send_json(req_body) {
        Ok(resp) => {
            if let Ok(json_resp) = resp.into_json::<Value>() {
                let content = json_resp
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();

                let live_calls = match decode_calls(&content, tools) {
                    Ok(calls) => {
                        let parsed = calls
                            .into_iter()
                            .map(|c| {
                                let parsed_args: Value =
                                    serde_json::from_str(&c.function.arguments).unwrap_or(json!({}));
                                json!({
                                    "name": c.function.name,
                                    "arguments": parsed_args,
                                })
                            })
                            .collect::<Vec<Value>>();
                        json!({ "calls": parsed })
                    }
                    Err(e) => {
                        let err_str = match e {
                            ToolCompactError::UnknownTool(_) => "unknown_tool",
                            ToolCompactError::InvalidArguments { .. } => "invalid_arguments",
                            ToolCompactError::Malformed(_) => "malformed",
                            _ => "invalid_arguments",
                        };
                        json!({ "error": err_str })
                    }
                };

                Some((content, live_calls))
            } else {
                None
            }
        }
        Err(_) => None,
    }
}
