//! Offline and live evaluator for compact tool schemas (P1).
//!
//! Run via:
//! ```sh
//! EVAL_SET=/path/to/compact-tools-eval.json OUT=/path/to/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use nasiko_tool_compact::{
    CompactTools, StreamDecoder, ToolDef, decode_calls, encode_tools, render_call,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EvalFile {
    #[serde(default)]
    schema_version: String,
    #[serde(default)]
    purpose: String,
    tools: Vec<ToolDef>,
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    expected: Vec<ExpectedCall>,
    #[serde(default)]
    #[allow(dead_code)]
    r#match: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
    tools: Vec<String>,
    chunks: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    expected: Value,
}

#[tokio::main]
async fn main() {
    let eval_set_path = std::env::var("EVAL_SET").unwrap_or_else(|_| {
        let default_path = "/tmp/compact-tools-eval.json";
        if Path::new(default_path).exists() {
            default_path.to_string()
        } else {
            // Local fallback for dev/testing
            "compact-tools-eval.json".to_string()
        }
    });

    let out_path = std::env::var("OUT").ok();
    let model = std::env::var("MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
    let report_enabled = std::env::var("REPORT").map(|v| v == "1").unwrap_or(false);
    let variants_enabled = std::env::var("VARIANTS").map(|v| v == "1").unwrap_or(false);
    let provider_base = std::env::var("PROVIDER_BASE_URL").ok();
    let provider_key = std::env::var("PROVIDER_API_KEY").ok();

    let file = match File::open(&eval_set_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to open EVAL_SET at '{eval_set_path}': {e}");
            std::process::exit(1);
        }
    };

    let reader = BufReader::new(file);
    let eval_data: EvalFile = match serde_json::from_reader(reader) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("Failed to parse EVAL_SET JSON: {e}");
            std::process::exit(1);
        }
    };

    // Index tools by name (using BTreeMap for deterministic key traversal)
    let tool_index: BTreeMap<String, ToolDef> = eval_data
        .tools
        .into_iter()
        .map(|t| (t.function.name.clone(), t))
        .collect();

    let mut out_writer: Box<dyn Write> = match out_path {
        Some(ref path) => {
            let f = File::create(path).unwrap_or_else(|e| {
                eprintln!("Failed to create OUT file '{path}': {e}");
                std::process::exit(1);
            });
            Box::new(BufWriter::new(f))
        }
        None => Box::new(std::io::stdout().lock()),
    };

    let mut report_rows = Vec::new();

    // 1. Process standard evaluation cases (ct-*)
    for case in eval_data.cases {
        let defs: Vec<ToolDef> = case
            .tools
            .iter()
            .filter_map(|name| {
                let found = tool_index.get(name).cloned();
                if found.is_none() {
                    eprintln!("Warning: tool '{name}' not found for case '{}'", case.id);
                }
                found
            })
            .collect();

        match encode_tools(&defs) {
            Ok(compact) => {
                let compact_req = build_compact_request(&compact, &case.messages, &model);
                let rendered_calls = case
                    .expected
                    .iter()
                    .map(|e| render_call(&e.name, &e.arguments))
                    .collect::<Vec<_>>()
                    .join("\n");

                let roundtrip_calls = match decode_calls(&rendered_calls, &defs) {
                    Ok(calls) => calls
                        .into_iter()
                        .map(|c| {
                            let args: Value =
                                serde_json::from_str(&c.function.arguments).unwrap_or(Value::Null);
                            json!({
                                "name": c.function.name,
                                "arguments": args
                            })
                        })
                        .collect::<Vec<_>>(),
                    Err(e) => {
                        eprintln!("Roundtrip decode error for case '{}': {e}", case.id);
                        vec![]
                    }
                };

                let mut line = json!({
                    "id": case.id,
                    "compact_request": compact_req,
                    "compacted": true,
                    "rendered_calls": rendered_calls,
                    "roundtrip_calls": roundtrip_calls,
                });

                if let Some(base_url) = &provider_base {
                    let client = reqwest::Client::new();
                    let url = if base_url.ends_with("/chat/completions") {
                        base_url.clone()
                    } else {
                        format!("{base_url}/chat/completions")
                    };
                    let mut req_builder = client.post(&url).json(&json!({
                        "model": model,
                        "messages": compact_req["messages"],
                        "temperature": 0
                    }));
                    if let Some(key) = &provider_key {
                        req_builder = req_builder.bearer_auth(key);
                    }
                    let res = req_builder
                        .timeout(std::time::Duration::from_secs(30))
                        .send()
                        .await;
                    match res {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(resp_json) = resp.json::<Value>().await {
                                let content = resp_json["choices"][0]["message"]["content"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let live_calls = match decode_calls(&content, &defs) {
                                    Ok(calls) => {
                                        let calls_val: Vec<Value> = calls
                                            .into_iter()
                                            .map(|c| {
                                                let args: Value =
                                                    serde_json::from_str(&c.function.arguments)
                                                        .unwrap_or(Value::Null);
                                                json!({
                                                    "name": c.function.name,
                                                    "arguments": args
                                                })
                                            })
                                            .collect();
                                        json!({ "calls": calls_val })
                                    }
                                    Err(e) => json!({ "error": e.code() }),
                                };
                                line["raw_output"] = Value::String(content);
                                line["live_calls"] = live_calls;
                            } else {
                                line["live_calls"] = json!({ "error": "invalid_response_json" });
                            }
                        }
                        Ok(resp) => {
                            line["live_calls"] =
                                json!({ "error": format!("http_{}", resp.status()) });
                        }
                        Err(e) => {
                            line["live_calls"] = json!({ "error": format!("network_error: {e}") });
                        }
                    }
                }

                if let Err(e) = writeln!(out_writer, "{line}") {
                    eprintln!("Failed to write output line: {e}");
                }

                if report_enabled {
                    let baseline_req = build_native_request(&defs, &case.messages, &model);
                    report_rows.push((case.id, true, baseline_req, compact_req));
                }
            }
            Err(e) => {
                eprintln!(
                    "Bypassing compaction for case '{}' due to unsupported schema: {e}",
                    case.id
                );
                let bypass_req = build_native_request(&defs, &case.messages, &model);
                let line = json!({
                    "id": case.id,
                    "compact_request": bypass_req,
                    "compacted": false,
                    "rendered_calls": "",
                    "roundtrip_calls": [],
                });

                if let Err(err) = writeln!(out_writer, "{line}") {
                    eprintln!("Failed to write output line: {err}");
                }

                if report_enabled {
                    report_rows.push((case.id, false, bypass_req.clone(), bypass_req));
                }
            }
        }
    }

    // 2. Process decoder test cases (dc-*)
    for dc in eval_data.decoder_cases {
        let defs: Vec<ToolDef> = dc
            .tools
            .iter()
            .filter_map(|name| {
                let found = tool_index.get(name).cloned();
                if found.is_none() {
                    eprintln!("Warning: tool '{name}' not found for dc '{}'", dc.id);
                }
                found
            })
            .collect();

        let decoded = match StreamDecoder::new(&defs) {
            Ok(mut decoder) => {
                for chunk in &dc.chunks {
                    if decoder.push(chunk).is_err() {
                        break;
                    }
                }
                match decoder.finish() {
                    Ok(calls) => {
                        let calls_val: Vec<Value> = calls
                            .into_iter()
                            .map(|c| {
                                let args: Value = serde_json::from_str(&c.function.arguments)
                                    .unwrap_or(Value::Null);
                                json!({
                                    "name": c.function.name,
                                    "arguments": args
                                })
                            })
                            .collect();
                        json!({ "calls": calls_val })
                    }
                    Err(e) => json!({ "error": e.code() }),
                }
            }
            Err(e) => json!({ "error": e.code() }),
        };

        let line = json!({
            "id": dc.id,
            "decoded": decoded,
        });

        if let Err(e) = writeln!(out_writer, "{line}") {
            eprintln!("Failed to write output line: {e}");
        }
    }

    if let Err(e) = out_writer.flush() {
        eprintln!("Failed to flush output writer: {e}");
    }

    // 3. Optional token reporting (REPORT=1)
    if report_enabled {
        print_token_report(&report_rows);
    }

    // 4. Optional D2-D6 variant comparison (VARIANTS=1)
    if variants_enabled {
        print_variant_comparison();
    }
}

fn build_compact_request(compact: &CompactTools, messages: &[Value], model: &str) -> Value {
    let mut msgs = Vec::new();
    let ref_time = "Reference time: 2026-10-02 Asia/Kolkata";
    let system_content = format!("{}\n{}", compact.render(), ref_time);

    let mut has_system = false;
    for m in messages {
        let mut msg_obj = m.clone();
        if msg_obj.get("role").and_then(Value::as_str) == Some("system") && !has_system {
            let existing = msg_obj.get("content").and_then(Value::as_str).unwrap_or("");
            let merged = if existing.is_empty() {
                system_content.clone()
            } else {
                format!("{existing}\n{system_content}")
            };
            msg_obj["content"] = Value::String(merged);
            has_system = true;
        }
        msgs.push(msg_obj);
    }

    if !has_system {
        msgs.insert(
            0,
            json!({
                "role": "system",
                "content": system_content,
            }),
        );
    }

    json!({
        "model": model,
        "messages": msgs,
    })
}

fn build_native_request(defs: &[ToolDef], messages: &[Value], model: &str) -> Value {
    let mut msgs = Vec::new();
    let ref_time = "Reference time: 2026-10-02 Asia/Kolkata";

    let mut has_system = false;
    for m in messages {
        let mut msg_obj = m.clone();
        if msg_obj.get("role").and_then(Value::as_str) == Some("system") && !has_system {
            let existing = msg_obj.get("content").and_then(Value::as_str).unwrap_or("");
            let merged = if existing.is_empty() {
                ref_time.to_string()
            } else {
                format!("{existing}\n{ref_time}")
            };
            msg_obj["content"] = Value::String(merged);
            has_system = true;
        }
        msgs.push(msg_obj);
    }

    if !has_system {
        msgs.insert(
            0,
            json!({
                "role": "system",
                "content": ref_time,
            }),
        );
    }

    let tools_val: Vec<Value> = defs
        .iter()
        .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
        .collect();

    json!({
        "model": model,
        "messages": msgs,
        "tools": tools_val,
    })
}

fn print_token_report(rows: &[(String, bool, Value, Value)]) {
    let bpe = tiktoken_rs::o200k_base().ok();

    eprintln!("\n=== COMPACT TOOLS TOKEN REPORT ===");
    let mut total_base = 0;
    let mut total_compact = 0;
    let mut bypassed = 0;

    for (id, compacted, base_req, comp_req) in rows {
        if !*compacted {
            bypassed += 1;
        }
        let (base_toks, comp_toks) = match &bpe {
            Some(tokenizer) => {
                let base_str = base_req.to_string();
                let comp_str = comp_req.to_string();
                (
                    tokenizer.encode_with_special_tokens(&base_str).len(),
                    tokenizer.encode_with_special_tokens(&comp_str).len(),
                )
            }
            None => {
                // Character-level approximation if tokenizer unavailable
                (
                    base_req.to_string().len() / 4,
                    comp_req.to_string().len() / 4,
                )
            }
        };

        total_base += base_toks;
        total_compact += comp_toks;

        let pct = if base_toks > 0 {
            100.0 * (1.0 - (comp_toks as f64 / base_toks as f64))
        } else {
            0.0
        };

        eprintln!("{id}: baseline={base_toks} compact={comp_toks} reduction={pct:.1}%");
    }

    let overall_pct = if total_base > 0 {
        100.0 * (1.0 - (total_compact as f64 / total_base as f64))
    } else {
        0.0
    };

    eprintln!(
        "Total: baseline={total_base} compact={total_compact} reduction={overall_pct:.1}% (bypassed: {bypassed}/{})",
        rows.len()
    );
    eprintln!("===================================\n");
}

fn print_variant_comparison() {
    let Ok(tokenizer) = tiktoken_rs::o200k_base() else {
        eprintln!("Tokenizer o200k_base not available for variant measurement");
        return;
    };

    eprintln!("\n=== D2-D6 VARIANT MEASUREMENT (o200k_base) ===");

    // D2: Type spellings in compact field context
    eprintln!("-- D2: Type spellings in compact field context --");
    for (short, long) in [
        ("title:str", "title:string"),
        ("count:int", "count:integer"),
        ("price:num", "price:number"),
        ("active:bool", "active:boolean"),
        ("created_at:datetime", "created_at:date-time"),
    ] {
        let tok_short = tokenizer.encode_with_special_tokens(short).len();
        let tok_long = tokenizer.encode_with_special_tokens(long).len();
        eprintln!("  {short:22} ({tok_short} toks) vs {long:22} ({tok_long} toks)");
    }

    // D3: Optional markers
    eprintln!("\n-- D3: Optional marker syntax --");
    for (label, snippet) in [
        ("bare '?' (baseline)", "duration_min?:int"),
        ("opt: prefix", "opt:duration_min:int"),
        ("=null suffix", "duration_min:int=null"),
    ] {
        let toks = tokenizer.encode_with_special_tokens(snippet).len();
        eprintln!("  {label:25} ({snippet}): {toks} tokens");
    }

    // D5: Description syntax
    eprintln!("\n-- D5: Field description syntax --");
    for (label, snippet) in [
        (
            "space + quote (baseline)",
            "start:datetime \"Start time, ISO 8601\"",
        ),
        ("dash syntax", "start:datetime - Start time, ISO 8601"),
        ("parentheses syntax", "start:datetime(Start time, ISO 8601)"),
    ] {
        let toks = tokenizer.encode_with_special_tokens(snippet).len();
        eprintln!("  {label:25} -> {toks} tokens");
    }

    // D6: Instruction wording
    eprintln!("\n-- D6: Instruction text variants --");
    let var_a = "To call a tool, emit: <<call name {json args}>>";
    let var_b = "Call: <<call name {args}>>";
    let var_c = "Tool call format: <<call name {valid json object}>>. Only emit calls.";

    let toks_a = tokenizer.encode_with_special_tokens(var_a).len();
    let toks_b = tokenizer.encode_with_special_tokens(var_b).len();
    let toks_c = tokenizer.encode_with_special_tokens(var_c).len();
    eprintln!("  Variant A (baseline): {toks_a} tokens: \"{var_a}\"");
    eprintln!("  Variant B (minimal):  {toks_b} tokens: \"{var_b}\"");
    eprintln!("  Variant C (explicit): {toks_c} tokens: \"{var_c}\"");
    eprintln!("==============================================\n");
}
