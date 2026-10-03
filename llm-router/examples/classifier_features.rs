//! Dump the local classifier's features for a dataset — the input to its offline trainer.
//!
//! Run:
//!   DATASET=llm-router/classifier/data/train.json OUT=/tmp/train.features.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_features
//!
//! Reads `DATASET` (eval-set schema: an `examples` array) and writes one JSONL line per
//! example: `{"id", "hashed": [[bucket, value], ...], "dense": [...]}`. Training on this
//! output, rather than re-implementing the features in Python, is what guarantees the
//! trained weights match what the router computes at inference time.
use std::io::Write;

use nasiko_llm_router::routing::ClassifyInput;
use nasiko_llm_router::routing::classifier::linear::features;

fn main() {
    let path = std::env::var("DATASET").expect("set DATASET to a dataset JSON path");
    let out_path = std::env::var("OUT").expect("set OUT to the features JSONL path");
    let raw = std::fs::read_to_string(&path).expect("read DATASET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid dataset JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let input = ClassifyInput {
            query: example["query"].as_str().expect("query"),
            context: example["context"].as_str(),
        };
        let f = features(&input);
        let hashed: Vec<(usize, f64)> = f.hashed.into_iter().collect();
        let line = serde_json::json!({
            "id": example["id"],
            "hashed": hashed,
            "dense": f.dense,
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
}
