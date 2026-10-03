//! Evaluation example for P2 — Request Classifier track.
//!
//! Contract:
//! - Reads EVAL_SET (JSON file with test queries)
//! - Writes OUT (JSONL with classifications per query)
//! - Each line: {"id", "request_type", "complexity", "confidence", "latency_us"}
//! - Exit 0 on success; scorer computes metrics
//!
//! This baseline uses regex patterns (the existing classifier).

use std::env;
use std::fs;
use std::time::Instant;
use serde_json::{json, Value};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET")?;
    let out_path = env::var("OUT")?;

    // Read evaluation set
    let eval_set_content = fs::read_to_string(&eval_set_path)?;
    let eval_set: Value = serde_json::from_str(&eval_set_content)?;

    let mut out_file = fs::File::create(&out_path)?;

    // Process cases
    if let Some(examples) = eval_set.get("examples").and_then(|e| e.as_array()) {
        for example in examples {
            let id = example.get("id").and_then(|id| id.as_str()).unwrap_or("unknown");
            let query = example.get("query").and_then(|q| q.as_str()).unwrap_or("");
            let context = example.get("context").and_then(|c| c.as_str());

            let start = Instant::now();

            // Stub regex classifier: match on keywords
            let request_type = classify_request_type(query);
            let complexity = estimate_complexity(query, context);
            let confidence = if query.len() > 100 { 0.85 } else { 0.95 };

            let latency_us = start.elapsed().as_micros() as u64;

            let result = json!({
                "id": id,
                "request_type": request_type,
                "complexity": complexity,
                "confidence": confidence,
                "latency_us": latency_us
            });

            writeln!(out_file, "{}", result.to_string())?;
        }
    }

    eprintln!("✓ Classification complete: {}", out_path);
    Ok(())
}

fn classify_request_type(query: &str) -> String {
    let lower = query.to_lowercase();

    if lower.contains("code") || lower.contains("function") || lower.contains("implement") {
        "code_generation".to_string()
    } else if lower.contains("why") || lower.contains("explain") || lower.contains("understand") {
        "code_understanding".to_string()
    } else if lower.contains("design") || lower.contains("architecture") {
        "technical_design".to_string()
    } else if lower.contains("analyze") || lower.contains("compare") {
        "analytical_reasoning".to_string()
    } else if lower.contains("write") || lower.contains("edit") {
        "writing".to_string()
    } else if lower.contains("what") || lower.contains("which") {
        "factual_lookup".to_string()
    } else {
        "general".to_string()
    }
}

fn estimate_complexity(query: &str, context: Option<&str>) -> u8 {
    let mut complexity = 1u8;

    if query.len() > 200 {
        complexity += 1;
    }
    if let Some(ctx) = context {
        if ctx.len() > 500 {
            complexity += 1;
        }
    }
    if query.contains("multi") || query.contains("nested") {
        complexity += 1;
    }

    std::cmp::min(complexity, 5)
}
