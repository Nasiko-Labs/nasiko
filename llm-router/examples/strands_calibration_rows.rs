//! Export train-only calibration requests using the exact Rust rubric and state builder.
use nasiko_llm_router::routing::laya::request_body;
use nasiko_llm_router::routing::local_classifier::eval_state;
use std::io::{BufRead, Write};

fn main() {
    let input = std::env::args().nth(1).expect("training JSONL path");
    // Reuse the exact rubric to warm both native and type-only comparison paths.
    // Calibration collection retains its default single-question behavior.
    let native_complexity = std::env::args().any(|arg| arg == "--native-complexity");
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in std::io::BufReader::new(std::fs::File::open(input).expect("training file")).lines()
    {
        let line = line.expect("line");
        if line.trim().is_empty() {
            continue;
        }
        let mut row: serde_json::Value = serde_json::from_str(&line).expect("training row");
        let state = eval_state(
            row["query"].as_str().expect("query"),
            row["context"].as_str(),
        );
        let mut body = request_body(&state);
        body["model"] = "strands-decider-latest".into();
        if !native_complexity {
            body["questions"]
                .as_object_mut()
                .expect("questions")
                .remove("complexity");
        }
        row["body"] = body;
        writeln!(out, "{row}").expect("write");
    }
}
