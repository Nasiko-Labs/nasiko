//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! The backend comes from the same environment the router reads (`CLASSIFIER_BACKEND`,
//! `CLASSIFIER_ENDPOINT`, `CLASSIFIER_TIMEOUT_MS`, …) and is built by the router's own
//! `build_request_classifier`, then called through the same `FallbackClassifier::decide`
//! the router uses — so this measures the production code path, fallbacks included. One
//! difference: this eval passes each case's `context`, while the live chat path does not yet
//! supply one (`context: None`). With nothing set it runs the regex baseline. A run summary (backend, load time, latency
//! percentiles, fallback counts) goes to stderr.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::{ClassifyInput, DecisionSource};
use nasiko_llm_router::{GatewayConfig, build_request_classifier};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    // Load once, before the loop; per-case latency excludes it.
    let load_started = Instant::now();
    let classifier = build_request_classifier(&GatewayConfig::from_env(), &reqwest::Client::new());
    let load_ms = load_started.elapsed().as_secs_f64() * 1e3;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut latencies_us = Vec::with_capacity(examples.len());
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let input = ClassifyInput {
            query: example["query"].as_str().expect("query"),
            context: example["context"].as_str(),
        };
        let started = Instant::now();
        let decision = classifier.decide(&input).await;
        let latency_us = started.elapsed().as_micros() as u64;
        latencies_us.push(latency_us);
        if decision.source != DecisionSource::Backend {
            eprintln!("{id}: {:?}", decision.source);
        }
        let c = decision.classification;
        let line = serde_json::json!({
            "id": id,
            "request_type": c.request_type.as_str(),
            "complexity": c.complexity,
            "confidence": c.confidence,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");

    let stats = classifier.stats();
    latencies_us.sort_unstable();
    eprintln!(
        "backend={} cases={} load_ms={load_ms:.1} p50_us={} p95_us={} fallbacks={} low_confidence={}",
        classifier.backend_name(),
        stats.decisions,
        percentile(&latencies_us, 0.50),
        percentile(&latencies_us, 0.95),
        stats.fallbacks,
        stats.low_confidence,
    );
}

/// Nearest-rank percentile of an ascending slice; 0 when empty.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}
