//! Request-classifier evaluation harness.
//!
//! ```text
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! Reads `EVAL_SET` (`{"examples": [{"id", "query", "context"?, ...}]}`) and writes one JSONL line
//! per case to `OUT`: `{"id","request_type","complexity","confidence","latency_us"}`.
//!
//! Backend selection uses the router's own configuration and builder
//! ([`GatewayConfig::from_env`] + [`build_classifier`]), so this exercises the exact code path
//! the router runs, including timeout, regex fallback and the low-confidence safe default:
//!
//! | env | meaning |
//! |---|---|
//! | `CLASSIFIER_BACKEND` | `regex` (default) \| `local` \| `hosted` |
//! | `CLASSIFIER_MODEL_PATH` | `local`: JSONL training set instead of the embedded one |
//! | `CLASSIFIER_ENDPOINT` / `CLASSIFIER_MODEL` / `CLASSIFIER_API_KEY` | `hosted` |
//! | `CLASSIFIER_TIMEOUT_MS` | per-decision budget (default 250) |
//! | `CLASSIFIER_MIN_CONFIDENCE` | below this, answer `general` (default 0 = off) |
//!
//! The model is built once before the loop; `latency_us` times only `classify`. A summary
//! (p50/p95, fallback counts and, when the set carries labels, accuracy and ECE) goes to stderr.
//! Exit code 0 means the eval ran.

use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::GatewayConfig;
use nasiko_llm_router::build_classifier;
use nasiko_llm_router::routing::{ClassifyInput, RequestClassifier};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = std::env::var("EVAL_SET").map_err(|_| "EVAL_SET is required")?;
    let out_path = std::env::var("OUT").map_err(|_| "OUT is required")?;
    let set: Value = serde_json::from_str(&std::fs::read_to_string(&eval_path)?)?;
    let cases = set["examples"]
        .as_array()
        .ok_or("EVAL_SET needs an `examples` array")?;

    let cfg = GatewayConfig::from_env();
    // Load once, before timing anything.
    let load_start = Instant::now();
    let classifier = build_classifier(&cfg, reqwest::Client::new());
    let load_ms = load_start.elapsed().as_secs_f64() * 1e3;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path)?);
    let mut lat: Vec<u128> = Vec::with_capacity(cases.len());
    let (mut labelled, mut correct, mut fallbacks) = (0usize, 0usize, 0usize);
    let mut bins = [(0usize, 0f64, 0usize); 10]; // (n, sum_conf, n_correct) for ECE

    for (i, case) in cases.iter().enumerate() {
        let id = case["id"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("case-{i}"));
        let query = case["query"].as_str().unwrap_or_default();
        let context = case["context"].as_str();
        let start = Instant::now();
        let result = classifier.classify(&ClassifyInput { query, context }).await;
        let us = start.elapsed().as_micros();
        lat.push(us);
        // The guard never returns Err; handle it anyway so the harness can't abort mid-run.
        let c = match result {
            Ok(c) => c,
            Err(_) => {
                fallbacks += 1;
                nasiko_llm_router::routing::RegexClassifier
                    .classify(&ClassifyInput { query, context })
                    .await
                    .expect("regex is infallible")
            }
        };
        writeln!(
            out,
            "{}",
            json!({
                "id": id, "request_type": c.request_type.as_str(), "complexity": c.complexity,
                "confidence": (c.confidence * 1e4).round() / 1e4, "latency_us": us as u64,
            })
        )?;
        if let Some(label) = case["request_type"].as_str() {
            labelled += 1;
            let ok = label == c.request_type.as_str();
            correct += ok as usize;
            let b = ((c.confidence * 10.0) as usize).min(9);
            bins[b].0 += 1;
            bins[b].1 += c.confidence as f64;
            bins[b].2 += ok as usize;
        }
    }
    out.flush()?;

    lat.sort_unstable();
    let pct = |p: f64| {
        lat.get(((lat.len() as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0)
    };
    eprintln!(
        "backend={} cases={} load_ms={load_ms:.2}",
        classifier.name(),
        cases.len()
    );
    eprintln!(
        "latency_us p50={} p95={} max={}",
        pct(0.5),
        pct(0.95),
        lat.last().copied().unwrap_or(0)
    );
    eprintln!(
        "harness_fallbacks={fallbacks} (guard fallbacks are counted inside the router build)"
    );
    if labelled > 0 {
        let ece: f64 = bins
            .iter()
            .filter(|b| b.0 > 0)
            .map(|b| {
                (b.0 as f64 / labelled as f64)
                    * ((b.1 / b.0 as f64) - (b.2 as f64 / b.0 as f64)).abs()
            })
            .sum();
        eprintln!(
            "labelled={labelled} accuracy={:.4} ece={ece:.4}",
            correct as f64 / labelled as f64
        );
    }
    Ok(())
}