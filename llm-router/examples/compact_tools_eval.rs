//! P1 Compact Tool Protocol evaluation runner.
//!
//! Conforms to Section 1.15 of the specification:
//! Runs offline deterministic evaluation or live model evaluation against an `EVAL_SET` JSON file
//! and outputs JSONL records to `OUT`.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;

use nasiko_tool_compact::types::ToolDef as CompactToolDef;
use nasiko_tool_compact::{decode_calls, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const BENCHMARK_REFERENCE_TIME: &str = "Today is 2026-10-02. Timezone: Asia/Kolkata.";

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub reference_time: Option<String>,
    #[serde(default)]
    pub tools: HashMap<String, Value>,
    #[serde(default)]
    pub cases: Vec<TestCase>,
    #[serde(default)]
    pub decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Deserialize)]
struct TestCase {
    pub id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub inline_tools: Option<Vec<Value>>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub expected_calls: Option<Vec<ExpectedCall>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ExpectedCall {
    pub name: String,
    pub arguments: Value,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct DecoderCase {
    pub id: String,
    pub input: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub expected_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct OrdinaryOutputRecord {
    pub id: String,
    pub compact_request: Value,
    pub compacted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bypass_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_calls: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roundtrip_calls: Option<Vec<ExpectedCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_calls: Option<Value>,
}

#[derive(Debug, Serialize)]
struct DecoderOutputRecord {
    pub id: String,
    pub decoded: Value,
}

fn parse_tool_val(v: &Value) -> Option<CompactToolDef> {
    if let Some(func) = v.get("function") {
        let name = func.get("name")?.as_str()?.to_string();
        let description = func.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
        let parameters = func.get("parameters").cloned().unwrap_or_else(|| json!({}));
        Some(CompactToolDef::new(name, description, parameters))
    } else if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
        let description = v.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
        let parameters = v.get("parameters").cloned().unwrap_or_else(|| json!({}));
        Some(CompactToolDef::new(name, description, parameters))
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "llm-router/fixtures/compact-tools-eval.json".to_string());
    let out_path = env::var("OUT").unwrap_or_else(|_| "target/compact-tools-eval-out.jsonl".to_string());

    let provider_base_url = env::var("PROVIDER_BASE_URL").ok();
    let model = env::var("MODEL").ok();
    let is_live = provider_base_url.is_some() && model.is_some();

    eprintln!("[compact_tools_eval] loading dataset from {}", eval_set_path);
    eprintln!("[compact_tools_eval] target output: {}", out_path);
    if is_live {
        eprintln!(
            "[compact_tools_eval] live mode enabled (provider: {}, model: {})",
            provider_base_url.as_ref().unwrap(),
            model.as_ref().unwrap()
        );
    } else {
        eprintln!("[compact_tools_eval] offline deterministic evaluation mode");
    }

    let file = File::open(&eval_set_path).map_err(|e| {
        eprintln!("[compact_tools_eval] fatal error opening {}: {}", eval_set_path, e);
        e
    })?;
    let dataset: EvalDataset = serde_json::from_reader(BufReader::new(file)).map_err(|e| {
        eprintln!("[compact_tools_eval] fatal error parsing dataset JSON: {}", e);
        e
    })?;

    if let Some(parent) = Path::new(&out_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut out_file = File::create(&out_path)?;

    let mut total_cases = 0;
    let mut total_compacted = 0;
    let mut total_bypassed = 0;
    let mut total_decoder_cases = 0;

    // Process standard test cases
    for case in dataset.cases {
        total_cases += 1;
        // Resolve tools
        let mut tool_defs: Vec<CompactToolDef> = Vec::new();
        if let Some(inline) = case.inline_tools {
            for t in &inline {
                if let Some(td) = parse_tool_val(t) {
                    tool_defs.push(td);
                }
            }
        }
        for name in &case.tools {
            if let Some(tool_val) = dataset.tools.get(name) {
                if let Some(td) = parse_tool_val(tool_val) {
                    tool_defs.push(td);
                }
            }
        }

        // Check if supported
        let encode_res = encode_tools(&tool_defs);
        let (compacted, bypass_reason, compact_doc) = match encode_res {
            Ok(ct) => (true, None, Some(ct.text)),
            Err(err) => (false, Some(err.to_string()), None),
        };

        if compacted {
            total_compacted += 1;
        } else {
            total_bypassed += 1;
        }

        let mut req_messages = case.messages.clone();
        // Add fixed reference-time context and compact instructions if compacted
        if compacted {
            let ref_time_msg = json!({
                "role": "system",
                "content": BENCHMARK_REFERENCE_TIME
            });
            let compact_msg = json!({
                "role": "system",
                "content": compact_doc.clone().unwrap()
            });
            req_messages.insert(0, ref_time_msg);
            req_messages.push(compact_msg);
        }

        let compact_request = json!({
            "messages": req_messages,
        });

        // Offline rendered calls & roundtrip calls
        let mut rendered_calls = None;
        let mut roundtrip_calls = None;

        if let Some(expected) = &case.expected_calls {
            let mut calls_str = String::new();
            for call in expected {
                calls_str.push_str(&format!(
                    "<<call {} {}>>",
                    call.name,
                    serde_json::to_string(&call.arguments).unwrap_or_default()
                ));
            }
            rendered_calls = Some(calls_str.clone());

            if compacted {
                if let Ok(decoded) = decode_calls(&calls_str, &tool_defs) {
                    let r_calls: Vec<ExpectedCall> = decoded
                        .into_iter()
                        .map(|c| ExpectedCall {
                            name: c.name,
                            arguments: c.arguments,
                        })
                        .collect();
                    roundtrip_calls = Some(r_calls);
                }
            }
        }

        let mut record = OrdinaryOutputRecord {
            id: case.id.clone(),
            compact_request,
            compacted,
            bypass_reason,
            rendered_calls,
            roundtrip_calls,
            raw_output: None,
            live_calls: None,
        };

        // Live execution if requested
        if is_live && compacted {
            let base_url = provider_base_url.as_ref().unwrap();
            let mdl = model.as_ref().unwrap();
            let auth_header = env::var("PROVIDER_API_KEY").unwrap_or_default();

            let client = reqwest::Client::new();
            let mut req_builder = client
                .post(format!("{}/chat/completions", base_url.trim_end_matches('/')))
                .json(&json!({
                    "model": mdl,
                    "messages": req_messages,
                    "temperature": 0.0
                }));
            if !auth_header.is_empty() {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", auth_header));
            }

            match req_builder.send().await {
                Ok(resp) => {
                    if let Ok(body) = resp.json::<Value>().await {
                        let text = body["choices"][0]["message"]["content"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        record.raw_output = Some(text.clone());

                        match decode_calls(&text, &tool_defs) {
                            Ok(dec) => {
                                let c_list: Vec<Value> = dec
                                    .into_iter()
                                    .map(|c| json!({ "name": c.name, "arguments": c.arguments }))
                                    .collect();
                                record.live_calls = Some(json!({ "calls": c_list }));
                            }
                            Err(e) => {
                                record.live_calls = Some(json!({ "error": e.to_string() }));
                            }
                        }
                    } else {
                        record.live_calls = Some(json!({ "error": "upstream_parse_failed" }));
                    }
                }
                Err(err) => {
                    record.live_calls = Some(json!({ "error": format!("http_error: {err}") }));
                }
            }
        }

        let line = serde_json::to_string(&record)?;
        writeln!(out_file, "{}", line)?;
    }

    // Process decoder cases
    for dcase in dataset.decoder_cases {
        total_decoder_cases += 1;
        let mut tool_defs: Vec<CompactToolDef> = Vec::new();
        for name in &dcase.tools {
            if let Some(tool_val) = dataset.tools.get(name) {
                if let Some(td) = parse_tool_val(tool_val) {
                    tool_defs.push(td);
                }
            }
        }

        let decoded_val = match decode_calls(&dcase.input, &tool_defs) {
            Ok(res) => {
                let calls: Vec<Value> = res
                    .into_iter()
                    .map(|c| json!({ "name": c.name, "arguments": c.arguments }))
                    .collect();
                json!({ "calls": calls })
            }
            Err(e) => {
                json!({ "error": e.eval_label() })
            }
        };

        let record = DecoderOutputRecord {
            id: dcase.id,
            decoded: decoded_val,
        };
        let line = serde_json::to_string(&record)?;
        writeln!(out_file, "{}", line)?;
    }

    eprintln!(
        "[compact_tools_eval] completed successfully: {} test cases ({} compacted, {} bypassed), {} decoder cases",
        total_cases, total_compacted, total_bypassed, total_decoder_cases
    );
    Ok(())
}
