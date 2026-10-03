use std::io::Write;
use std::time::{Duration, Instant};

use nasiko_llm_router::routing::{build_request_classifier, classify_with_classifier};

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());

    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let client = reqwest::Client::new();

    let classifier = build_request_classifier("regex", "", "", "", Duration::from_secs(5), client);

    let cells = std::collections::HashMap::new();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");

        let started = Instant::now();

        let (_tier, classification) = futures::executor::block_on(classify_with_classifier(
            classifier.as_ref(),
            query,
            "eval",
            &cells,
        ));

        let latency_us = started.elapsed().as_micros() as u64;

        let line = serde_json::json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "latency_us": latency_us,
        });

        writeln!(out, "{line}").expect("write OUT");
    }

    out.flush().expect("flush OUT");
}
