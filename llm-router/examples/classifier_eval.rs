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

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::classifier::build_classifier;

#[tokio::main]
async fn main() {
    let cfg = GatewayConfig::from_env();
    let load_started = Instant::now();
    let classifier = build_classifier(&cfg, reqwest::Client::new());
    eprintln!(
        "classifier_backend={} model={} local_init_us={}",
        cfg.classifier_backend,
        cfg.classifier_model,
        load_started.elapsed().as_micros()
    );
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let started = Instant::now();
        let result = classifier
            .classify(query, example["context"].as_str().unwrap_or(""))
            .await
            .expect("guarded classification");
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": result.request_type.as_str(),
            "complexity": result.complexity,
            "confidence": result.confidence,
            "fallback_reason": result.fallback_reason,
            "decision_cost_usd": result.decision_cost_usd,
            "backend": cfg.classifier_backend,
            "effective_backend": if result.fallback_reason.is_some() { "regex" } else { classifier.name() },
            "model": if cfg.classifier_backend == "regex" { "regex" } else { &cfg.classifier_model },
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
