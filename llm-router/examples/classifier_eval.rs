//! Request classifier evaluation (regex default; opt-in local or hosted model).
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::classifier::ClassifyInput;
#[path = "../src/bin/config.rs"]
mod config;

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    let cfg = config::ClassifierConfig::from_env().expect("valid classifier configuration");
    let load = Instant::now();
    cfg.runtime.prepare(cfg.load_timeout).await;
    eprintln!(
        "backend={} load_ms={}",
        cfg.runtime.classifier.name(),
        load.elapsed().as_millis()
    );

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = match example.get("context") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.as_str()),
            _ => panic!("context must be a string or null"),
        };
        let started = Instant::now();
        let outcome = cfg.runtime.run(&ClassifyInput { query, context }).await;
        let latency_us = started.elapsed().as_micros() as u64;
        let line = serde_json::json!({
            "id": id,
            "request_type": outcome.classification.request_type.as_str(),
            "complexity": outcome.classification.complexity,
            "confidence": outcome.classification.confidence,
            "latency_us": latency_us,
        });
        writeln!(out, "{line}").expect("write OUT");
        if let Some(reason) = outcome.fallback {
            eprintln!("fallback id={id} reason={}", reason.as_str());
        }
    }
    out.flush().expect("flush OUT");
    eprintln!(
        "fallbacks={}",
        cfg.runtime
            .fallbacks
            .load(std::sync::atomic::Ordering::Relaxed)
    );
}
