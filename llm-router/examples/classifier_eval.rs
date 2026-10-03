use nasiko_llm_router::routing::classifier::{
    ClassifyInput, ContextClassifier, RegexClassifier, RequestClassifier,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct EvalItem {
    id: String,
    query: String,
    context: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EvalDataset {
    #[serde(default)]
    examples: Vec<EvalItem>,
}

#[derive(Debug, Serialize)]
struct EvalOutput<'a> {
    id: &'a str,
    request_type: &'static str,
    complexity: u8,
    confidence: f32,
    latency_us: u128,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path =
        env::var("EVAL_SET").unwrap_or_else(|_| "classifier-eval.json".to_string());
    let out_path =
        env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".to_string());
    let backend =
        env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "context".to_string());

    let raw = fs::read_to_string(&eval_set_path)?;

    let items: Vec<EvalItem> = if let Ok(ds) = serde_json::from_str::<EvalDataset>(&raw) {
        if !ds.examples.is_empty() {
            ds.examples
        } else {
            serde_json::from_str::<Vec<EvalItem>>(&raw)?
        }
    } else {
        serde_json::from_str::<Vec<EvalItem>>(&raw)?
    };

    let classifier: Arc<dyn RequestClassifier> = match backend.as_str() {
        "regex" => Arc::new(RegexClassifier),
        _ => Arc::new(ContextClassifier::default()),
    };

    let mut out_file = File::create(&out_path)?;

    for item in &items {
        let input = ClassifyInput {
            query: &item.query,
            context: item.context.as_deref(),
        };

        let start = Instant::now();
        let res = match classifier.classify_request(&input).await {
            Ok(c) => c,
            Err(_) => {
                RegexClassifier
                    .classify_request(&input)
                    .await
                    .expect("fallback must succeed")
            }
        };
        let duration_us = start.elapsed().as_micros();

        let out = EvalOutput {
            id: &item.id,
            request_type: res.request_type.as_str(),
            complexity: res.complexity,
            confidence: res.confidence,
            latency_us: duration_us,
        };

        let line = serde_json::to_string(&out)?;
        writeln!(out_file, "{}", line)?;
    }

    println!("Processed {} test cases into {}", items.len(), out_path);
    Ok(())
}