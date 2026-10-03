//! EVAL_SET=eval.json OUT=results.jsonl cargo run -p nasiko-llm-router --example classifier_eval
//! EVAL_SET is a JSON object containing an examples array with id, query, and context.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use nasiko_llm_router::routing::classifier::{ClassifyInput, classify_request_type};
use serde::Deserialize;
#[path = "../src/bin/classifier/config.rs"]
mod classifier_config;

#[derive(Deserialize)]
struct EvalInput {
    id: String,
    query: String,
    context: Option<String>,
}

#[derive(Deserialize)]
struct EvalSet {
    examples: Vec<EvalInput>,
}

fn read_eval_set(text: &str) -> Result<Vec<EvalInput>, serde_json::Error> {
    serde_json::from_str::<EvalSet>(text).map(|set| set.examples)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = std::env::var("EVAL_SET")?;
    let out_path = std::env::var("OUT")?;
    let inputs = read_eval_set(&std::fs::read_to_string(&eval_path)?)?;
    // Refuse to truncate the dataset when OUT names the input file or an alias of it.
    if std::path::Path::new(&out_path).exists()
        && std::fs::canonicalize(&eval_path)? == std::fs::canonicalize(&out_path)?
    {
        return Err("OUT must differ from EVAL_SET".into());
    }
    let load_start = Instant::now();
    let (classifier, stats) = classifier_config::ClassifierConfig::from_env()?.build();
    let load_us = load_start.elapsed().as_micros();
    // Initialize regex patterns and inference/thread-pool paths outside per-case timings.
    classify_request_type("");
    classifier
        .classify(&ClassifyInput {
            query: "warmup",
            context: None,
        })
        .await?;
    stats.reset();
    let mut output = BufWriter::new(File::create(out_path)?);
    let decisions = inputs.len();
    for entry in inputs {
        let input = ClassifyInput {
            query: &entry.query,
            context: entry.context.as_deref(),
        };
        let start = Instant::now();
        let result = classifier.classify(&input).await?;
        let latency_us = start.elapsed().as_micros();
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({
                "id": entry.id,
                "request_type": result.request_type.as_str(),
                "complexity": result.complexity,
                "confidence": result.confidence,
                "latency_us": latency_us,
            }),
        )?;
        writeln!(output)?;
    }
    output.flush()?;
    eprintln!(
        "backend={} load_us={} fallbacks={} decisions={}",
        classifier.name(),
        load_us,
        stats.fallbacks.load(std::sync::atomic::Ordering::Relaxed),
        decisions
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_brief_format_with_string_and_null_context() {
        let examples = read_eval_set(
            r#"{"examples":[{"id":"pub-01","query":"hello","context":"background"},{"id":"pub-02","query":"bye","context":null}]}"#,
        )
        .unwrap();
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[0].id, "pub-01");
        assert_eq!(examples[0].query, "hello");
        assert_eq!(examples[0].context.as_deref(), Some("background"));
        assert!(examples[1].context.is_none());
    }

    #[test]
    fn rejects_missing_examples_and_missing_ids() {
        assert!(read_eval_set("[]").is_err());
        assert!(read_eval_set(r#"{"examples":[{"query":"hello","context":null}]}"#).is_err());
    }
}
