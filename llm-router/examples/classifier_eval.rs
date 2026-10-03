//! Request classifier eval: exercises the same `RequestClassifier` decision
//! interface the router uses.
//!
//! Run (offline default — regex backend, no network, no keys):
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Backend selection (all optional; library never reads env itself):
//!   CLASSIFIER_BACKEND=regex|hosted   (default: regex)
//!   CLASSIFIER_ENDPOINT=https://…/v1  (hosted base URL, no trailing path)
//!   CLASSIFIER_MODEL=<model id>
//!   CLASSIFIER_TIMEOUT_MS=2000
//!   CLASSIFIER_API_KEY | PROVIDER_API_KEY | OPENAI_API_KEY (bearer, if set)
//!
//! Reads `examples` from `EVAL_SET` and writes one JSONL line per case to
//! `OUT`: `{"id","request_type","complexity","confidence","latency_us"}`.
//! It does not compute scores; the scorer does that. The classifier is built
//! once before the loop, so per-call `latency_us` excludes load. Any hosted
//! failure falls back to the regex (counted on stderr, never in `OUT`).
//! Deterministic with the default backend: two runs diff clean.
use std::io::Write;
use std::time::{Duration, Instant};

use nasiko_llm_router::routing::{ClassifierService, ClassifyInput};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let backend = env_or("CLASSIFIER_BACKEND", "regex");
    let endpoint = env_or("CLASSIFIER_ENDPOINT", "");
    let model = env_or("CLASSIFIER_MODEL", "");
    let timeout_ms: u64 = std::env::var("CLASSIFIER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let api_key = std::env::var("CLASSIFIER_API_KEY")
        .or_else(|_| std::env::var("PROVIDER_API_KEY"))
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok()
        .filter(|s| !s.is_empty());
    let http = reqwest::Client::builder().build().expect("http client");
    let service = ClassifierService::from_config(
        &backend,
        &endpoint,
        &model,
        api_key.as_deref(),
        Duration::from_millis(timeout_ms),
        http,
    );
    eprintln!(
        "classifier_eval: backend={} timeout_ms={} cases={}",
        service.backend_name(),
        timeout_ms,
        examples.len()
    );

    // Warm up once before the timed loop (regex set compilation), so
    // per-call `latency_us` excludes load, like a model load would.
    service
        .classify_detailed(&ClassifyInput {
            query: "warm up",
            context: None,
        })
        .await;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();
        let started = Instant::now();
        let (decision, _) = service
            .classify_detailed(&ClassifyInput { query, context })
            .await;
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": decision.request_type.as_str(),
            "complexity": decision.complexity,
            "confidence": decision.confidence,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    eprintln!(
        "classifier_eval: wrote {} lines, fallbacks={}",
        examples.len(),
        service.fallbacks()
    );
}
