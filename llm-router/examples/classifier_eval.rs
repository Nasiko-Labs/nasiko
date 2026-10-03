//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! The classifier is built by `build_request_classifier` — the same factory the router uses —
//! so this exercises the production code path, including the timeout and regex fallback.
//! Backend: `CLASSIFIER_BACKEND=regex|local|hosted|cascade`. **When unset, this eval uses
//! `local`** (the router's default stays `regex`); `CLASSIFIER_BACKEND=regex` reproduces the
//! baseline. Other knobs: `CLASSIFIER_MODEL_PATH`, `CLASSIFIER_ENDPOINT`, `CLASSIFIER_MODEL`,
//! `CLASSIFIER_API_KEY`, `CLASSIFIER_TIMEOUT_MS` (see the llm-router README).
//! `EVAL_VERBOSE=1` appends `backend`, `fallback` and `escalated` to each line.
//!
//! Each line: `{"id","request_type","complexity","confidence","latency_us"}`; confidence is
//! rounded to 4 decimals so repeated runs diff cleanly. `latency_us` covers the classify call
//! only — model load and a warm-up call happen before the loop and are reported on stderr.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::local_classifier::{LocalClassifier, embedded_artifact_len};
use nasiko_llm_router::routing::{ClassifyInput, RegexClassifier, RequestClassifier};
use nasiko_llm_router::{GatewayConfig, build_request_classifier};

/// Backend this eval uses when `CLASSIFIER_BACKEND` is unset.
const EVAL_DEFAULT_BACKEND: &str = "local";

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");
    let verbose = std::env::var("EVAL_VERBOSE").is_ok_and(|v| v == "1");

    let mut cfg = GatewayConfig::from_env();
    if std::env::var("CLASSIFIER_BACKEND").map_or(true, |v| v.trim().is_empty()) {
        cfg.classifier.backend = EVAL_DEFAULT_BACKEND.into();
    }

    // Load once (plus one untimed warm-up call: regex/feature `LazyLock`s compile on first use).
    let load_started = Instant::now();
    let classifier = build_request_classifier(&cfg.classifier, &reqwest::Client::new());
    let _ = classifier
        .classify(&ClassifyInput {
            query: "warm up: compile patterns",
            context: Some("warm up"),
        })
        .await;
    let load_ms = load_started.elapsed().as_secs_f64() * 1e3;
    let backend = classifier.name().to_string();

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut latencies = Vec::with_capacity(examples.len());
    let (mut labelled, mut type_correct, mut cx_exact, mut cx_within1) = (0usize, 0, 0, 0);
    let warm = classifier.stats().snapshot();
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();

        let before = classifier.stats().snapshot();
        let started = Instant::now();
        let c = classifier
            .classify(&ClassifyInput { query, context })
            .await
            .unwrap_or_else(|_| RegexClassifier::classify_sync(query));
        let latency_us = started.elapsed().as_micros() as u64;
        let after = classifier.stats().snapshot();
        latencies.push(latency_us);

        let mut line = serde_json::json!({
            "id": id,
            "request_type": c.request_type.as_str(),
            "complexity": c.complexity,
            "confidence": round4(f64::from(c.confidence)),
            "latency_us": latency_us,
        });
        if verbose {
            line["backend"] = backend.clone().into();
            line["fallback"] = (after.fallbacks() > before.fallbacks()).into();
            line["escalated"] = (after.escalations > before.escalations).into();
        }
        writeln!(out, "{line}").expect("write OUT");

        if let (Some(gold_type), Some(gold_cx)) =
            (example["request_type"].as_str(), example["complexity"].as_u64())
        {
            labelled += 1;
            type_correct += usize::from(gold_type == c.request_type.as_str());
            let diff = (i64::from(c.complexity) - gold_cx as i64).abs();
            cx_exact += usize::from(diff == 0);
            cx_within1 += usize::from(diff <= 1);
        }
    }
    out.flush().expect("flush OUT");

    let stats = classifier.stats().snapshot();
    latencies.sort_unstable();
    let n = examples.len();
    eprintln!("classifier_eval: backend={backend} n={n} load_ms={load_ms:.2} -> {out_path}");
    if backend == "local" || backend == "cascade" {
        let (bytes, provenance) = if cfg.classifier.model_path.is_empty() {
            (
                embedded_artifact_len(),
                LocalClassifier::embedded().ok().map(|m| m.provenance().clone()),
            )
        } else {
            (
                std::fs::metadata(&cfg.classifier.model_path).map_or(0, |m| m.len() as usize),
                LocalClassifier::from_path(&cfg.classifier.model_path)
                    .ok()
                    .map(|m| m.provenance().clone()),
            )
        };
        let p = provenance.unwrap_or_default();
        eprintln!(
            "  artifact: {bytes} bytes, dataset_sha256={}, trained_at={}",
            p["dataset_sha256"].as_str().unwrap_or("n/a"),
            p["trained_at_utc"].as_str().unwrap_or("n/a")
        );
    }
    eprintln!(
        "  latency_us: p50={} p95={} p99={} max={}",
        percentile(&latencies, 50.0),
        percentile(&latencies, 95.0),
        percentile(&latencies, 99.0),
        latencies.last().copied().unwrap_or(0)
    );
    eprintln!(
        "  fallbacks (excl. warm-up): error={} timeout={} | load fallback at build: {} | escalations={} (failed {})",
        stats.fallback_error - warm.fallback_error,
        stats.fallback_timeout - warm.fallback_timeout,
        stats.fallback_load,
        stats.escalations - warm.escalations,
        stats.escalation_failures - warm.escalation_failures
    );
    if labelled > 0 {
        eprintln!(
            "  informational (the scorer computes its own): type accuracy {}/{labelled}, complexity exact {}/{labelled}, within ±1 {}/{labelled}",
            type_correct, cx_exact, cx_within1
        );
    }
}
