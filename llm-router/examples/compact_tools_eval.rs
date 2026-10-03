//! Evaluation harness for compact tool schemas.
//!
//! Contract:
//! - Reads dataset path from `EVAL_SET` env var (defaults to `compact-tools-eval.json`).
//! - Writes one JSONL output per case to `OUT` env var (defaults to `/tmp/out.jsonl` or stdout).
//! - Deterministic offline mode: no network, zero API keys required.
//! - Optional live mode: when `PROVIDER_BASE_URL` and `MODEL` are set, executes requests against
//!   an OpenAI-compatible endpoint at temperature 0 and appends `raw_output` and `live_calls`.

use nasiko_tool_compact::{self as compact, StreamDecoder, ToolDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: String,
    #[serde(default)]
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<TestCase>,
    #[serde(default)]
    decoder_cases: Vec<DecoderTestCase>,
}

#[derive(Debug, Deserialize)]
struct TestCase {
    id: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
}

#[derive(Debug, Deserialize)]
struct DecoderTestCase {
    id: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct CaseOutput {
    id: String,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_calls: Option<Value>,
}

#[derive(Debug, Serialize)]
struct DecoderCaseOutput {
    id: String,
    decoded: Value,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = std::env::var("EVAL_SET").unwrap_or_else(|_| {
        if Path::new("compact-tools-eval.json").exists() {
            "compact-tools-eval.json".to_string()
        } else {
            "/tmp/compact-tools-eval.json".to_string()
        }
    });

    let out_path = std::env::var("OUT").unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let provider_base_url = std::env::var("PROVIDER_BASE_URL").ok();
    let model = std::env::var("MODEL").ok();
    let live_mode = provider_base_url.is_some() && model.is_some();

    eprintln!("[compact-tools-eval] Reading eval set from: {eval_set_path}");
    eprintln!("[compact-tools-eval] Writing output to: {out_path}");
    if live_mode {
        eprintln!(
            "[compact-tools-eval] Live mode active: model={:?} base_url={:?}",
            model, provider_base_url
        );
    } else {
        eprintln!("[compact-tools-eval] Offline mode: deterministic, no network calls");
    }

    let file = File::open(&eval_set_path).map_err(|e| {
        format!("Failed to open EVAL_SET file '{eval_set_path}': {e}. Ensure file exists.")
    })?;
    let dataset: EvalDataset = serde_json::from_reader(BufReader::new(file))?;

    let mut out_file = File::create(&out_path)?;

    let mut total_baseline_bytes: usize = 0;
    let mut total_compact_bytes: usize = 0;

    // 1. Process `cases`
    for case in &dataset.cases {
        // Resolve tools referenced by this case
        let case_tools: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|t_name| dataset.tools.iter().find(|t| t.name() == t_name).cloned())
            .collect();

        // Calculate baseline request representation size
        let baseline_req = json!({
            "model": "gpt-4o",
            "messages": case.messages,
            "tools": case_tools,
        });
        let baseline_bytes = serde_json::to_string(&baseline_req)?.len();
        total_baseline_bytes += baseline_bytes;

        // Attempt compaction
        let (compact_request, compacted) = match compact::encode_tools(&case_tools) {
            Ok(encoded) => {
                let mut msgs = case.messages.clone();
                let injection = format!(
                    "{}\n{}\nToday is 2026-10-02, timezone Asia/Kolkata.",
                    encoded.definitions, encoded.instructions
                );

                let mut injected = false;
                for m in &mut msgs {
                    if m.get("role").and_then(Value::as_str) == Some("system") {
                        if let Some(existing) = m.get_mut("content") {
                            if let Some(s) = existing.as_str() {
                                *existing = Value::String(format!("{s}\n\n{injection}"));
                                injected = true;
                                break;
                            }
                        }
                    }
                }
                if !injected {
                    msgs.insert(
                        0,
                        json!({
                            "role": "system",
                            "content": injection,
                        }),
                    );
                }

                let req = json!({
                    "model": "gpt-4o",
                    "messages": msgs,
                });
                (req, true)
            }
            Err(_) => {
                // Compaction bypassed
                (baseline_req, false)
            }
        };

        let compact_bytes = serde_json::to_string(&compact_request)?.len();
        total_compact_bytes += compact_bytes;

        // Render expected calls in <<call ...>> syntax
        let mut rendered_lines = Vec::new();
        for exp in &case.expected {
            let args_json = serde_json::to_string(&exp.arguments)?;
            rendered_lines.push(format!("<<call {} {}>>", exp.name, args_json));
        }
        let rendered_calls = rendered_lines.join("\n");

        // Roundtrip decode back
        let roundtrip_calls = if rendered_calls.is_empty() {
            Vec::new()
        } else {
            let decoded = compact::decode_calls(&rendered_calls, &case_tools).unwrap_or_default();
            decoded
                .into_iter()
                .map(|c| {
                    json!({
                        "name": c.function.name,
                        "arguments": serde_json::from_str::<Value>(&c.function.arguments).unwrap_or(Value::Null)
                    })
                })
                .collect()
        };

        // Live mode execution if configured
        let mut raw_output = None;
        let mut live_calls = None;

        if live_mode {
            let client = reqwest::Client::new();
            let base_url = provider_base_url.as_ref().unwrap().trim_end_matches('/');
            let endpoint = format!("{base_url}/chat/completions");

            let mut live_body = compact_request.clone();
            if let Some(obj) = live_body.as_object_mut() {
                obj.insert("model".to_string(), Value::String(model.clone().unwrap()));
                obj.insert("temperature".to_string(), json!(0));
            }

            match client.post(&endpoint).json(&live_body).send().await {
                Ok(resp) => {
                    if let Ok(resp_json) = resp.json::<Value>().await {
                        if let Some(content) = resp_json
                            .pointer("/choices/0/message/content")
                            .and_then(Value::as_str)
                        {
                            raw_output = Some(content.to_string());
                            match compact::decode_calls(content, &case_tools) {
                                Ok(calls) => {
                                    let formatted: Vec<_> = calls
                                        .into_iter()
                                        .map(|c| {
                                            json!({
                                                "name": c.function.name,
                                                "arguments": serde_json::from_str::<Value>(&c.function.arguments).unwrap_or(Value::Null)
                                            })
                                        })
                                        .collect();
                                    live_calls = Some(json!({ "calls": formatted }));
                                }
                                Err(e) => {
                                    live_calls = Some(json!({ "error": e.error_code() }));
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("[compact-tools-eval] Live request error for {}: {err}", case.id);
                }
            }
        }

        let output_line = CaseOutput {
            id: case.id.clone(),
            compact_request,
            compacted,
            rendered_calls,
            roundtrip_calls,
            raw_output,
            live_calls,
        };

        let line_json = serde_json::to_string(&output_line)?;
        writeln!(out_file, "{line_json}")?;
    }

    // 2. Process `decoder_cases`
    for d_case in &dataset.decoder_cases {
        let case_tools: Vec<ToolDef> = d_case
            .tools
            .iter()
            .filter_map(|t_name| dataset.tools.iter().find(|t| t.name() == t_name).cloned())
            .collect();

        let mut decoder = StreamDecoder::new(case_tools);
        let mut feed_err = None;

        for chunk in &d_case.chunks {
            if let Err(e) = decoder.feed(chunk) {
                feed_err = Some(e);
                break;
            }
        }

        let decoded = match feed_err {
            Some(e) => json!({ "error": e.error_code() }),
            None => match decoder.finish() {
                Ok(calls) => {
                    let formatted: Vec<_> = calls
                        .into_iter()
                        .map(|c| {
                            json!({
                                "name": c.function.name,
                                "arguments": serde_json::from_str::<Value>(&c.function.arguments).unwrap_or(Value::Null)
                            })
                        })
                        .collect();
                    json!({ "calls": formatted })
                }
                Err(e) => json!({ "error": e.error_code() }),
            },
        };

        let output_line = DecoderCaseOutput {
            id: d_case.id.clone(),
            decoded,
        };

        let line_json = serde_json::to_string(&output_line)?;
        writeln!(out_file, "{line_json}")?;
    }

    out_file.flush()?;

    let savings_pct = if total_baseline_bytes > 0 {
        (1.0 - (total_compact_bytes as f64 / total_baseline_bytes as f64)) * 100.0
    } else {
        0.0
    };

    eprintln!("[compact-tools-eval] Done! Processed {} cases + {} decoder cases.", dataset.cases.len(), dataset.decoder_cases.len());
    eprintln!("[compact-tools-eval] Baseline bytes: {total_baseline_bytes}, Compact bytes: {total_compact_bytes}");
    eprintln!("[compact-tools-eval] Estimated byte savings: {savings_pct:.1}%");

    Ok(())
}
