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

use nasiko_llm_router::routing::{ClassifyInput, build_classifier};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let backend = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".into());
    let model_path = std::env::var("CLASSIFIER_MODEL_PATH").unwrap_or_default();
    let timeout_ms = std::env::var("CLASSIFIER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let min_confidence = std::env::var("CLASSIFIER_MIN_CONFIDENCE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.30);
    // Construct/load once. Per-case timing below is inference only.
    let load_started = Instant::now();
    let classifier = build_classifier(&backend, &model_path, timeout_ms, min_confidence);
    let load_us = load_started.elapsed().as_micros();
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut latencies = Vec::with_capacity(examples.len());
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example.get("context").and_then(|value| value.as_str());
        let started = Instant::now();
        let classification = classifier.decide(&ClassifyInput { query, context }).await;
        let latency_us = started.elapsed().as_micros() as u64;
        latencies.push(latency_us);
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
    latencies.sort_unstable();
    let percentile = |fraction: f64| -> u64 {
        let index = ((latencies.len().saturating_sub(1) as f64) * fraction).floor() as usize;
        latencies.get(index).copied().unwrap_or(0)
    };
    eprintln!(
        "backend={} load_us={} p50_us={} p95_us={} fallbacks={}",
        classifier.backend_name(),
        load_us,
        percentile(0.50),
        percentile(0.95),
        classifier
            .fallbacks
            .error
            .load(std::sync::atomic::Ordering::Relaxed)
            + classifier
                .fallbacks
                .timeout
                .load(std::sync::atomic::Ordering::Relaxed)
            + classifier
                .fallbacks
                .invalid
                .load(std::sync::atomic::Ordering::Relaxed)
            + classifier
                .fallbacks
                .low_confidence
                .load(std::sync::atomic::Ordering::Relaxed),
    );
}
