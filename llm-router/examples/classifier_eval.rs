use std::env;
use std::fs;
use std::time::Instant;

use nasiko_llm_router::routing::{
    build_classifier,
    classify_with_fallback,
    Classification,
    ClassifierSettings,
    ClassifierStats,
    ClassifyInput,
    RegexRequestClassifier,
    RequestClassifier,
    RequestType,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct EvalCase {
    query: String,
    request_type: String,
    complexity: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(default)]
    cases: Vec<EvalCase>,
}

#[derive(Debug, Serialize)]
struct EvalOutput {
    backend: String,
    query: String,
    expected_request_type: String,
    predicted_request_type: String,
    expected_complexity: Option<u8>,
    predicted_complexity: u8,
    confidence: f32,
    fallback: bool,
    latency_us: u128,
}

#[derive(Debug, Default)]
struct Metrics {
    total: usize,
    correct: usize,
    complexity_total: usize,
    complexity_correct: usize,
    confidences: Vec<f64>,
    correctness: Vec<bool>,
    latencies_us: Vec<u128>,
    fallbacks: usize,
    outputs: Vec<EvalOutput>,
}

impl Metrics {
    fn accuracy(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }

        self.correct as f64 / self.total as f64
    }

    fn complexity_accuracy(&self) -> f64 {
        if self.complexity_total == 0 {
            return 0.0;
        }

        self.complexity_correct as f64 / self.complexity_total as f64
    }

    fn ece(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }

        let mut bins: Vec<Vec<(f64, bool)>> =
            (0..10).map(|_| Vec::new()).collect();

        for (confidence, correct) in
            self.confidences.iter().zip(self.correctness.iter())
        {
            let index = ((*confidence * 10.0).floor() as usize).min(9);
            bins[index].push((*confidence, *correct));
        }

        let mut ece = 0.0;

        for bin in bins {
            if bin.is_empty() {
                continue;
            }

            let count = bin.len() as f64;

            let avg_confidence =
                bin.iter().map(|(confidence, _)| confidence).sum::<f64>()
                    / count;

            let accuracy =
                bin.iter().filter(|(_, correct)| *correct).count() as f64
                    / count;

            ece += (count / self.total as f64)
                * (avg_confidence - accuracy).abs();
        }

        ece
    }

    fn p50_us(&self) -> u128 {
        percentile(&self.latencies_us, 0.50)
    }

    fn p95_us(&self) -> u128 {
        percentile(&self.latencies_us, 0.95)
    }
}

fn percentile(values: &[u128], percentile: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }

    let mut sorted = values.to_vec();
    sorted.sort_unstable();

    let index =
        ((sorted.len() - 1) as f64 * percentile).round() as usize;

    sorted[index.min(sorted.len() - 1)]
}

fn parse_eval_file(path: &str) -> Result<Vec<EvalCase>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("failed to read {path}: {e}"))?;

    if let Ok(file) = serde_json::from_str::<EvalFile>(&content) {
        if !file.cases.is_empty() {
            return Ok(file.cases);
        }
    }

    serde_json::from_str::<Vec<EvalCase>>(&content)
        .map_err(|e| format!("invalid evaluation JSON: {e}"))
}

fn request_type_from_case(value: &str) -> Option<RequestType> {
    RequestType::from_wire(value)
}

async fn evaluate_backend(
    name: &str,
    classifier: &dyn RequestClassifier,
    cases: &[EvalCase],
) -> Metrics {
    let mut metrics = Metrics::default();

    for case in cases {
        let expected =
            match request_type_from_case(&case.request_type) {
                Some(value) => value,
                None => {
                    eprintln!(
                        "Skipping unknown request_type: {}",
                        case.request_type
                    );
                    continue;
                }
            };

        let stats = ClassifierStats::new();

        let start = Instant::now();

        let (classification, fell_back) =
            classify_with_fallback(
                classifier,
                &ClassifyInput {
                    query: &case.query,
                    context: None,
                },
                Some(&stats),
            )
            .await;

        let elapsed = start.elapsed().as_micros();

        let correct =
            classification.request_type == expected;

        metrics.total += 1;
        metrics.latencies_us.push(elapsed);

        if correct {
            metrics.correct += 1;
        }

        let confidence =
            classification.confidence.clamp(0.0, 1.0) as f64;

        metrics.confidences.push(confidence);
        metrics.correctness.push(correct);

        if let Some(expected_complexity) = case.complexity {
            metrics.complexity_total += 1;

            if classification.complexity == expected_complexity {
                metrics.complexity_correct += 1;
            }
        }

        if fell_back {
            metrics.fallbacks += 1;
        }

        metrics.outputs.push(EvalOutput {
            backend: name.to_string(),
            query: case.query.clone(),
            expected_request_type: expected.as_str().to_string(),
            predicted_request_type:
                classification.request_type.as_str().to_string(),
            expected_complexity: case.complexity,
            predicted_complexity: classification.complexity,
            confidence: classification.confidence,
            fallback: fell_back,
            latency_us: elapsed,
        });
    }

    println!();
    println!("========================================");
    println!("CLASSIFIER EVALUATION: {name}");
    println!("========================================");

    println!(
        "Total cases:              {}",
        metrics.total
    );

    println!(
        "Request-type accuracy:    {:.2}%",
        metrics.accuracy() * 100.0
    );

    println!(
        "Complexity accuracy:      {:.2}%",
        metrics.complexity_accuracy() * 100.0
    );

    println!(
        "ECE:                      {:.4}",
        metrics.ece()
    );

    println!(
        "p50 latency:              {} µs",
        metrics.p50_us()
    );

    println!(
        "p95 latency:              {} µs",
        metrics.p95_us()
    );

    println!(
        "Fallbacks:                {}",
        metrics.fallbacks
    );

    metrics
}

fn write_jsonl(
    path: &str,
    outputs: &[EvalOutput],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut content = String::new();

    for output in outputs {
        content.push_str(
            &serde_json::to_string(output)?
        );
        content.push('\n');
    }

    fs::write(path, content)?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET")
        .unwrap_or_else(|_| {
            "examples/data/classifier-eval-dev-json"
                .to_string()
        });

    let out_path = env::var("OUT")
        .unwrap_or_else(|_| {
            "/tmp/classifier-out.jsonl"
                .to_string()
        });

    let cases = parse_eval_file(&eval_path)?;

    if cases.is_empty() {
        return Err(
            "evaluation set is empty".into()
        );
    }

    println!(
        "Loaded {} evaluation cases from {}",
        cases.len(),
        eval_path
    );

    // ------------------------------------------------------------
    // 1. Regex baseline
    // ------------------------------------------------------------

    let regex = RegexRequestClassifier;

    let regex_metrics =
        evaluate_backend(
            "REGEX BASELINE",
            &regex,
            &cases,
        )
        .await;

    // ------------------------------------------------------------
    // 2. Configured backend
    // ------------------------------------------------------------

    let settings = ClassifierSettings {
        backend: env::var(
            "CLASSIFIER_BACKEND"
        )
        .unwrap_or_else(|_| "regex".to_string()),

        model: env::var(
            "CLASSIFIER_MODEL"
        )
        .unwrap_or_default(),

        endpoint: env::var(
            "CLASSIFIER_ENDPOINT"
        )
        .unwrap_or_default(),

        api_key: env::var(
            "CLASSIFIER_API_KEY"
        )
        .ok(),

        timeout_ms: env::var(
            "CLASSIFIER_TIMEOUT_MS"
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000),

        min_confidence: env::var(
            "CLASSIFIER_MIN_CONFIDENCE"
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5),
    };

    let configured =
        build_classifier(&settings);

    let configured_metrics =
        evaluate_backend(
            configured.name(),
            configured.as_ref(),
            &cases,
        )
        .await;

    // ------------------------------------------------------------
    // Summary
    // ------------------------------------------------------------

    println!();
    println!("========================================");
    println!("COMPARISON");
    println!("========================================");

    println!(
        "Regex accuracy:       {:.2}%",
        regex_metrics.accuracy() * 100.0
    );

    println!(
        "{} accuracy:       {:.2}%",
        configured.name(),
        configured_metrics.accuracy() * 100.0
    );

    println!(
        "Regex ECE:            {:.4}",
        regex_metrics.ece()
    );

    println!(
        "{} ECE:            {:.4}",
        configured.name(),
        configured_metrics.ece()
    );

    println!(
        "Regex p50:            {} µs",
        regex_metrics.p50_us()
    );

    println!(
        "{} p50:            {} µs",
        configured.name(),
        configured_metrics.p50_us()
    );

    println!(
        "Regex p95:            {} µs",
        regex_metrics.p95_us()
    );

    println!(
        "{} p95:            {} µs",
        configured.name(),
        configured_metrics.p95_us()
    );

    println!(
        "Regex fallbacks:      {}",
        regex_metrics.fallbacks
    );

    println!(
        "{} fallbacks:      {}",
        configured.name(),
        configured_metrics.fallbacks
    );

    let mut all_outputs = regex_metrics.outputs;
    all_outputs.extend(configured_metrics.outputs);

    write_jsonl(&out_path, &all_outputs)?;

    println!();
    println!(
        "Evaluation output: {}",
        out_path
    );

    println!();
    println!(
        "Evaluation completed successfully."
    );

    Ok(())
}
