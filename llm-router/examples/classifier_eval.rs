//! Evaluate the configured request classifier against a JSON array or JSONL file.
//!
//! The evaluator uses the same `RequestClassifier` trait as the router. Model loading/client
//! construction happens once; per-case latency measures only classification.

use std::{env, fs, sync::Arc, time::Instant};

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::{
    ClassifyInput, FallbackClassifier, ModelClassifier, RegexClassifier, RequestClassifier,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    query: String,
    #[serde(default)]
    context: Option<String>,
}

fn load_cases(path: &str) -> Result<Vec<EvalCase>, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    if let Ok(cases) = serde_json::from_str::<Vec<EvalCase>>(&raw) {
        return Ok(cases);
    }
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

fn classifier(config: &GatewayConfig) -> Arc<dyn RequestClassifier> {
    if config.classifier_backend.eq_ignore_ascii_case("model")
        && !config.classifier_endpoint.is_empty()
    {
        let model = if config.classifier_model_path.is_empty() {
            "classifier".to_owned()
        } else {
            config.classifier_model_path.clone()
        };
        let primary = ModelClassifier {
            http: reqwest::Client::new(),
            endpoint: config.classifier_endpoint.clone(),
            model,
            timeout: std::time::Duration::from_millis(config.classifier_timeout_ms),
        };
        Arc::new(FallbackClassifier {
            primary: Arc::new(primary),
            fallback: RegexClassifier,
        })
    } else {
        Arc::new(RegexClassifier)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = env::var("EVAL_SET").unwrap_or_else(|_| "/tmp/classifier-eval.json".into());
    let output = env::var("OUT").unwrap_or_else(|_| "/tmp/classifier-out.jsonl".into());
    let config = GatewayConfig::from_env();
    let classifier = classifier(&config);
    let cases = load_cases(&input)?;
    let mut records = Vec::with_capacity(cases.len());

    for case in cases {
        let started = Instant::now();
        let result = classifier
            .classify(&ClassifyInput {
                query: &case.query,
                context: case.context.as_deref(),
            })
            .await?;
        let latency_us = started.elapsed().as_micros() as u64;
        records.push(serde_json::json!({
            "id": case.id,
            "request_type": result.request_type,
            "complexity": result.complexity,
            "confidence": result.confidence,
            "latency_us": latency_us,
        }));
    }

    let body = records
        .into_iter()
        .map(|record: Value| serde_json::to_string(&record))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(output, format!("{body}\n"))?;
    Ok(())
}

// Keep the evaluator's output contract visible to rustdoc and downstream tooling.
#[allow(dead_code)]
fn _output_contract() -> (&'static str, &'static str) {
    ("request_type", "complexity/confidence/latency_us")
}
