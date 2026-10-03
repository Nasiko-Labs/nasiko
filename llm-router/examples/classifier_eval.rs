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
    MlRequestClassifier, RegexClassifier, RequestClassifier,
};

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let classifier: Box<dyn RequestClassifier> = match std::env::var("CLASSIFIER_ENABLED").as_deref() {
        Ok("true" | "1") => {
            let threshold = std::env::var("CLASSIFIER_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.42);
            let timeout_ms = std::env::var("CLASSIFIER_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100);
            let weights_path = std::env::var("CLASSIFIER_WEIGHTS_PATH").unwrap_or_default();
            if weights_path.is_empty() {
                Box::new(
                    MlRequestClassifier::embedded(threshold, timeout_ms)
                        .expect("embedded weights failed to load"),
                )
            } else {
                Box::new(
                    MlRequestClassifier::from_path(&weights_path, threshold, timeout_ms)
                        .expect("failed to load weights from CLASSIFIER_WEIGHTS_PATH"),
                )
            }
        }
        _ => Box::new(RegexClassifier),
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example.get("context").and_then(|c| c.as_str());

        let started = Instant::now();
        let classification = classifier.classify(query, context);
        let latency_us = started.elapsed().as_micros() as u64;

        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": if classification.complexity == 0 {
                serde_json::Value::Null
            } else {
                serde_json::json!(classification.complexity)
            },
            "confidence": if (classification.confidence - 1.0).abs() < 1e-9 && classification.complexity == 0 {
                serde_json::Value::Null
            } else {
                serde_json::json!(classification.confidence)
            },
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
