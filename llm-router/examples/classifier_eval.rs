//! Request classifier eval — runs the pluggable `RequestClassifier` over an eval set and
//! writes one JSONL line of outputs per case.
//!
//! This is the P2 submission harness. It calls the *same* `RequestClassifier` seam the router
//! calls, so a backend that works here works in routing (and vice versa) — including the
//! low-confidence/error fallback, which is applied by `ClassifierRuntime::classify_or_fallback`
//! exactly as `route_model` applies it.
//!
//! # Run
//!
//! ```sh
//! curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval -o /tmp/classifier-eval.json
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! No required arguments. `EVAL_SET` (default `/tmp/classifier-eval.json`) is the eval JSON and
//! `OUT` (default `classifier-out.jsonl`) is where outputs are written.
//!
//! # Backend selection (all optional, env only)
//!
//! | var | values | default | meaning |
//! |---|---|---|---|
//! | `CLASSIFIER_BACKEND` | `local` \| `regex` \| `hosted` | `local` | Which implementation classifies |
//! | `CLASSIFIER_MODEL_PATH` | path | *(embedded)* | Weights override for `local` |
//! | `CLASSIFIER_ENDPOINT` | base URL (…/v1) | *(empty)* | OpenAI-compatible endpoint for `hosted` |
//! | `CLASSIFIER_API_KEY` | bearer token | *(empty)* | Key for `hosted` (never commit one) |
//! | `CLASSIFIER_MODEL` | model id | `gpt-4o-mini` | Model id for `hosted` |
//! | `CLASSIFIER_TIMEOUT_MS` | ms, `0` = none | `1500` | Per-decision deadline (timeout ⇒ regex fallback) |
//! | `CLASSIFIER_LOW_CONFIDENCE` | `[0,1]` | `0.55` | Below this, the safe default is used |
//!
//! The eval defaults to `local` (the embedded model) so the documented command measures the
//! submission, not the baseline. Reproduce the shipped regex baseline with
//! `CLASSIFIER_BACKEND=regex`. The router's own default is still `regex` — nothing here changes
//! production behaviour unless an operator opts in.
//!
//! # Output contract (unchanged from the baseline example)
//!
//! One JSON object per line to `OUT`:
//! ```json
//! {"id":"pub-01","request_type":"code_generation","complexity":1,"confidence":0.92,"latency_us":840}
//! ```
//! `complexity` is 1–5, `confidence` is in `[0,1]`, and `latency_us` is the per-call time
//! *excluding* the one-time model load (the classifier is built once, before the loop).
//! Scores are printed to stderr for the demo only; the graded artifact is `OUT`.
use std::io::Write;
use std::time::{Duration, Instant};

use nasiko_llm_router::routing::{
    BackendKind, ClassifierConfig, ClassifierRuntime, ClassifyInput, build_request_classifier,
};

/// Read an optional env var, falling back to `default` when unset.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Read a `u64` env var, falling back to `default` when unset or unparseable.
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Read an `f32` env var, falling back to `default` when unset or unparseable.
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Build the classifier config from the environment. Mirrors `GatewayConfig::classifier_config`
/// (the library never reads env itself), so the eval and the router are configured the same way.
fn config_from_env() -> ClassifierConfig {
    // Unset ⇒ `local`: the submission's backend. An explicit but unrecognized value warns and
    // still uses `local` rather than silently measuring the baseline.
    let backend = match std::env::var("CLASSIFIER_BACKEND") {
        Ok(raw) => BackendKind::parse(&raw).unwrap_or_else(|| {
            eprintln!("warning: unknown CLASSIFIER_BACKEND={raw:?}; using `local`");
            BackendKind::Local
        }),
        Err(_) => BackendKind::Local,
    };
    ClassifierConfig {
        backend,
        model_path: env_or("CLASSIFIER_MODEL_PATH", ""),
        endpoint: env_or("CLASSIFIER_ENDPOINT", ""),
        api_key: env_or("CLASSIFIER_API_KEY", ""),
        model: env_or("CLASSIFIER_MODEL", "gpt-4o-mini"),
        timeout: Duration::from_millis(env_u64("CLASSIFIER_TIMEOUT_MS", 1500)),
        low_confidence: env_f32("CLASSIFIER_LOW_CONFIDENCE", 0.55),
    }
}

/// Nearest-rank percentile of a sorted slice (`q` in `[0,1]`); `0` when empty.
fn percentile(sorted: &[u128], q: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx]
}

#[tokio::main]
async fn main() {
    let path = env_or("EVAL_SET", "/tmp/classifier-eval.json");
    let out_path = env_or("OUT", "classifier-out.jsonl");
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    // Build the classifier once, before the loop: the one-time model load is excluded from
    // the per-call `latency_us` the scorer cross-checks (and from the performance we report).
    let cfg = config_from_env();
    let load_started = Instant::now();
    let backend = build_request_classifier(&cfg, reqwest::Client::new());
    let runtime = ClassifierRuntime::new(backend, cfg.low_confidence);
    let load_us = load_started.elapsed().as_micros();

    eprintln!(
        "classifier_eval: backend={} low_confidence={} timeout_ms={} cases={} load_us={load_us}",
        runtime.name(),
        cfg.low_confidence,
        cfg.timeout.as_millis(),
        examples.len()
    );

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut latencies: Vec<u128> = Vec::with_capacity(examples.len());
    let mut correct = 0usize;
    let mut labelled = 0usize;
    let mut fallbacks = 0usize;

    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str();
        let expected = example["request_type"].as_str();

        let started = Instant::now();
        let (classification, fell_back) = runtime
            .classify_or_fallback(&ClassifyInput { query, context })
            .await;
        let latency_us = started.elapsed().as_micros() as u64;

        latencies.push(latency_us as u128);
        if fell_back {
            fallbacks += 1;
        }
        if let Some(expected) = expected {
            labelled += 1;
            if classification.request_type.as_str() == expected {
                correct += 1;
            }
        }

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

    // Demo summary (stderr only — never part of the graded JSONL). The scorer recomputes all
    // metrics; these numbers are for the PR/demo narrative.
    latencies.sort_unstable();
    let n = latencies.len();
    let mean = if n == 0 {
        0
    } else {
        latencies.iter().sum::<u128>() / n as u128
    };
    eprintln!(
        "classifier_eval: wrote {n} lines to {out_path} | p50={}us p95={}us mean={mean}us \
         fallbacks={fallbacks} ({:.1}%)",
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.95),
        if n == 0 {
            0.0
        } else {
            100.0 * fallbacks as f64 / n as f64
        }
    );
    if labelled > 0 {
        eprintln!(
            "classifier_eval: request-type accuracy {correct}/{labelled} = {:.3} on the \
             labelled cases in EVAL_SET (self-check only; not the held-out score)",
            correct as f64 / labelled as f64
        );
    }
}