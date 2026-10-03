//! Request classifier eval (regex baseline).
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::classifier::{ClassifyInput, RequestClassifier, RegexClassifier, Classification, ClassifyError};

struct HostedClassifier {
    endpoint: String,
}

#[async_trait::async_trait]
impl RequestClassifier for HostedClassifier {
    fn name(&self) -> &str {
        "hosted"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "query": input.query,
            "context": input.context
        });
        
        let res = client.post(&self.endpoint)
            .json(&payload)
            .timeout(std::time::Duration::from_millis(1500))
            .send()
            .await
            .map_err(|e| if e.is_timeout() { ClassifyError::Timeout } else { ClassifyError::Internal(e.to_string()) })?;
            
        if !res.status().is_success() {
            return Err(ClassifyError::Internal(format!("HTTP {}", res.status())));
        }
        
        // Assume API returns standard JSON matching Classification
        #[derive(serde::Deserialize)]
        struct Resp {
            request_type: String,
            complexity: u8,
            confidence: f32,
        }
        let parsed: Resp = res.json().await.map_err(|e| ClassifyError::Internal(e.to_string()))?;
        let request_type = nasiko_llm_router::routing::classifier::RequestType::from_wire(&parsed.request_type)
            .unwrap_or(nasiko_llm_router::routing::classifier::RequestType::General);
        
        Ok(Classification {
            request_type,
            complexity: parsed.complexity,
            confidence: parsed.confidence,
        })
    }
}

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let backend = std::env::var("CLASSIFIER_BACKEND").unwrap_or_else(|_| "regex".into());
    let endpoint = std::env::var("CLASSIFIER_ENDPOINT").unwrap_or_else(|_| "http://localhost:8080/classify".into());
    let classifier: Box<dyn RequestClassifier> = match backend.as_str() {
        "regex" => Box::new(RegexClassifier),
        "hosted" => Box::new(HostedClassifier { endpoint }),
        "local" => Box::new(RegexClassifier), // Default to regex for local stub
        _ => panic!("Unknown backend: {}", backend),
    };

    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();

        let started = Instant::now();
        let classification = rt.block_on(classifier.classify(&ClassifyInput { query, context })).unwrap();
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
