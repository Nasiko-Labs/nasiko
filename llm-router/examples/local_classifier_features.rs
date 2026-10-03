//! Dumps the local classifier's features for a training set, so `eval/train_local.py` trains
//! on exactly the features the router computes — Rust is the single source of truth.
//!
//! Run:
//!   cargo run --release -p nasiko-llm-router --example local_classifier_features -- \
//!     eval/train-a.jsonl eval/train-b.jsonl > /tmp/features.jsonl
//!
//! Input: JSONL rows `{id, query, context?, request_type, complexity}`.
//! Output: JSONL rows `{id, request_type, complexity, features: [[bucket, value], ...]}`.
use std::io::{BufRead, Write};

use nasiko_llm_router::routing::ClassifierInput;
use nasiko_llm_router::routing::local_classifier::{eval_state, features_for};

fn main() {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for path in std::env::args().skip(1) {
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
        for line in std::io::BufReader::new(file).lines() {
            let line = line.expect("read line");
            if line.trim().is_empty() {
                continue;
            }
            let row: serde_json::Value = serde_json::from_str(&line).expect("valid JSONL row");
            let query = row["query"].as_str().expect("query");
            let state = eval_state(query, row["context"].as_str());
            let feats = features_for(&ClassifierInput {
                query,
                state: &state,
            });
            let dumped = serde_json::json!({
                "id": row["id"],
                "request_type": row["request_type"],
                "complexity": row["complexity"],
                "features": feats.iter().map(|(b, v)| serde_json::json!([b, v])).collect::<Vec<_>>(),
            });
            writeln!(out, "{dumped}").expect("write");
        }
    }
    out.flush().expect("flush");
}
