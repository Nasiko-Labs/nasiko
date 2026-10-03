//! Request classifier evaluation using the same configured trait as router requests.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   CLASSIFIER_BACKEND=local \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! The regex backend is the default. The evaluator writes one JSONL prediction per case;
//! scoring remains the harness's responsibility.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nasiko_llm_router::routing::{ClassifyInput, RequestClassifier, configured_request_classifier};

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = env_or("OUT", "classifier-out.jsonl");
    let backend = env_or("CLASSIFIER_BACKEND", "regex");
    let model_path = env_or("CLASSIFIER_MODEL_PATH", "");
    let timeout_ms = env_or("CLASSIFIER_TIMEOUT_MS", "50")
        .parse::<u64>()
        .expect("CLASSIFIER_TIMEOUT_MS must be an unsigned integer");
    let min_confidence = env_or("CLASSIFIER_MIN_CONFIDENCE", "0.50")
        .parse::<f32>()
        .expect("CLASSIFIER_MIN_CONFIDENCE must be a float");
    assert!(
        min_confidence.is_finite() && (0.0..=1.0).contains(&min_confidence),
        "CLASSIFIER_MIN_CONFIDENCE must be in [0, 1]"
    );

    let classifier: Arc<dyn RequestClassifier> = configured_request_classifier(
        &backend,
        Some(&model_path),
        Duration::from_millis(timeout_ms),
        min_confidence,
    );
    eprintln!(
        "classifier_eval: backend={} fallback_count_start={}",
        classifier.name(),
        classifier.fallback_count()
    );

    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create Tokio runtime");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();
        let fallbacks_before = classifier.fallback_count();
        let started = Instant::now();
        let classification = runtime
            .block_on(classifier.classify(&ClassifyInput { query, context }))
            .expect("configured classifier must fall back instead of returning errors");
        let latency_us = started.elapsed().as_micros() as u64;
        let used_fallback = classifier.fallback_count() > fallbacks_before;
        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "latency_us": latency_us,
            "fallback": used_fallback,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
