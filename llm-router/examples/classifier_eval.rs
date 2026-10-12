//! The same backend, timeout and fallback path as production routing, without DB/Redis.
//! See ../docs/classifier.md for configuration, data provenance and measurement limits.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use nasiko_llm_router::config::{ClassifierConfig, build_classifier};
use nasiko_llm_router::routing::classifier::{ClassifyInput, classify_request_type};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
struct EvaluationSet {
    examples: Vec<Example>,
}

#[derive(Deserialize)]
struct Example {
    id: String,
    query: String,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    request_type: Option<String>,
    #[serde(default)]
    complexity: Option<u8>,
}

#[derive(Serialize)]
struct EvaluationOutput<'a> {
    id: &'a str,
    request_type: &'a str,
    complexity: u8,
    confidence: f32,
    latency_us: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dataset = std::env::var("EVAL_SET").unwrap_or_else(|_| {
        format!(
            "{}/tests/data/classifier_validation.json",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let output = std::env::var("OUT").unwrap_or_else(|_| "/tmp/classifier-out.jsonl".into());
    let set: EvaluationSet = serde_json::from_reader(File::open(dataset)?)?;
    let load_start = Instant::now();
    let config = ClassifierConfig::from_env();
    let classifier = build_classifier(&config);
    let load_us = load_start.elapsed().as_micros();
    let mut writer = BufWriter::new(File::create(output)?);
    // Optional local diagnostics, separate from the scorer's OUT contract. No prompt text.
    let mut diagnostics = std::env::var("CLASSIFIER_DIAGNOSTICS")
        .ok()
        .map(File::create)
        .transpose()?
        .map(BufWriter::new);
    let mut latencies = Vec::new();
    let mut models = BTreeSet::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut labelled = 0;
    let mut correct = 0;
    let mut regex_correct = 0;
    let mut complexity_cases = 0;
    let mut complexity_correct = 0;
    let mut complexity_error = 0;
    let mut calibration = [(0usize, 0usize, 0.0f64); 10];
    for case in &set.examples {
        let input = ClassifyInput {
            query: &case.query,
            context: case.context.as_deref(),
        };
        let start = Instant::now();
        let decision = classifier.classify(&input).await;
        let latency_us = start.elapsed().as_micros() as u64;
        latencies.push(latency_us);
        let classification = &decision.classification;
        if let Some(writer) = &mut diagnostics {
            let candidate = decision.candidate.as_ref().unwrap_or(classification);
            serde_json::to_writer(
                &mut *writer,
                &json!({
                    "id":case.id, "status":format!("{:?}", decision.status),
                    "candidate_type":candidate.request_type.as_str(),
                    "candidate_complexity":candidate.complexity,
                    "candidate_confidence":candidate.confidence,
                    "complexity_confidence":candidate.complexity_confidence,
                    "model":candidate.usage.as_ref().map(|usage| &usage.model),
                }),
            )?;
            writeln!(writer)?;
        }
        // OUT contains case outputs, never aggregate metrics or ground-truth answers.
        serde_json::to_writer(
            &mut writer,
            &EvaluationOutput {
                id: &case.id,
                request_type: classification.request_type.as_str(),
                complexity: classification.complexity,
                confidence: classification.confidence,
                latency_us,
            },
        )?;
        writeln!(writer)?;
        if let Some(usage) = &classification.usage {
            input_tokens += usage.input_tokens;
            output_tokens += usage.output_tokens;
            models.insert(usage.model.clone());
        }
        if let Some(expected) = &case.request_type {
            labelled += 1;
            let matched = classification.request_type.as_str() == expected;
            correct += usize::from(matched);
            regex_correct += usize::from(classify_request_type(&case.query).as_str() == expected);
            let bin = (classification.confidence * 10.0).floor().min(9.0) as usize;
            calibration[bin].0 += 1;
            calibration[bin].1 += usize::from(matched);
            calibration[bin].2 += classification.confidence as f64;
        }
        if let Some(expected) = case.complexity {
            complexity_cases += 1;
            complexity_correct += usize::from(classification.complexity == expected);
            complexity_error += classification.complexity.abs_diff(expected) as usize;
        }
    }
    writer.flush()?;
    if let Some(writer) = &mut diagnostics {
        writer.flush()?;
    }
    latencies.sort_unstable();
    let (decisions, fallbacks) = classifier.counts();
    let ece: f64 = calibration
        .iter()
        .filter(|(count, _, _)| *count > 0)
        .map(|(_, matches, confidence)| {
            (confidence - *matches as f64).abs() / labelled.max(1) as f64
        })
        .sum();
    // API price is an operator-supplied measurement assumption, not a baked-in claim.
    let price = std::env::var("CLASSIFIER_INPUT_USD_PER_MILLION")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|price| price.is_finite() && *price >= 0.0);
    eprintln!(
        "{}",
        json!({
            "backend": classifier.name(), "requested_model": config.model, "resolved_models": models,
            "cases": decisions, "fallbacks": fallbacks, "load_us": load_us,
            "p50_latency_us": percentile(&latencies, 50), "p95_latency_us": percentile(&latencies, 95),
            "input_tokens": input_tokens, "output_tokens": output_tokens,
            "input_usd_per_million": price,
            "estimated_usd_per_decision": price.map(|price| input_tokens as f64 * price / 1_000_000.0 / decisions.max(1) as f64),
            "labelled_cases": labelled, "request_type_correct": correct, "regex_correct": regex_correct,
            "ece_including_fallbacks": ece,
            "complexity_cases": complexity_cases, "complexity_correct": complexity_correct,
            "complexity_mean_absolute_error": complexity_error as f64 / complexity_cases.max(1) as f64,
            "timing_note": "Real wall-clock latency varies; compare predictions separately for reproducibility. Fallback confidence 0 is uncalibrated. Usage may be missing for failed requests."
        })
    );
    Ok(())
}

fn percentile(sorted: &[u64], percent: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)])
}
