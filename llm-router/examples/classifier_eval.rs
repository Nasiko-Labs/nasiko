//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! The backend is built by the same `build_request_classifier` the router uses, from the
//! same env vars: `CLASSIFIER_BACKEND=regex|nb` (default `regex`),
//! `CLASSIFIER_CONFIDENCE_THRESHOLD` (default 0.35), `CLASSIFIER_TIMEOUT_MS` (default 50).
//! Each case goes through the router's timeout + regex-fallback guard. One-time load
//! (training, for `nb`) happens before the loop and is excluded from `latency_us`; it and
//! the fallback count are reported on stderr only.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::ClassifyInput;
use nasiko_llm_router::{GatewayConfig, build_request_classifier};

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");

    let cfg = GatewayConfig::from_env();
    let load_started = Instant::now();
    let classifier = build_request_classifier(&cfg);
    let load_us = load_started.elapsed().as_micros();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str().filter(|c| !c.is_empty());
        let input = ClassifyInput { query, context };
        let started = Instant::now();
        let (c, _outcome) = runtime.block_on(classifier.classify(&input));
        let latency_us = started.elapsed().as_micros() as u64;
        // Rounded so the line is byte-identical across runs and platforms.
        let confidence = (c.confidence as f64 * 10_000.0).round() / 10_000.0;
        let line = serde_json::json!({
            "id": id,
            "request_type": c.request_type.as_str(),
            "complexity": c.complexity,
            "confidence": confidence,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    eprintln!(
        "classifier_eval: backend={} cases={} load_us={} fallbacks={}",
        classifier.backend_name(),
        examples.len(),
        load_us,
        classifier.fallbacks()
    );
}
