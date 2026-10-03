//! Deterministic request-classifier evaluation runner.
//!
//! `CLASSIFIER_BACKEND=regex` is the default compatibility baseline. Set it to `local`
//! to evaluate the context-aware, no-network backend. Unknown/hosted backends and low
//! confidence classifications fall back to the regex classifier.

use std::{
    env, fs,
    io::Write,
    time::{Duration, Instant},
};

use nasiko_llm_router::routing::classifier::{
    Classification, ClassifyInput, RegexRequestClassifier, RequestClassifier, builtin_classifier,
    classify_with_fallback,
};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/classifier-eval.json".into());
    let out_path = env::var("OUT").unwrap_or_else(|_| "/tmp/classifier-out.jsonl".into());
    let backend = env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".into());
    let timeout = Duration::from_millis(env_u64("CLASSIFIER_TIMEOUT_MS", 50));
    let min_confidence = env_f32("CLASSIFIER_MIN_CONFIDENCE", 0.55);
    let dataset: Value = serde_json::from_slice(&fs::read(&eval_path)?)?;
    let (classifier, load_fallback) = match builtin_classifier(&backend) {
        Ok(classifier) => (classifier, false),
        Err(error) => {
            eprintln!("classifier backend `{backend}` unavailable ({error}); using regex fallback");
            (
                Box::new(RegexRequestClassifier) as Box<dyn RequestClassifier>,
                true,
            )
        }
    };
    let mut out = fs::File::create(out_path)?;
    for example in dataset
        .get("examples")
        .and_then(Value::as_array)
        .ok_or("evaluation set is missing examples")?
    {
        let query = example
            .get("query")
            .and_then(Value::as_str)
            .ok_or("example is missing query")?;
        let context = example.get("context").and_then(Value::as_str);
        let input = ClassifyInput { query, context };
        let started = Instant::now();
        let (classification, fallback) =
            classify_with_fallback(classifier.as_ref(), &input, timeout, min_confidence).await;
        write_row(
            &mut out,
            example
                .get("id")
                .and_then(Value::as_str)
                .ok_or("example is missing id")?,
            classification,
            started.elapsed().as_micros(),
            fallback || load_fallback,
        )?;
    }
    Ok(())
}

fn write_row(
    out: &mut fs::File,
    id: &str,
    classification: Classification,
    latency_us: u128,
    fallback: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(
        out,
        "{}",
        serde_json::to_string(&json!({
            "id": id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "latency_us": latency_us,
            "fallback": fallback,
        }))?
    )?;
    Ok(())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f32(key: &str, default: f32) -> f32 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| (0.0..=1.0).contains(value))
        .unwrap_or(default)
}
