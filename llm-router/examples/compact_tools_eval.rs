use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};

use nasiko_tool_compact::{StreamDecoder, ToolDef, decode_calls, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    schema_version: String,
    #[serde(default)]
    purpose: String,
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<TestCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TestCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
    #[serde(default)]
    match_spec: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[serde(default)]
    note: Option<String>,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: Value,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path =
        env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let provider_base_url = env::var("PROVIDER_BASE_URL").ok();
    let model = env::var("MODEL").ok();
    let api_key = env::var("OPENAI_API_KEY").or_else(|_| env::var("API_KEY")).ok();

    let is_live_mode = provider_base_url.is_some() && model.is_some();

    eprintln!("Evaluating compact tools with:");
    eprintln!("  EVAL_SET: {}", eval_set_path);
    eprintln!("  OUT:      {}", out_path);
    if is_live_mode {
        eprintln!(
            "  Live Mode: endpoint={}, model={}",
            provider_base_url.as_deref().unwrap(),
            model.as_deref().unwrap()
        );
    } else {
        eprintln!("  Mode: Offline (deterministic)");
    }

    let file = File::open(&eval_set_path)
        .map_err(|e| format!("failed to open EVAL_SET at '{}': {}", eval_set_path, e))?;
    let dataset: EvalDataset = serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("failed to parse EVAL_SET JSON: {}", e))?;

    let mut out_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&out_path)
        .map_err(|e| format!("failed to create OUT file at '{}': {}", out_path, e))?;

    let bpe = tiktoken_rs::o200k_base().ok();
    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    let http_client = if is_live_mode {
        Some(reqwest::Client::new())
    } else {
        None
    };

    // ── 1. Process standard evaluation cases ─────────────────────────────────
    for tc in &dataset.cases {
        let active_tools: Vec<ToolDef> = dataset
            .tools
            .iter()
            .filter(|t| tc.tools.contains(&t.function.name))
            .cloned()
            .collect();

        // Encode tools compactly
        let compact_tools = encode_tools(&active_tools)?;

        // Prepare compact messages
        let mut compact_messages = Vec::new();
        let system_prompt = if is_live_mode {
            format!("{}\nToday is 2026-10-02 (Asia/Kolkata).", compact_tools.prompt)
        } else {
            compact_tools.prompt.clone()
        };
        compact_messages.push(json!({
            "role": "system",
            "content": system_prompt
        }));

        for msg in &tc.messages {
            compact_messages.push(msg.clone());
        }

        let compact_request = json!({
            "messages": compact_messages
        });

        // Compute tokens for baseline vs compact request
        if let Some(ref tokenizer) = bpe {
            let baseline_request = json!({
                "messages": tc.messages,
                "tools": active_tools
            });
            let baseline_str = serde_json::to_string(&baseline_request).unwrap_or_default();
            let compact_str = serde_json::to_string(&compact_request).unwrap_or_default();

            let base_tokens = tokenizer.encode_with_special_tokens(&baseline_str).len();
            let comp_tokens = tokenizer.encode_with_special_tokens(&compact_str).len();

            total_baseline_tokens += base_tokens;
            total_compact_tokens += comp_tokens;
        }

        // Render expected calls in <<call ...>> syntax
        let mut rendered_parts = Vec::new();
        for exp in &tc.expected {
            let args_json = serde_json::to_string(&exp.arguments)?;
            rendered_parts.push(format!("<<call {} {}>>", exp.name, args_json));
        }
        let rendered_calls = rendered_parts.join("\n");

        // Decode back rendered_calls (roundtrip)
        let roundtrip_tool_calls = decode_calls(&rendered_calls, &active_tools)?;
        let roundtrip_calls: Vec<Value> = roundtrip_tool_calls
            .iter()
            .map(|c| {
                let parsed_args: Value =
                    serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                json!({
                    "name": c.function.name,
                    "arguments": parsed_args
                })
            })
            .collect();

        let mut line_obj = json!({
            "id": tc.id,
            "compact_request": compact_request,
            "compacted": true,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls
        });

        // Live mode execution if configured
        if let (Some(client), Some(base_url), Some(m)) =
            (&http_client, &provider_base_url, &model)
        {
            let mut req_body = compact_request.clone();
            if let Value::Object(ref mut map) = req_body {
                map.insert("model".to_string(), Value::String(m.clone()));
                map.insert("temperature".to_string(), json!(0.0));
            }

            let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut req_builder = client.post(&endpoint).json(&req_body);
            if let Some(ref k) = api_key {
                req_builder = req_builder.bearer_auth(k);
            }

            match req_builder.send().await {
                Ok(resp) => {
                    let resp_json: Value = resp.json().await.unwrap_or(Value::Null);
                    let raw_text = resp_json
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();

                    let live_calls = match decode_calls(&raw_text, &active_tools) {
                        Ok(calls) => {
                            let calls_val: Vec<Value> = calls
                                .iter()
                                .map(|c| {
                                    let parsed_args: Value =
                                        serde_json::from_str(&c.function.arguments)
                                            .unwrap_or(Value::Null);
                                    json!({
                                        "name": c.function.name,
                                        "arguments": parsed_args
                                    })
                                })
                                .collect();
                            json!(calls_val)
                        }
                        Err(e) => {
                            json!({ "error": e.error_code() })
                        }
                    };

                    if let Value::Object(ref mut map) = line_obj {
                        map.insert("raw_output".to_string(), Value::String(raw_text));
                        map.insert("live_calls".to_string(), live_calls);
                    }
                }
                Err(e) => {
                    eprintln!("Live request failed for {}: {}", tc.id, e);
                }
            }
        }

        let line_str = serde_json::to_string(&line_obj)?;
        writeln!(out_file, "{}", line_str)?;
    }

    // ── 2. Process streaming decoder test cases ──────────────────────────────
    for dc in &dataset.decoder_cases {
        let active_tools: Vec<ToolDef> = dataset
            .tools
            .iter()
            .filter(|t| dc.tools.contains(&t.function.name))
            .cloned()
            .collect();

        let mut decoder = StreamDecoder::new(&active_tools);
        let mut err_code = None;

        for chunk in &dc.chunks {
            if let Err(e) = decoder.feed(chunk) {
                err_code = Some(e.error_code().to_string());
                break;
            }
        }

        let decoded_obj = if let Some(code) = err_code {
            json!({ "error": code })
        } else {
            match decoder.finish() {
                Ok(calls) => {
                    let calls_val: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            let parsed: Value =
                                serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                            json!({
                                "name": c.function.name,
                                "arguments": parsed
                            })
                        })
                        .collect();
                    json!({ "calls": calls_val })
                }
                Err(e) => {
                    json!({ "error": e.error_code() })
                }
            }
        };

        let line_obj = json!({
            "id": dc.id,
            "decoded": decoded_obj
        });

        let line_str = serde_json::to_string(&line_obj)?;
        writeln!(out_file, "{}", line_str)?;
    }

    out_file.flush()?;

    if total_baseline_tokens > 0 {
        let reduction =
            1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64);
        eprintln!("Token Reduction Statistics (o200k_base):");
        eprintln!("  Baseline tokens: {}", total_baseline_tokens);
        eprintln!("  Compact tokens:  {}", total_compact_tokens);
        eprintln!("  Reduction:       {:.2}%", reduction * 100.0);
    }

    eprintln!("Successfully evaluated all cases to {}", out_path);
    Ok(())
}
