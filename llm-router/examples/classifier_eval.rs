//! Request classifier eval (regex baseline).
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::hosted_classifier::regex_fallback;

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let expected_request_type = example["expected_request_type"].as_str();
        let expected_complexity = example["expected_complexity"].as_u64();
        let started = Instant::now();
        let classified = regex_fallback(query);
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": classified.request_type.as_str(),
            "complexity": classified.complexity,
            "confidence": classified.confidence,
            "hosted": false,
            "expected_request_type": expected_request_type,
            "expected_complexity": expected_complexity,
            "request_type_correct": expected_request_type.is_some_and(|expected| expected == classified.request_type.as_str()),
            "complexity_correct": expected_complexity.is_some_and(|expected| expected == u64::from(classified.complexity)),
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
