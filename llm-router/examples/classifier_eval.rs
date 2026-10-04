//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET`, instantiates the configured classifier once, and writes
//! a JSONL row per example with the required fields plus the structured profile.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::{ClassifyInput, build_request_classifier};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let cfg = GatewayConfig::default();
    let classifier = build_request_classifier(&cfg);
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();

        let started = Instant::now();
        let classification = classifier
            .classify(&ClassifyInput {
                query,
                context,
                codebase_size: None,
                cache_affinity: None,
            })
            .await
            .expect("classify request");
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "profile": {
                "reasoning_depth": classification.profile.reasoning_depth,
                "coding_complexity": classification.profile.coding_complexity,
                "debugging_difficulty": classification.profile.debugging_difficulty,
                "tool_orchestration": classification.profile.tool_orchestration,
                "conversation_dependency": classification.profile.conversation_dependency,
                "change_impact": classification.profile.change_impact,
            },
            "context": {
                "codebase_size": classification.context.codebase_size,
                "context_size": classification.context.context_size,
                "cache_affinity": classification.context.cache_affinity,
            },
            "latency_us": latency_us,
            "fallback": classification.fallback,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
