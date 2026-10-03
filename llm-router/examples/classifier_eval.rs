//! Request classifier evaluation.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! The classifier backend is selected through environment variables.
//!
//! Default backend:
//!   regex
//!
//! Hosted backend:
//!   CLASSIFIER_BACKEND=groq
//!   CLASSIFIER_MODEL=openai/gpt-oss-20b
//!   CLASSIFIER_ENDPOINT=https://api.groq.com/openai/v1/chat/completions
//!   GROQ_API_KEY=<secret>
//!
//! The model is initialized once before evaluation. Per-example latency
//! measures classification only and excludes backend initialization.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nasiko_llm_router::routing::{
    ClassifyInput, HostedClassifier, RegexClassifier, RequestClassifier,
};

/// Build the configured classifier backend once.
///
/// Supported backends:
/// - `regex`
/// - `groq`
/// - `hosted`
/// - `openai-compatible`
fn build_classifier() -> Arc<dyn RequestClassifier> {
    let backend = std::env::var("CLASSIFIER_BACKEND")
        .unwrap_or_else(|_| "regex".to_string())
        .to_ascii_lowercase();

    match backend.as_str() {
        "regex" => {
            eprintln!("classifier backend: regex");
            Arc::new(RegexClassifier)
        }

        "groq" | "hosted" | "openai-compatible" => {
            let endpoint = std::env::var("CLASSIFIER_ENDPOINT")
                .unwrap_or_else(|_| "https://api.groq.com/openai/v1/chat/completions".to_string());

            let model = std::env::var("CLASSIFIER_MODEL")
                .unwrap_or_else(|_| "openai/gpt-oss-20b".to_string());

            let api_key = std::env::var("GROQ_API_KEY").unwrap_or_default();

            let timeout_secs = std::env::var("CLASSIFIER_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(5);

            if api_key.is_empty() {
                eprintln!(
                    "classifier backend requested but GROQ_API_KEY is \
                     missing; falling back to regex"
                );

                return Arc::new(RegexClassifier);
            }

            eprintln!(
                "classifier backend: hosted model={} endpoint={}",
                model, endpoint
            );

            match HostedClassifier::new(endpoint, model, api_key, Duration::from_secs(timeout_secs))
            {
                Ok(classifier) => Arc::new(classifier),

                Err(error) => {
                    eprintln!(
                        "failed to initialize hosted classifier: {}; \
                         falling back to regex",
                        error
                    );

                    Arc::new(RegexClassifier)
                }
            }
        }

        other => {
            eprintln!(
                "unknown CLASSIFIER_BACKEND='{}'; \
                 falling back to regex",
                other
            );

            Arc::new(RegexClassifier)
        }
    }
}

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET")
        .or_else(|_| {
            if std::path::Path::new("classifier-eval.json").exists() {
                Ok("classifier-eval.json".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        })
        .expect("set EVAL_SET to the eval JSON path");

    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());

    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    // Initialize the backend once. Initialization is deliberately outside
    // the per-example latency measurement.
    let classifier = build_classifier();

    let evaluation_started = Instant::now();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));

    let mut latencies_us = Vec::with_capacity(examples.len());

    let mut request_type_correct = 0usize;
    let mut complexity_correct = 0usize;
    let mut joint_correct = 0usize;

    let mut confidence_sum = 0.0f64;
    let mut calibration = Vec::with_capacity(examples.len());
    let mut fallback_count = 0usize;

    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str().map(str::to_owned);

        let input = ClassifyInput {
            query: query.to_string(),
            context,
        };

        let started = Instant::now();

        let classification = classifier.classify(input).await;

        let latency_us = started.elapsed().as_micros() as u64;
        latencies_us.push(latency_us);

        confidence_sum += f64::from(classification.confidence);

        let expected_request_type = example["request_type"].as_str();
        let expected_complexity = example["complexity"].as_u64();

        let request_type_ok = expected_request_type
            .map(|expected| expected == classification.request_type.as_str())
            .unwrap_or(false);

        let complexity_ok = expected_complexity
            .map(|expected| expected == u64::from(classification.complexity))
            .unwrap_or(false);

        if request_type_ok {
            request_type_correct += 1;
        }

        if complexity_ok {
            complexity_correct += 1;
        }

        if request_type_ok && complexity_ok {
            joint_correct += 1;
        }
        if classification.fallback {
            fallback_count += 1;
        }

        calibration.push((f64::from(classification.confidence), request_type_ok));

        // Keep the required JSONL output schema unchanged.
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

    let count = examples.len();

    fn percentile(sorted: &[u64], percentile: f64) -> u64 {
        if sorted.is_empty() {
            return 0;
        }

        let rank = (percentile * sorted.len() as f64).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);

        sorted[index]
    }

    let mut sorted_latencies = latencies_us.clone();
    sorted_latencies.sort_unstable();

    let p50_us = percentile(&sorted_latencies, 0.50);
    let p95_us = percentile(&sorted_latencies, 0.95);

    let min_us = sorted_latencies.first().copied().unwrap_or(0);
    let max_us = sorted_latencies.last().copied().unwrap_or(0);

    let mean_confidence = if count == 0 {
        0.0
    } else {
        confidence_sum / count as f64
    };

    let request_type_accuracy = if count == 0 {
        0.0
    } else {
        request_type_correct as f64 / count as f64
    };

    let complexity_accuracy = if count == 0 {
        0.0
    } else {
        complexity_correct as f64 / count as f64
    };

    let joint_accuracy = if count == 0 {
        0.0
    } else {
        joint_correct as f64 / count as f64
    };

    let total_ms = evaluation_started.elapsed().as_secs_f64() * 1000.0;
    let ece = if count == 0 {
        0.0
    } else {
        let bins = 10usize;
        let mut ece = 0.0f64;

        for bin in 0..bins {
            let lower = bin as f64 / bins as f64;
            let upper = (bin + 1) as f64 / bins as f64;

            let items: Vec<_> = calibration
                .iter()
                .filter(|(confidence, _)| {
                    if bin == bins - 1 {
                        *confidence >= lower && *confidence <= upper
                    } else {
                        *confidence >= lower && *confidence < upper
                    }
                })
                .collect();

            if items.is_empty() {
                continue;
            }

            let accuracy =
                items.iter().filter(|(_, correct)| *correct).count() as f64 / items.len() as f64;

            let mean_confidence =
                items.iter().map(|(confidence, _)| *confidence).sum::<f64>() / items.len() as f64;

            ece += (items.len() as f64 / count as f64) * (accuracy - mean_confidence).abs();
        }

        ece
    };

    let fallback_rate = if count == 0 {
        0.0
    } else {
        fallback_count as f64 / count as f64
    };

    eprintln!();
    eprintln!("=== classifier evaluation summary ===");
    eprintln!("cases:                 {count}");
    eprintln!(
        "request-type accuracy: {:.1}% ({}/{})",
        request_type_accuracy * 100.0,
        request_type_correct,
        count
    );
    eprintln!(
        "complexity accuracy:   {:.1}% ({}/{})",
        complexity_accuracy * 100.0,
        complexity_correct,
        count
    );
    eprintln!(
        "joint accuracy:        {:.1}% ({}/{})",
        joint_accuracy * 100.0,
        joint_correct,
        count
    );
    eprintln!("mean confidence:       {:.3}", mean_confidence);
    eprintln!("latency p50:           {:.3} ms", p50_us as f64 / 1000.0);
    eprintln!("latency p95:           {:.3} ms", p95_us as f64 / 1000.0);
    eprintln!("latency min:           {:.3} ms", min_us as f64 / 1000.0);
    eprintln!("latency max:           {:.3} ms", max_us as f64 / 1000.0);
    eprintln!("evaluation wall time:  {:.3} ms", total_ms);
    eprintln!("model load time:       excluded from latency_us");
    eprintln!(
        "fallback rate:         {:.1}% ({}/{})",
        fallback_rate * 100.0,
        fallback_count,
        count
    );
    eprintln!("ECE:                   {:.4}", ece);
    eprintln!("====================================");
}
