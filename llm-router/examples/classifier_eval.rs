//! `classifier_eval` — the `[classifier]` (request classifier) evaluation harness.
//!
//! Contract (see the track brief):
//!   * `EVAL_SET` — path to the eval JSON (`{examples: [...]}`). Required.
//!   * `OUT`      — path to write one JSONL line per example. Required.
//!   * `CLASSIFIER_BACKEND` — `regex` (default) or `heuristic`. Both run locally with no
//!     network; the regex backend is the out-of-the-box router default.
//!
//! Output line: `{"id", "request_type", "complexity", "confidence", "latency_us"}`.
//! The classifier is built once before the loop; `latency_us` is the per-call classify time
//! and excludes that one-time construction. The harness reports outputs; the grader scores.
//!
//! Run:
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   CLASSIFIER_BACKEND=heuristic \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```

use std::env;
use std::fs;
use std::time::Instant;

use serde_json::{json, Value};

use nasiko_llm_router::routing::{
    Classification, ClassifyInput, HeuristicClassifier, RegexClassifier, RequestClassifier,
};

#[tokio::main]
async fn main() {
    let eval_path = env::var("EVAL_SET").expect("EVAL_SET must point to the eval JSON file");
    let out_path = env::var("OUT").expect("OUT must point to the JSONL output path");
    let backend = env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".to_string());

    let raw = fs::read_to_string(&eval_path).expect("failed to read EVAL_SET");
    let data: Value = serde_json::from_str(&raw).expect("EVAL_SET is not valid JSON");

    // Build the classifier once, before the measured loop.
    let classifier: Box<dyn RequestClassifier> = match backend.as_str() {
        "heuristic" => Box::new(HeuristicClassifier),
        "regex" => Box::new(RegexClassifier),
        other => panic!("unknown CLASSIFIER_BACKEND `{other}` (expected regex|heuristic)"),
    };

    let mut lines: Vec<String> = Vec::new();
    for example in data.get("examples").and_then(Value::as_array).into_iter().flatten() {
        let id = example.get("id").and_then(Value::as_str).unwrap_or("");
        let query = example.get("query").and_then(Value::as_str).unwrap_or("");
        let context = example.get("context").and_then(Value::as_str);

        let input = ClassifyInput { query, context };
        let started = Instant::now();
        let result = classifier.classify(&input).await;
        let latency_us = started.elapsed().as_micros();

        // Fail-closed for the eval: a backend error falls back to the regex baseline so the
        // run always produces a line (mirrors the router's fallback policy).
        let Classification { request_type, complexity, confidence } = match result {
            Ok(c) => c,
            Err(_) => RegexClassifier
                .classify(&input)
                .await
                .expect("regex fallback never errors"),
        };

        lines.push(
            json!({
                "id": id,
                "request_type": request_type.as_str(),
                "complexity": complexity,
                "confidence": confidence,
                "latency_us": latency_us,
            })
            .to_string(),
        );
    }

    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&out_path, body).expect("failed to write OUT");
    eprintln!(
        "classifier_eval: backend={} wrote {} line(s) to {out_path}",
        classifier.name(),
        lines.len()
    );
}
