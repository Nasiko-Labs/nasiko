//! Evaluate the configured request classifier with the same backend/fallback policy used
//! by routing. The sidecar loads the fitted model once at service startup.
//!
//! Run with `EVAL_SET=... OUT=... REQUEST_CLASSIFIER_BACKEND=llmrouter_knn cargo run
//! --release -p nasiko-llm-router --example classifier_eval`.
use std::io::Write;
use std::time::{Duration, Instant};

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::request_classifier::{
    ClassifyInput, classify_with_fallback, configured_request_classifier,
};
use serde_json::Value;

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    let cfg = GatewayConfig::from_env();
    let client = reqwest::Client::new();
    let backend = configured_request_classifier(&cfg, client);
    let timeout = Duration::from_millis(cfg.request_classifier_timeout_ms);
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();
        let input = ClassifyInput { query, context };
        let started = Instant::now();
        let (classification, did_fallback) = classify_with_fallback(
            backend.as_ref(),
            &input,
            timeout,
            cfg.request_classifier_min_confidence,
        )
        .await;
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "fallback": did_fallback,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
