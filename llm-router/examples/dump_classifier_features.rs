//! Dump request-classifier features for offline training.
//!
//! Run:
//!   DATASET=llm-router/training/request_classifier/data/labelled.jsonl \
//!   DUMP_OUT=llm-router/training/request_classifier/out/features.jsonl \
//!   cargo run --release -p nasiko-llm-router --example dump_classifier_features
//!
//! Reads labelled JSONL (`id`, `query`, `context`, `request_type`, `complexity`, `split`)
//! and writes one line per item with the exact feature vector the router's local backend
//! computes (`routing::request_features::extract`), so the trainer never re-implements a
//! feature. Output order equals input order.
use std::io::{BufRead, Write};
use std::time::Instant;

use nasiko_llm_router::routing::classify_request_type;
use nasiko_llm_router::routing::request_features::{extract, features_to_json, regex_vote_summary};

fn main() {
    let path = std::env::var("DATASET").expect("set DATASET to the labelled JSONL path");
    let out_path =
        std::env::var("DUMP_OUT").unwrap_or_else(|_| "classifier-features.jsonl".into());
    let started = Instant::now();
    let input = std::io::BufReader::new(std::fs::File::open(&path).expect("open DATASET"));
    let mut out =
        std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create DUMP_OUT"));
    let (mut n, mut nnz) = (0usize, 0usize);
    for line in input.lines() {
        let line = line.expect("read DATASET");
        if line.trim().is_empty() {
            continue;
        }
        let item: serde_json::Value = serde_json::from_str(&line).expect("valid JSONL line");
        let query = item["query"].as_str().expect("query");
        let context = item["context"].as_str();
        let features = extract(query, context);
        let (_, votes_total) = regex_vote_summary(query);
        let mut record = features_to_json(&features);
        record["id"] = item["id"].clone();
        record["split"] = item
            .get("split")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::from("unsplit"));
        record["y_type"] = item["request_type"].clone();
        record["y_cx"] = item["complexity"].clone();
        record["regex_type"] = classify_request_type(query).as_str().into();
        record["regex_votes_total"] = votes_total.into();
        writeln!(out, "{record}").expect("write DUMP_OUT");
        n += 1;
        nnz += features.hashed.len();
    }
    out.flush().expect("flush DUMP_OUT");
    eprintln!(
        "dump_classifier_features: {n} items, mean nnz {:.1}, {} ms -> {out_path}",
        nnz as f64 / n.max(1) as f64,
        started.elapsed().as_millis()
    );
}
