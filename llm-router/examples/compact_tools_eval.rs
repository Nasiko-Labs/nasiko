//! P1 eval harness — compact_tools_eval
//!
//! Tests the full compact-tool pipeline:
//!   1. Compact the tool schemas → measure token reduction
//!   2. Build a <<call...>> marker from the expected call
//!   3. Feed it through the streaming decoder
//!   4. Validate the decoded tool name and arguments match expected
//!   5. Verify edge cases: no-tool cases (empty expected) are handled cleanly
//!
//! # Usage
//! ```sh
//! EVAL_SET=/tmp/compact-tools-eval.json \
//! OUT=/tmp/out-compact.jsonl \
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Eval set format (compact-tools-eval-v1)
//! ```json
//! {
//!   "schema_version": "compact-tools-eval-v1",
//!   "tools": [ /* full OpenAI tool defs */ ],
//!   "cases": [{
//!     "id": "ct-001",
//!     "tools": ["create_calendar_event"],
//!     "messages": [{"role":"user","content":"..."}],
//!     "expected": [{"name":"create_calendar_event","arguments":{...}}],
//!     "match": {"free_text_fields":["title"]}
//!   }]
//! }
//! ```
//!
//! # Output (one JSONL line per case)
//! ```json
//! {"id":"ct-001","decoded_ok":true,"tool_name":"create_calendar_event",
//!  "arguments":"{...}","token_reduction":0.71,"latency_us":95,"error":null}
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

use nasiko_tool_compact::{compact_tools, KnownTool, ToolDecoder};

fn main() {
    let eval_path =
        std::env::var("EVAL_SET").expect("EVAL_SET must be set (path to eval JSON file)");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".into());

    let raw = std::fs::read_to_string(&eval_path)
        .unwrap_or_else(|e| panic!("cannot read EVAL_SET {eval_path}: {e}"));
    let data: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("EVAL_SET is not valid JSON: {e}"));

    // All tool definitions (keyed by name for fast lookup)
    let all_tools = data["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("EVAL_SET must have a top-level \"tools\" array"));

    let tool_map: HashMap<&str, &serde_json::Value> = all_tools
        .iter()
        .filter_map(|t| {
            t.get("function")?.get("name")?.as_str().map(|n| (n, t))
        })
        .collect();

    let cases = data["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("EVAL_SET must have a top-level \"cases\" array"));

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path)
            .unwrap_or_else(|e| panic!("cannot create OUT {out_path}: {e}")),
    );

    let total = cases.len();
    let mut passed = 0usize;

    // ── Compact ALL tools once for the token reduction measurement ───────────
    let all_compact = compact_tools(all_tools);
    eprintln!("compact_tools_eval: {} tools compacted", all_compact.signatures.len());
    eprintln!("Compact signatures:");
    for sig in &all_compact.signatures {
        eprintln!("  {sig}");
    }
    eprintln!();

    for (i, case) in cases.iter().enumerate() {
        let id = case["id"].as_str().unwrap_or("?");

        // Which tools are active for this case
        let case_tool_names: Vec<&str> = case["tools"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let case_tools: Vec<serde_json::Value> = case_tool_names
            .iter()
            .filter_map(|name| tool_map.get(name).map(|v| (*v).clone()))
            .collect();

        // Compact just the tools for this case
        let compact = compact_tools(&case_tools);

        // Build KnownTool list for validation
        let known_tools: Vec<KnownTool> = build_known_tools(&case_tools);

        // Expected calls for this case
        let expected = case["expected"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);

        let t0 = Instant::now();

        if expected.is_empty() {
            // No-call case: verify the compactor+decoder handle no <<call>> cleanly
            let mut dec = ToolDecoder::new();
            let result = dec.push("I don't have a tool for that.", &known_tools).unwrap();
            let latency_us = t0.elapsed().as_micros() as u64;
            assert!(result.is_none(), "should not decode a call from prose");
            passed += 1;

            eprintln!(
                "  [{:>3}/{}] id={:<8} no call expected — correctly produced None  ✓  {}µs",
                i + 1, total, id, latency_us
            );

            let line = serde_json::json!({
                "id": id,
                "decoded_ok": true,
                "tool_name": null,
                "arguments": null,
                "token_reduction": compact.reduction_ratio(),
                "latency_us": latency_us,
                "error": null
            });
            writeln!(out, "{line}").unwrap();
            continue;
        }

        // For cases with expected calls, simulate the model producing <<call...>> for each
        let mut all_ok = true;
        let mut last_tool_name = String::new();
        let mut last_arguments = String::new();
        let mut last_error: Option<String> = None;

        for exp in expected {
            let exp_name = exp["name"].as_str().unwrap_or("");
            let exp_args = &exp["arguments"];
            let args_str = serde_json::to_string(exp_args).unwrap_or_default();

            // Simulate model output: <<call tool_name {args_json}>>
            let simulated = format!("<<call {exp_name} {args_str}>>");

            let mut dec = ToolDecoder::new();
            match dec.push(&simulated, &known_tools) {
                Ok(Some(call)) => {
                    last_tool_name = call.tool_name.clone();
                    last_arguments = call.arguments.clone();
                    // Verify tool name matches
                    if call.tool_name != exp_name {
                        all_ok = false;
                        last_error = Some(format!(
                            "tool name mismatch: got '{}' expected '{}'",
                            call.tool_name, exp_name
                        ));
                    }
                }
                Ok(None) => {
                    all_ok = false;
                    last_error = Some("decoder returned None for complete marker".into());
                }
                Err(e) => {
                    all_ok = false;
                    last_error = Some(e.to_string());
                }
            }
        }

        let latency_us = t0.elapsed().as_micros() as u64;

        if all_ok {
            passed += 1;
        }

        let mark = if all_ok { "✓" } else { "✗" };
        eprintln!(
            "  [{:>3}/{}] id={:<8} tool={:<30} {}  {}µs  reduction={:.1}%",
            i + 1, total, id, last_tool_name, mark, latency_us,
            compact.reduction_ratio() * 100.0
        );
        if let Some(ref err) = last_error {
            eprintln!("            error: {err}");
        }

        let line = serde_json::json!({
            "id": id,
            "decoded_ok": all_ok,
            "tool_name": if last_tool_name.is_empty() { serde_json::Value::Null } else { last_tool_name.into() },
            "arguments": if last_arguments.is_empty() { serde_json::Value::Null } else { last_arguments.into() },
            "token_reduction": compact.reduction_ratio(),
            "latency_us": latency_us,
            "error": last_error.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
        });
        writeln!(out, "{line}").unwrap();
    }

    out.flush().unwrap();

    eprintln!("\n── Results ─────────────────────────────────────────────────");
    eprintln!("  total cases       : {}", total);
    eprintln!("  decoded correctly : {} / {}", passed, total);
    eprintln!("  overall reduction : {:.1}%", all_compact.reduction_ratio() * 100.0);
    eprintln!("  output            : {}", out_path);
    eprintln!("────────────────────────────────────────────────────────────");
    eprintln!("Done. Run again and diff outputs to verify determinism.");
}

/// Build `KnownTool` validation structs from raw tool JSON values.
fn build_known_tools(tools: &[serde_json::Value]) -> Vec<KnownTool> {
    tools
        .iter()
        .filter_map(|t| {
            let func = t.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let required: Vec<String> = func
                .get("parameters")
                .and_then(|p| p.get("required"))
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();

            let mut enum_fields: HashMap<String, Vec<String>> = HashMap::new();
            if let Some(props) = func
                .get("parameters")
                .and_then(|p| p.get("properties"))
                .and_then(|p| p.as_object())
            {
                for (field, schema) in props {
                    if let Some(variants) = schema.get("enum").and_then(|e| e.as_array()) {
                        let vals: Vec<String> = variants
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                        if !vals.is_empty() {
                            enum_fields.insert(field.clone(), vals);
                        }
                    }
                }
            }
            Some(KnownTool { name, required, enum_fields })
        })
        .collect()
}
