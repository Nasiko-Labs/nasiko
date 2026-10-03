use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};

use nasiko_llm_router::{
    ClassifyInput, RequestClassifier, RequestType,
};

#[derive(Debug, Deserialize)]
struct EvalCase {
    #[serde(default)]
    id: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    request_type: Option<String>,
}

fn extract_cases(value: &Value) -> Vec<EvalCase> {
    if let Some(items) = value.get("examples").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(|item| serde_json::from_value::<EvalCase>(item.clone()).ok())
            .collect();
    }
    if let Some(items) = value.get("cases").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(|item| serde_json::from_value::<EvalCase>(item.clone()).ok())
            .collect();
    }
    if value.is_array() {
        return value
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| serde_json::from_value::<EvalCase>(item.clone()).ok())
            .collect();
    }
    Vec::new()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let eval_set = env::var("EVAL_SET").unwrap_or_else(|_| {
        eprintln!("EVAL_SET must be set to a JSON dataset path");
        std::process::exit(1);
    });
    let out_path = env::var("OUT").unwrap_or_else(|_| {
        eprintln!("OUT must be set to a JSONL output path");
        std::process::exit(1);
    });

    let raw = std::fs::read_to_string(&eval_set)?;
    let parsed: Value = serde_json::from_str(&raw)?;
    let cases = extract_cases(&parsed);

    let backend = env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".to_string());
    let endpoint = env::var("CLASSIFIER_ENDPOINT").unwrap_or_default();
    let timeout_ms = env::var("CLASSIFIER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2000);
    let classifier = nasiko_llm_router::routing::build_request_classifier(
        &backend,
        &endpoint,
        timeout_ms,
    );

    let file = File::create(&out_path)?;
    let mut writer = BufWriter::new(file);

    for mut case in cases {
        let start = Instant::now();
        let classification = match classifier
            .classify(&ClassifyInput {
                query: &case.query,
                context: case.context.as_deref(),
            })
            .await
        {
            Ok(classification) => classification,
            Err(err) => {
                let fallback = nasiko_llm_router::routing::RegexRequestClassifier
                    .classify(&ClassifyInput {
                        query: &case.query,
                        context: case.context.as_deref(),
                    })
                    .await
                    .unwrap_or_else(|_| nasiko_llm_router::routing::Classification {
                        request_type: RequestType::General,
                        complexity: 1,
                        confidence: 0.0,
                    });
                eprintln!("classifier backend failed for {}: {err}; using regex fallback", case.id);
                fallback
            }
        };

        let latency_us = start.elapsed().as_micros() as u64;
        if case.id.trim().is_empty() {
            case.id = format!("case-{}", latency_us);
        }

        let line = json!({
            "id": case.id,
            "request_type": classification.request_type.as_str(),
            "complexity": classification.complexity,
            "confidence": classification.confidence,
            "latency_us": latency_us,
        });
        writeln!(writer, "{}", line)?;
    }

    writer.flush()?;
    Ok(())
}
