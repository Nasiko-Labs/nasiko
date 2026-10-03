//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! Builds the classifier through [`nasiko_llm_router::build_classifier`] — the exact
//! function `LlmRouterCtx::from_shared` calls in production — driven by
//! `GatewayConfig::from_env()`'s `CLASSIFIER_*` env vars, so this exercises the same code
//! path as the router rather than a parallel reimplementation of it. Unset
//! `CLASSIFIER_BACKEND` (or any value other than `"hosted"`) ⇒ the regex backend: no
//! network, deterministic. Set `CLASSIFIER_BACKEND=hosted` (plus `CLASSIFIER_ENDPOINT`,
//! `CLASSIFIER_MODEL`, and optionally `CLASSIFIER_API_KEY`/`CLASSIFIER_TIMEOUT_MS`) to
//! measure the hosted backend instead; any hosted failure or timeout falls back to the
//! regex result (see `FallbackClassifier`), so this still completes and writes a line for
//! every case.
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use nasiko_llm_router::{ClassifyInput, GatewayConfig, build_classifier};

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    eprintln!("classifier_eval: EVAL_SET={path} OUT={out_path}");

    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    eprintln!("classifier_eval: loaded {} example(s) from EVAL_SET", examples.len());

    // Load once, before the loop — latency below excludes backend/model load time.
    let cfg = Arc::new(GatewayConfig::from_env());
    let classifier = build_classifier(&cfg, reqwest::Client::new());
    eprintln!(
        "classifier_eval: backend = {} (CLASSIFIER_BACKEND={:?})",
        classifier.name(),
        std::env::var("CLASSIFIER_BACKEND").unwrap_or_default()
    );

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for (i, example) in examples.iter().enumerate() {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example.get("context").and_then(|c| c.as_str());
        eprintln!("classifier_eval: [{}/{}] classifying {id:?}...", i + 1, examples.len());

        let started = Instant::now();
        let result = classifier
            .classify(&ClassifyInput { query, context })
            .await;
        let latency_us = started.elapsed().as_micros() as u64;
        eprintln!("classifier_eval: [{}/{}] {id:?} done in {latency_us}us", i + 1, examples.len());

        let line = match result {
            Ok(c) => serde_json::json!({
                "id": id,
                "request_type": c.request_type.as_str(),
                "complexity": c.complexity,
                "confidence": c.confidence,
                "latency_us": latency_us,
            }),
            // Every classifier `build_classifier` wires (regex directly, or hosted wrapped
            // in FallbackClassifier) is infallible in practice — kept for honesty rather
            // than `.expect()`, so a caller-supplied raw classifier can't crash the run.
            Err(e) => serde_json::json!({
                "id": id,
                "error": e.to_string(),
                "latency_us": latency_us,
            }),
        };
        writeln!(out, "{line}").expect("write OUT");
        out.flush().expect("flush OUT after each case");
    }

    eprintln!(
        "classifier_eval: wrote {} line(s) to {out_path} (backend = {})",
        examples.len(),
        classifier.name()
    );
}
