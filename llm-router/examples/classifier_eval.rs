//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! Backend selection: `CLASSIFIER_BACKEND=regex|heuristic` (default: heuristic).
//! Both backends are local and deterministic: the run needs no network and
//! produces byte-identical output across runs.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::{ClassifyInput, classifier_from_backend};

fn main() {
    let backend_name = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "heuristic".into());
    let backend = classifier_from_backend(&backend_name);
    eprintln!("[classifier-eval] backend: {}", backend.name());

    let path = std::env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/classifier-eval.json".into());
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("[classifier-eval] cannot read EVAL_SET={path}: {e}");
            std::process::exit(1);
        }
    };
    let data: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("[classifier-eval] invalid eval JSON: {e}");
            std::process::exit(1);
        }
    };
    let examples = match data["examples"].as_array() {
        Some(examples) => examples,
        None => {
            eprintln!("[classifier-eval] eval JSON has no 'examples' array");
            std::process::exit(1);
        }
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&out_path).unwrap_or_else(|e| {
            eprintln!("[classifier-eval] cannot create OUT={out_path}: {e}");
            std::process::exit(1);
        }),
    );
    for example in examples {
        let id = example["id"].as_str().unwrap_or("unknown");
        let query = example["query"].as_str().unwrap_or("");
        let context = example["context"].as_str();
        let started = Instant::now();
        let result = rt.block_on(backend.classify(&ClassifyInput { query, context }));
        let latency_us = started.elapsed().as_micros() as u64;
        // Any backend error falls back to the regex baseline, exactly like the
        // router does on timeout or inference failure.
        let (request_type, complexity, confidence) = match result {
            Ok(c) => (
                c.request_type.as_str().to_string(),
                serde_json::json!(c.complexity),
                serde_json::json!(c.confidence),
            ),
            Err(e) => {
                eprintln!("[classifier-eval] {id}: backend error ({e}), regex fallback");
                let fallback = rt.block_on(
                    classifier_from_backend("regex").classify(&ClassifyInput { query, context }),
                );
                match fallback {
                    Ok(c) => (
                        c.request_type.as_str().to_string(),
                        serde_json::json!(c.complexity),
                        serde_json::json!(c.confidence),
                    ),
                    Err(_) => ("general".to_string(), serde_json::json!(2), serde_json::json!(0.3)),
                }
            }
        };
        let line = serde_json::json!({
            "id": id,
            "request_type": request_type,
            "complexity": complexity,
            "confidence": confidence,
            "latency_us": latency_us,
        });
        if writeln!(out, "{line}").is_err() {
            eprintln!("[classifier-eval] failed writing OUT");
            std::process::exit(1);
        }
    }
    if out.flush().is_err() {
        eprintln!("[classifier-eval] failed flushing OUT");
        std::process::exit(1);
    }
    eprintln!("[classifier-eval] wrote {} cases to {out_path}", examples.len());
}
