//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use nasiko_llm_router::routing::{
    classify_request_type, ClassifyInput, FallbackClassifier, HostedClassifier, RegexClassifier,
    RequestClassifier, SmartLocalClassifier,
};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let backend = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "local".into());

    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    // Load classifier once before the loop (per-call latency_us excludes model load).
    let classifier: Arc<dyn RequestClassifier> = match backend.as_str() {
        "regex" => Arc::new(RegexClassifier::new()),
        "hosted" => {
            let endpoint = std::env::var("CLASSIFIER_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:8000/v1/chat/completions".into());
            let model = std::env::var("CLASSIFIER_MODEL")
                .unwrap_or_else(|_| "gemini-2.0-flash".into());
            let hosted = HostedClassifier::new(endpoint, model);
            Arc::new(FallbackClassifier::new(
                Arc::new(hosted),
                Arc::new(RegexClassifier::new()),
            ))
        }
        _ => {
            // Default: SmartLocalClassifier with fallback wrapper
            Arc::new(FallbackClassifier::new(
                Arc::new(SmartLocalClassifier::new()),
                Arc::new(RegexClassifier::new()),
            ))
        }
    };

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example.get("context").and_then(|c| c.as_str());

        // Context-aware classification via RequestClassifier
        let input = ClassifyInput::with_context(query, context);
        let started = Instant::now();
        let res = classifier.classify(&input).await.unwrap_or_else(|_| {
            let fallback_type = classify_request_type(query);
            nasiko_llm_router::routing::Classification {
                request_type: fallback_type,
                complexity: 2,
                confidence: 0.60,
            }
        });
        let latency_us = started.elapsed().as_micros() as u64;

        let line = serde_json::json!({
            "id": id,
            "request_type": res.request_type.as_str(),
            "complexity": res.complexity,
            "confidence": (res.confidence * 100.0).round() / 100.0,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
