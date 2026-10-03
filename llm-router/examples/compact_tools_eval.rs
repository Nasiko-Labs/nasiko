//! Compact Tool Schemas Evaluation Runner.
//!
//! Run contract:
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```

use nasiko_tool_compact::{decode_calls, encode_tools, StreamDecoder, ToolDef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use tiktoken_rs::o200k_base;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TestCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
    #[serde(default)]
    match_spec: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let provider_base_url = env::var("PROVIDER_BASE_URL").ok();
    let live_model = env::var("MODEL").ok();

    println!("============================================================");
    println!("🚀 Nasiko Compact Tools Evaluation (TOON Engine)");
    println!("   EVAL_SET: {}", eval_set_path);
    println!("   OUT:      {}", out_path);
    if let Some(model) = &live_model {
        println!("   LIVE MODE: Enabled with model {}", model);
    } else {
        println!("   MODE:     Offline Deterministic");
    }
    println!("============================================================");

    let content = fs::read_to_string(&eval_set_path)
        .map_err(|e| format!("failed to read EVAL_SET from '{}': {}", eval_set_path, e))?;
    let dataset: EvalDataset = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse EVAL_SET json: {}", e))?;

    let out_file = File::create(&out_path)
        .map_err(|e| format!("failed to create OUT file at '{}': {}", out_path, e))?;
    let mut writer = BufWriter::new(out_file);

    let bpe = o200k_base().ok();
    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;

    // ─── Part 1: Process Evaluation Cases ────────────────────────────────────
    for case in &dataset.cases {
        let case_tools: Vec<ToolDef> = dataset
            .tools
            .iter()
            .filter(|t| case.tools.contains(&t.function.name))
            .cloned()
            .collect();

        let (compact_request, rendered_calls, roundtrip_calls) = if !case_tools.is_empty() {
            let compact = encode_tools(&case_tools)?;

            // Build system message with fixed reference time + compact tool prompt
            let system_prompt = format!(
                "Today is 2026-10-02, timezone Asia/Kolkata.\n\n{}",
                compact.prompt_block()
            );

            let mut messages = vec![json!({
                "role": "system",
                "content": system_prompt
            })];

            for msg in &case.messages {
                messages.push(msg.clone());
            }

            let compact_req = json!({
                "model": live_model.clone().unwrap_or_else(|| "gpt-4o".to_string()),
                "messages": messages,
                "temperature": 0.0
            });

            // Baseline request for token comparison
            let baseline_req = json!({
                "model": "gpt-4o",
                "messages": case.messages,
                "tools": case_tools
            });

            if let Some(tokenizer) = &bpe {
                let baseline_str = serde_json::to_string(&baseline_req).unwrap_or_default();
                let compact_str = serde_json::to_string(&compact_req).unwrap_or_default();
                total_baseline_tokens += tokenizer.encode_with_special_tokens(&baseline_str).len();
                total_compact_tokens += tokenizer.encode_with_special_tokens(&compact_str).len();
            }

            // Render expected calls
            let mut rendered = Vec::new();
            for exp in &case.expected {
                let args_json = serde_json::to_string(&exp.arguments)?;
                rendered.push(format!("<<call {} {}>>", exp.name, args_json));
            }
            let rendered_calls_str = rendered.join(" ");

            // Roundtrip calls through decode_calls
            let decoded_calls = if !rendered_calls_str.is_empty() {
                decode_calls(&rendered_calls_str, &case_tools)?
            } else {
                Vec::new()
            };

            let roundtrip: Vec<Value> = decoded_calls
                .into_iter()
                .map(|c| {
                    let parsed_args: Value =
                        serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                    json!({
                        "name": c.function.name,
                        "arguments": parsed_args
                    })
                })
                .collect();

            (compact_req, rendered_calls_str, roundtrip)
        } else {
            let mut messages = vec![json!({
                "role": "system",
                "content": "Today is 2026-10-02, timezone Asia/Kolkata."
            })];
            for msg in &case.messages {
                messages.push(msg.clone());
            }
            let compact_req = json!({
                "model": live_model.clone().unwrap_or_else(|| "gpt-4o".to_string()),
                "messages": messages,
                "temperature": 0.0
            });

            (compact_req, String::new(), Vec::new())
        };

        let mut out_line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": true,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });

        // Handle live execution if configured
        if let (Some(base_url), Some(_model_name)) = (&provider_base_url, &live_model) {
            let client = reqwest::Client::new();
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut req_builder = client.post(&url).json(&out_line["compact_request"]);

            if let Ok(key) = env::var("API_KEY")
                .or_else(|_| env::var("PROVIDER_API_KEY"))
                .or_else(|_| env::var("BEDROCK_API_KEY"))
                .or_else(|_| env::var("OPENAI_API_KEY"))
                .or_else(|_| env::var("OPENROUTER_API_KEY"))
                .or_else(|_| env::var("GROQ_API_KEY"))
            {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
            }

            let resp = req_builder.send().await;

            match resp {
                Ok(res) => {
                    let status = res.status();
                    if let Ok(json_resp) = res.json::<Value>().await {
                        if !status.is_success() {
                            eprintln!("⚠️  Live API returned error HTTP {}: {}", status, json_resp);
                        }
                        let raw_text = json_resp["choices"][0]["message"]["content"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();

                        let live_decoded = decode_calls(&raw_text, &case_tools);
                        let live_val = match live_decoded {
                            Ok(calls) => {
                                let c_list: Vec<Value> = calls
                                    .into_iter()
                                    .map(|c| {
                                        let a: Value = serde_json::from_str(&c.function.arguments)
                                            .unwrap_or(Value::Null);
                                        json!({ "name": c.function.name, "arguments": a })
                                    })
                                    .collect();
                                json!({ "calls": c_list })
                            }
                            Err(e) => json!({ "error": e.eval_error_kind() }),
                        };

                        out_line["raw_output"] = json!(raw_text);
                        out_line["live_calls"] = live_val;
                    }
                }
                Err(e) => {
                    eprintln!("Live request failed for {}: {}", case.id, e);
                }
            }
        }

        writeln!(writer, "{}", serde_json::to_string(&out_line)?)?;
    }

    // ─── Part 2: Process Stream Decoder Cases ────────────────────────────────
    for dc in &dataset.decoder_cases {
        let dc_tools: Vec<ToolDef> = dataset
            .tools
            .iter()
            .filter(|t| dc.tools.contains(&t.function.name))
            .cloned()
            .collect();

        let mut decoder = StreamDecoder::new(dc_tools);
        for chunk in &dc.chunks {
            decoder.push_chunk(chunk);
        }

        let decoded_result = match decoder.finish() {
            Ok(calls) => {
                let call_values: Vec<Value> = calls
                    .into_iter()
                    .map(|c| {
                        let args: Value =
                            serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                        json!({
                            "name": c.function.name,
                            "arguments": args
                        })
                    })
                    .collect();
                json!({ "calls": call_values })
            }
            Err(e) => {
                json!({ "error": e.eval_error_kind() })
            }
        };

        let out_line = json!({
            "id": dc.id,
            "decoded": decoded_result
        });

        writeln!(writer, "{}", serde_json::to_string(&out_line)?)?;
    }

    writer.flush()?;

    println!("✅ Evaluation output written to {}", out_path);
    if total_baseline_tokens > 0 {
        let saved_pct = (1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)) * 100.0;
        println!("------------------------------------------------------------");
        println!("📊 TOKEN REDUCTION METRICS (o200k_base):");
        println!("   Baseline Tokens: {}", total_baseline_tokens);
        println!("   Compact Tokens:  {}", total_compact_tokens);
        println!("   Tokens Saved:    {:.2}%", saved_pct);
        println!("------------------------------------------------------------");
    }

    Ok(())
}
