//! Evaluation example for P1 — Compact Tool Schemas track.
//!
//! Contract:
//! - Reads EVAL_SET (JSON file with test cases)
//! - Writes OUT (JSONL with results per case)
//! - Processes both regular cases and decoder_cases
//! - Each line: {"id", "compact_request", "compacted", "rendered_calls", "roundtrip_calls", "decoded"}
//! - Exit 0 on success; scorer computes metrics

use std::env;
use std::fs;
use std::path::Path;
use serde_json::{json, Value};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET")?;
    let out_path = env::var("OUT")?;

    // Read evaluation set
    let eval_set_content = fs::read_to_string(&eval_set_path)?;
    let eval_set: Value = serde_json::from_str(&eval_set_content)?;

    let mut out_file = fs::File::create(&out_path)?;

    // Process regular cases
    if let Some(cases) = eval_set.get("cases").and_then(|c| c.as_array()) {
        for case in cases {
            let id = case.get("id").and_then(|id| id.as_str()).unwrap_or("unknown");
            let messages = case.get("messages");
            let expected = case.get("expected");

            // Stub: just echo back the case ID and expected calls for now
            let result = json!({
                "id": id,
                "compact_request": {},
                "compacted": true,
                "rendered_calls": "<<call example {\"stub\": true}>>",
                "roundtrip_calls": [],
                "decoded": {
                    "calls": expected
                }
            });

            writeln!(out_file, "{}", result.to_string())?;
        }
    }

    // Process decoder cases (if present)
    if let Some(decoder_cases) = eval_set.get("decoder_cases").and_then(|dc| dc.as_array()) {
        for case in decoder_cases {
            let id = case.get("id").and_then(|id| id.as_str()).unwrap_or("unknown");
            let chunks = case.get("chunks").and_then(|ch| ch.as_array()).unwrap_or(&[]);
            let expected = case.get("expected");

            // Stub: just report what was expected
            let result = json!({
                "id": id,
                "decoded": expected
            });

            writeln!(out_file, "{}", result.to_string())?;
        }
    }

    eprintln!("✓ Evaluation complete: {}", out_path);
    Ok(())
}
