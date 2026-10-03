//! compact_tools_eval: Evaluates compact-tools schema compaction, token reduction, and stream decoding.
//!
//! Invocation contract:
//! `EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl cargo run --release -p nasiko-llm-router --example compact_tools_eval`

use std::fs::{File, read_to_string};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use nasiko_tool_compact::{
    CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls, encode_tools, render_calls,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[allow(dead_code)]
    schema_version: Option<String>,
    #[allow(dead_code)]
    purpose: Option<String>,
    tools: Vec<ToolDef>,
    cases: Vec<EvalCase>,
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ExpectedCall>,
    #[allow(dead_code)]
    #[serde(rename = "match")]
    match_spec: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[allow(dead_code)]
    note: Option<String>,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: ExpectedDecoderResult,
}

#[derive(Debug, Deserialize)]
struct ExpectedDecoderResult {
    calls: Option<Vec<ExpectedCall>>,
    error: Option<String>,
}

fn resolve_eval_path() -> PathBuf {
    if let Ok(env_path) = std::env::var("EVAL_SET") {
        let p = PathBuf::from(&env_path);
        if p.exists() {
            return p;
        }
        #[cfg(windows)]
        {
            if env_path.starts_with("/tmp/") || env_path.starts_with("\\tmp\\") {
                let stripped = env_path.trim_start_matches('/').trim_start_matches('\\');
                let win_tmp = PathBuf::from("C:\\").join(stripped);
                if win_tmp.exists() {
                    return win_tmp;
                }
            }
        }
    }

    let candidates = [
        PathBuf::from("/tmp/compact-tools-eval.json"),
        PathBuf::from("C:\\tmp\\compact-tools-eval.json"),
        PathBuf::from("compact-tools-eval.json"),
        PathBuf::from("../nasiko-work/compact-tools-eval.json"),
    ];

    for c in candidates {
        if c.exists() {
            return c;
        }
    }

    PathBuf::from("compact-tools-eval.json")
}

fn resolve_out_path() -> PathBuf {
    if let Ok(env_out) = std::env::var("OUT") {
        let p = PathBuf::from(&env_out);
        #[cfg(windows)]
        {
            if env_out.starts_with("/tmp/") || env_out.starts_with("\\tmp\\") {
                let stripped = env_out.trim_start_matches('/').trim_start_matches('\\');
                return PathBuf::from("C:\\").join(stripped);
            }
        }
        return p;
    }

    #[cfg(windows)]
    return PathBuf::from("C:\\tmp\\out.jsonl");
    #[cfg(not(windows))]
    return PathBuf::from("/tmp/out.jsonl");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = resolve_eval_path();
    eprintln!(
        "[compact_tools_eval] Loading eval set from: {}",
        eval_path.display()
    );

    let content = match read_to_string(&eval_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[compact_tools_eval] Error reading {}: {e}",
                eval_path.display()
            );
            std::process::exit(1);
        }
    };

    let dataset: EvalDataset = serde_json::from_str(&content)?;
    let out_path = resolve_out_path();
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out_file = File::create(&out_path)?;
    let mut writer = BufWriter::new(out_file);

    // BPE tokenizer for o200k_base token measurement
    let bpe = tiktoken_rs::o200k_base().ok();

    let reference_time = "Reference time: today is 2026-10-02, timezone Asia/Kolkata.\n";

    let mut total_baseline_tokens = 0usize;
    let mut total_compact_tokens = 0usize;
    let mut cases_compacted = 0usize;
    let mut roundtrip_matches = 0usize;
    let mut decoder_matches = 0usize;

    // Optional live provider
    let live_provider_url = std::env::var("PROVIDER_BASE_URL").ok();
    let live_model = std::env::var("MODEL").ok();
    let live_api_key = std::env::var("PROVIDER_API_KEY").ok();
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // ── 1. Process Cases ──────────────────────────────────────────────────
    for case in &dataset.cases {
        let selected_tool_defs: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|t_name| dataset.tools.iter().find(|t| &t.name == t_name).cloned())
            .collect();

        // Check if compaction succeeds or bypasses
        let encode_res = encode_tools(&selected_tool_defs);
        let compacted = encode_res.is_ok();

        let mut request_messages = Vec::new();
        let mut system_text = reference_time.to_string();

        if let Ok(compact_tools) = &encode_res
            && !compact_tools.full_prompt.is_empty()
        {
            system_text.push_str(&compact_tools.full_prompt);
            system_text.push('\n');
        }

        request_messages.push(json!({
            "role": "system",
            "content": system_text
        }));

        for m in &case.messages {
            request_messages.push(m.clone());
        }

        let mut compact_request = json!({
            "messages": request_messages,
            "temperature": 0
        });

        // Baseline request for token comparison
        let mut baseline_messages = vec![json!({
            "role": "system",
            "content": reference_time
        })];
        for m in &case.messages {
            baseline_messages.push(m.clone());
        }
        let baseline_tools_val: Vec<Value> = selected_tool_defs
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect();

        let baseline_request = json!({
            "messages": baseline_messages,
            "tools": baseline_tools_val,
            "temperature": 0
        });

        if !compacted {
            // Bypass mode: tools stay native
            compact_request["tools"] = json!(baseline_tools_val);
        } else {
            cases_compacted += 1;
        }

        // Measure tokens
        let baseline_body_str = serde_json::to_string(&baseline_request).unwrap_or_default();
        let compact_body_str = serde_json::to_string(&compact_request).unwrap_or_default();

        let baseline_tokens = bpe
            .as_ref()
            .map(|tok| tok.encode_with_special_tokens(&baseline_body_str).len())
            .unwrap_or(baseline_body_str.len() / 4);
        let compact_tokens = if compacted {
            bpe.as_ref()
                .map(|tok| tok.encode_with_special_tokens(&compact_body_str).len())
                .unwrap_or(compact_body_str.len() / 4)
        } else {
            baseline_tokens // Bypassed cases count 0% savings
        };

        total_baseline_tokens += baseline_tokens;
        total_compact_tokens += compact_tokens;

        // Render expected calls
        let expected_tool_calls: Vec<ToolCall> = case
            .expected
            .iter()
            .map(|e| ToolCall::new(&e.name, e.arguments.clone()))
            .collect();
        let rendered_calls = render_calls(&expected_tool_calls);

        // Roundtrip decode
        let roundtrip_calls =
            decode_calls(&rendered_calls, &selected_tool_defs).unwrap_or_default();
        if roundtrip_calls == expected_tool_calls {
            roundtrip_matches += 1;
        }

        let mut out_line = json!({
            "id": case.id,
            "compact_request": compact_request,
            "compacted": compacted,
            "rendered_calls": rendered_calls,
            "roundtrip_calls": roundtrip_calls,
        });

        // Live mode if configured
        if let (Some(base_url), Some(model)) = (&live_provider_url, &live_model) {
            match execute_live_call(
                &http_client,
                base_url,
                model,
                live_api_key.as_deref(),
                &compact_request,
                &selected_tool_defs,
            )
            .await
            {
                Ok((raw_output, live_calls)) => {
                    out_line["raw_output"] = json!(raw_output);
                    out_line["live_calls"] = live_calls;
                }
                Err(err) => {
                    out_line["raw_output"] = json!(null);
                    out_line["live_calls"] = json!({ "error": err });
                }
            }
        }

        writeln!(writer, "{}", serde_json::to_string(&out_line)?)?;
    }

    // ── 2. Process Decoder Cases ──────────────────────────────────────────
    for d_case in &dataset.decoder_cases {
        let selected_tools: Vec<ToolDef> = d_case
            .tools
            .iter()
            .filter_map(|t_name| dataset.tools.iter().find(|t| &t.name == t_name).cloned())
            .collect();

        let mut decoder = StreamDecoder::new(selected_tools);
        for chunk in &d_case.chunks {
            let _ = decoder.push(chunk);
        }

        let decode_result = decoder.finish();
        let decoded_val = match decode_result {
            Ok(calls) => json!({ "calls": calls }),
            Err(CompactError::UnknownTool(_)) => json!({ "error": "unknown_tool" }),
            Err(CompactError::InvalidArguments { .. }) => json!({ "error": "invalid_arguments" }),
            Err(CompactError::Malformed(_)) => json!({ "error": "malformed" }),
            Err(CompactError::Unsupported { .. }) => json!({ "error": "unsupported" }),
        };

        // Check if decoder result matches expected
        let matches_expected = match &d_case.expected.error {
            Some(expected_err) => {
                decoded_val.get("error").and_then(Value::as_str) == Some(expected_err.as_str())
            }
            None => {
                if let Some(expected_calls) = &d_case.expected.calls {
                    if let Some(actual_calls) = decoded_val.get("calls").and_then(Value::as_array) {
                        let expected_json = json!(expected_calls);
                        let expected_arr = expected_json.as_array().unwrap();
                        actual_calls == expected_arr
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        };

        if matches_expected {
            decoder_matches += 1;
        }

        let out_line = json!({
            "id": d_case.id,
            "decoded": decoded_val,
        });

        writeln!(writer, "{}", serde_json::to_string(&out_line)?)?;
    }

    writer.flush()?;

    // ── 3. Summary to stderr ──────────────────────────────────────────────
    let reduction_pct = if total_baseline_tokens > 0 {
        (1.0 - (total_compact_tokens as f64 / total_baseline_tokens as f64)) * 100.0
    } else {
        0.0
    };

    eprintln!("\n=== COMPACT-TOOLS EVALUATION SUMMARY ===");
    eprintln!("Cases evaluated:        {}", dataset.cases.len());
    eprintln!(
        "Cases compacted:        {} (bypassed: {})",
        cases_compacted,
        dataset.cases.len() - cases_compacted
    );
    eprintln!(
        "Roundtrip matches:      {}/{} (100% target)",
        roundtrip_matches,
        dataset.cases.len()
    );
    eprintln!(
        "Decoder cases:          {}/{} matched expected",
        decoder_matches,
        dataset.decoder_cases.len()
    );
    eprintln!("Baseline request tokens: {}", total_baseline_tokens);
    eprintln!("Compact request tokens:  {}", total_compact_tokens);
    eprintln!("Token reduction:        {:.2}%", reduction_pct);
    eprintln!("OUT written to:         {}", out_path.display());
    eprintln!("========================================\n");

    Ok(())
}

async fn execute_live_call(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    compact_request: &Value,
    tools: &[ToolDef],
) -> Result<(String, Value), String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut req_body = compact_request.clone();
    req_body["model"] = json!(model);

    let mut request_builder = client.post(&url).json(&req_body);
    if let Some(key) = api_key {
        request_builder = request_builder.bearer_auth(key);
    }

    let resp = request_builder
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Provider error {status}: {body}"));
    }

    let resp_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid JSON response: {e}"))?;
    let raw_text = resp_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    match decode_calls(&raw_text, tools) {
        Ok(calls) => Ok((raw_text, json!({ "calls": calls }))),
        Err(err) => Ok((raw_text, json!({ "error": format!("{err}") }))),
    }
}
