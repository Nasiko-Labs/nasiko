//! Run the configured RequestClassifier over the classifier-eval dataset.
//!
//! `EVAL_SET` is a JSON file with an `examples` array (each row: `id`, `query`, optional
//! `context`). `OUT` receives one JSONL line per case:
//! `{"id","request_type","complexity","confidence","latency_us"}`.
//! Backend is chosen by `REQUEST_CLASSIFIER_BACKEND` (see `server/.env.example`); regex by default.
//! A backend error never aborts the run: the case is answered by the regex baseline instead.
//!
//! Usage: EVAL_SET=in.json OUT=out.jsonl cargo run --release -p nasiko-llm-router --example classifier_eval

use std::{
    env,
    error::Error,
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
};

use nasiko_llm_router::{
    GatewayConfig,
    routing::request_classifier::{self, RegexRequestClassifier, RequestClassifier},
};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let input = env::var("EVAL_SET").map_err(|_| "set EVAL_SET to the eval JSON path")?;
    let output = env::var("OUT").map_err(|_| "set OUT to the output JSONL path")?;

    let doc: Value = serde_json::from_reader(File::open(&input)?)?;
    let examples = doc
        .get("examples")
        .and_then(Value::as_array)
        .ok_or("EVAL_SET must be a JSON object with an `examples` array")?;

    // Built once, before the loop: per-case latency excludes setup.
    let classifier = request_classifier::from_config(&GatewayConfig::from_env());
    let regex = RegexRequestClassifier;
    let mut writer = BufWriter::new(File::create(&output)?);
    let mut fallbacks = 0usize;

    for (i, row) in examples.iter().enumerate() {
        let query = row
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("example {i} is missing string field `query`"))?;
        let context = row.get("context").and_then(Value::as_str);

        let start = Instant::now();
        let prediction = match classifier.classify(query, context).await {
            Ok(p) => p,
            Err(error) => {
                fallbacks += 1;
                eprintln!("example {i}: classifier failed ({error}); using regex");
                regex
                    .classify(query, context)
                    .await
                    .map_err(std::io::Error::other)?
            }
        };
        let latency_us = start.elapsed().as_micros() as u64;

        let line = json!({
            "id": row.get("id").cloned().unwrap_or_else(|| json!(format!("case-{i}"))),
            "request_type": prediction.request_type.as_str(),
            "complexity": prediction.complexity,
            "confidence": prediction.confidence,
            "latency_us": latency_us,
        });
        serde_json::to_writer(&mut writer, &line)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    eprintln!(
        "backend={} {} cases, {} fallbacks (backend-reported: {}) -> {output}",
        classifier.name(),
        examples.len(),
        fallbacks,
        classifier.fallback_count()
    );
    Ok(())
}
