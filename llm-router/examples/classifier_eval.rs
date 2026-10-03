use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::time::Instant;

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::routing::{ClassifyInput, create_classifier};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct EvalExample {
    id: String,
    query: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    request_type: Option<String>,
    #[serde(default)]
    complexity: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct EvalSet {
    examples: Vec<EvalExample>,
}

#[derive(Debug, Serialize)]
struct EvalOutputItem {
    id: String,
    request_type: String,
    complexity: u8,
    confidence: f32,
    latency_us: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").unwrap_or_else(|_| "eval_set.json".to_string());
    let out_path = env::var("OUT").ok();

    eprintln!("Loading evaluation set from: {eval_set_path}");

    let content = match fs::read_to_string(&eval_set_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read EVAL_SET path '{eval_set_path}': {e}");
            std::process::exit(1);
        }
    };

    let eval_set: EvalSet = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to parse JSON from EVAL_SET: {e}");
            std::process::exit(1);
        }
    };

    // Load model / instantiate classifier once before evaluation loop
    let config = GatewayConfig::from_env();
    let classifier = create_classifier(&config);
    eprintln!(
        "Initialized RequestClassifier backend: {}",
        classifier.name()
    );

    let mut output_items = Vec::with_capacity(eval_set.examples.len());

    for example in &eval_set.examples {
        let input = ClassifyInput {
            query: &example.query,
            context: example.context.as_deref(),
        };

        let start = Instant::now();
        let result = classifier.classify(input).await;
        let latency_us = start.elapsed().as_micros() as u64;

        let classification = match result {
            Ok(c) => c,
            Err(err) => {
                eprintln!("Error classifying example {}: {err}", example.id);
                continue;
            }
        };

        let item = EvalOutputItem {
            id: example.id.clone(),
            request_type: classification.request_type.as_str().to_string(),
            complexity: classification.complexity.clamp(1, 5),
            confidence: classification.confidence.clamp(0.0, 1.0),
            latency_us,
        };

        output_items.push(item);
    }

    if let Some(ref path) = out_path {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        for item in &output_items {
            let json_line = serde_json::to_string(item)?;
            writeln!(writer, "{json_line}")?;
        }
        eprintln!("Wrote {} evaluation results to {path}", output_items.len());
    } else {
        for item in &output_items {
            let json_line = serde_json::to_string(item)?;
            println!("{json_line}");
        }
    }

    Ok(())
}
