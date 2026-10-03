//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::{
    create_classifier, ClassifyInput, RegexClassifier, RequestClassifier,
};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let backend = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "local".into());
    let classifier = create_classifier(Some(&backend));
    let fallback_regex = RegexClassifier;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();

        let input = ClassifyInput { query, context };
        let started = Instant::now();

        // Run classifier with regex fallback on failure/timeout
        let classification = match classifier.classify(&input).await {
            Ok(c) => c,
            Err(_) => fallback_regex.classify(&input).await.unwrap(),
        };

        let latency_us = started.elapsed().as_micros() as u64;

        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
