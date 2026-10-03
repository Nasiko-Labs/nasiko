//! Request-classifier eval harness (P2 / `[classifier]`).
//!
//! Runs the configured `RequestClassifier` over an eval set and writes one JSONL line per
//! case — the router's predicted request type, complexity and confidence, plus the
//! per-decision latency. It reports *outputs*, not scores: the scorer recomputes every metric
//! from `OUT`, so the labels in the eval file are used only for the human-readable summary
//! this binary prints to **stderr**.
//!
//! ## Contract
//!
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! - No required arguments. `EVAL_SET` names the eval file (JSON with an `examples` array);
//!   `OUT` names the JSONL output (stdout if unset).
//! - The backend is chosen entirely by environment variables, through the same
//!   [`GatewayConfig`] + [`build_request_classifier`] path the router uses:
//!   `CLASSIFIER_BACKEND=regex|local|hosted` (default `regex`), `CLASSIFIER_MODEL_PATH`,
//!   `CLASSIFIER_ENDPOINT`, `CLASSIFIER_MODEL`, `CLASSIFIER_API_KEY`,
//!   `CLASSIFIER_TIMEOUT_MS`, `CLASSIFIER_MIN_CONFIDENCE`.
//! - One output line per case, in input order:
//!   `{"id","request_type","complexity","confidence","latency_us"}`.
//!   `confidence` is in `[0,1]`, `complexity` in `1..=5`, `latency_us` excludes one-time
//!   model load.
//! - Exit code 0 when the eval ran. A missing/unreadable `EVAL_SET` is a startup error.
//!
//! `regex` and `local` are deterministic, so running twice produces byte-identical `OUT`.
//! `hosted` is deterministic only to the extent the endpoint is (`temperature: 0`).

use std::io::Write;
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;

use nasiko_llm_router::{
    ClassifyInput, GatewayConfig, RegexClassifier, RequestClassifier, build_request_classifier,
};

#[derive(Deserialize)]
struct EvalFile {
    examples: Vec<EvalCase>,
}

#[derive(Deserialize)]
struct EvalCase {
    id: String,
    query: String,
    #[serde(default)]
    context: Option<String>,
    /// Present in the public/private eval files; used only for the stderr summary.
    #[serde(default)]
    request_type: Option<String>,
    #[serde(default)]
    complexity: Option<u8>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = std::env::var("EVAL_SET").map_err(
        |_| "EVAL_SET is required (path to the eval JSON, e.g. /tmp/classifier-eval.json)",
    )?;
    let raw = std::fs::read_to_string(&eval_path)
        .map_err(|e| format!("failed to read EVAL_SET {eval_path:?}: {e}"))?;
    let eval: EvalFile = serde_json::from_str(&raw)
        .map_err(|e| format!("EVAL_SET {eval_path:?} is not valid eval JSON: {e}"))?;

    // Build the classifier once, before the loop, through the router's own construction path.
    let cfg = GatewayConfig::from_env();
    let classifier = build_request_classifier(&cfg);
    eprintln!(
        "classifier_eval: backend={} cases={} eval_set={eval_path}",
        classifier.name(),
        eval.examples.len()
    );

    let mut out: Box<dyn Write> = match std::env::var("OUT") {
        Ok(path) => Box::new(std::io::BufWriter::new(std::fs::File::create(&path)?)),
        Err(_) => Box::new(std::io::BufWriter::new(std::io::stdout())),
    };

    let fallback = RegexClassifier; // defensive fallback if a backend returns Err
    let mut labelled_correct = 0usize;
    let mut labelled_total = 0usize;
    let mut complexity_abs_err = 0f64;
    let mut complexity_total = 0usize;
    let mut latencies_us: Vec<u64> = Vec::with_capacity(eval.examples.len());
    let mut errors = 0usize;

    for case in &eval.examples {
        let input = ClassifyInput {
            query: &case.query,
            context: case.context.as_deref(),
        };

        let started = Instant::now();
        let (classification, latency_us) = match classifier.classify(&input).await {
            Ok(c) => {
                let us = started.elapsed().as_micros() as u64;
                (c, us)
            }
            Err(e) => {
                // A resilient backend never reaches here; if one does, fail closed to regex.
                errors += 1;
                eprintln!("classifier_eval: case {} error: {e}", case.id);
                let us = started.elapsed().as_micros() as u64;
                (fallback.classify(&input).await?, us)
            }
        };
        latencies_us.push(latency_us);

        // Per-case metrics (stderr only; the output line reports no score).
        if let Some(expected) = case.request_type.as_deref() {
            if let Some(expected_type) =
                nasiko_llm_router::routing::classifier::RequestType::from_wire(expected)
            {
                labelled_total += 1;
                if classification.request_type == expected_type {
                    labelled_correct += 1;
                }
            }
        }
        if let Some(expected) = case.complexity {
            complexity_total += 1;
            complexity_abs_err += (classification.complexity as f64 - expected as f64).abs();
        }

        writeln!(
            out,
            "{}",
            json!({
                "id": case.id,
                "request_type": classification.request_type.as_str(),
                "complexity": classification.complexity,
                "confidence": classification.confidence,
                "latency_us": latency_us,
            })
        )?;
    }
    out.flush()?;

    // ── human-readable summary (stderr; never part of OUT) ──────────────────────────────
    latencies_us.sort_unstable();
    let pct = |p: f64| -> u64 {
        if latencies_us.is_empty() {
            return 0;
        }
        let idx = ((p / 100.0) * latencies_us.len() as f64).ceil() as usize;
        latencies_us[idx.saturating_sub(1).min(latencies_us.len() - 1)]
    };
    eprintln!(
        "--- classifier_eval summary (backend={}) ---",
        classifier.name()
    );
    if labelled_total > 0 {
        eprintln!(
            "request-type accuracy: {}/{} = {:.3}",
            labelled_correct,
            labelled_total,
            labelled_correct as f64 / labelled_total as f64
        );
    } else {
        eprintln!("request-type accuracy: n/a (no labels in eval set)");
    }
    if complexity_total > 0 {
        eprintln!(
            "complexity MAE: {:.3} (n={complexity_total})",
            complexity_abs_err / complexity_total as f64
        );
    }
    match classifier.fallback_stats() {
        Some((fallbacks, calls)) => eprintln!(
            "fallback: {fallbacks}/{calls} ({:.3})",
            if calls == 0 {
                0.0
            } else {
                fallbacks as f64 / calls as f64
            }
        ),
        None => eprintln!("fallback: 0 (backend has no fallback path)"),
    }
    if errors > 0 {
        eprintln!("backend errors: {errors}");
    }
    eprintln!(
        "latency us: p50={} p95={} max={}",
        pct(50.0),
        pct(95.0),
        latencies_us.last().copied().unwrap_or(0)
    );

    Ok(())
}
