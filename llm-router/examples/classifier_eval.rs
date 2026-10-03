//! P2 `classifier_eval` — request-classifier eval harness (required scope).
//!
//! Runs the same `Arc<dyn RequestClassifier>` code path the router uses at
//! Level 3 and reports one JSONL decision per case:
//!
//! ```sh
//! EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   CLASSIFIER_BACKEND=heuristic \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//! ```
//!
//! Contract (see `hackathon-problems.md` P2 "How we run it"):
//! - Reads `EVAL_SET` (`examples` array; a top-level array or `cases` array is
//!   also accepted). When `EVAL_SET` is unset, an embedded 10-case smoke set
//!   runs instead, so the example works with no arguments.
//! - Writes one JSONL line per case to `OUT` (or stdout when `OUT` is unset):
//!   `{"id","request_type","complexity","confidence","latency_us"}` plus
//!   `tier` / `backend` / `fallback` extras for the routing demo. `confidence`
//!   is in `[0,1]`, `complexity` in `1..=5`.
//! - Backend from `CLASSIFIER_BACKEND` (`regex` default — behaviour unchanged;
//!   `heuristic`/`local`, `minilm`/`semantic`, `hosted`), timeout from
//!   `CLASSIFIER_TIMEOUT_MS`, endpoint/model from `CLASSIFIER_ENDPOINT` /
//!   `CLASSIFIER_MODEL`. The model loads once before the loop; `latency_us`
//!   covers one `classify` call only (load excluded).
//! - Exit code 0 = eval ran. Metrics (accuracy vs. the set's labels, p50/p95
//!   latency, fallback rate) go to stderr; `OUT`/stdout carries decisions only.
//! - Deterministic: identical inputs give identical `request_type`/
//!   `complexity`/`confidence` across runs (`latency_us` is a measured
//!   cross-check and naturally varies).
//!
//! Labelling rubric for the embedded set (and any custom train/val data):
//! complexity 1 = typo/greeting/fact; 2 = small edit or short explanation;
//! 3 = moderate implementation or analysis; 4 = multi-constraint build;
//! 5 = architecture-scale design. Near-duplicates are split apart, never
//! across train/val.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Instant;

use nasiko_llm_router::GatewayConfig;
use nasiko_llm_router::routing::{
    ClassifyInput, RequestClassifier, RequestType, build_classifier, classify_with_fallback,
};

/// One eval case after tolerant parsing.
struct Case {
    id: String,
    query: String,
    context: Option<String>,
    expected_type: Option<RequestType>,
    expected_complexity: Option<u8>,
}

fn get_str(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn parse_cases(raw: &serde_json::Value) -> Vec<Case> {
    let arr = if let Some(a) = raw.get("examples").and_then(|v| v.as_array()) {
        a.clone()
    } else if let Some(a) = raw.get("cases").and_then(|v| v.as_array()) {
        a.clone()
    } else if let Some(a) = raw.as_array() {
        a.clone()
    } else {
        vec![]
    };
    arr.iter()
        .enumerate()
        .map(|(i, c)| Case {
            id: get_str(c, &["id", "case_id"]).unwrap_or_else(|| format!("case-{i}")),
            query: get_str(c, &["query", "prompt", "text", "query_text"]).unwrap_or_default(),
            context: get_str(c, &["context", "context_text"]),
            expected_type: c
                .get("request_type")
                .and_then(|v| v.as_str())
                .and_then(RequestType::from_wire),
            expected_complexity: c
                .get("complexity")
                .and_then(|v| v.as_u64())
                .map(|n| (n as u8).clamp(1, 5)),
        })
        .collect()
}

/// Embedded smoke set (10 cases): seven request types + ambiguous,
/// paraphrased, and noisy/padded variants. Mirrors the public sample schema.
fn smoke_set() -> serde_json::Value {
    serde_json::json!([
        {"id": "smoke-01", "query": "Fix typo in this Python comment: `# retrun the cached value`.", "context": "No other files or changes needed.", "request_type": "code_generation", "complexity": 1},
        {"id": "smoke-02", "query": "explain what this function does", "context": "fn retry(op: impl Fn() -> bool, n: u32) { for _ in 0..n { if op() { return; } } }", "request_type": "code_understanding", "complexity": 2},
        {"id": "smoke-03", "query": "how should I design this API?", "context": "Versioned REST API for billing events with idempotency keys.", "request_type": "technical_design", "complexity": 4},
        {"id": "smoke-04", "query": "calculate the probability that it rains tomorrow", "context": "Prior 0.3, sensor true-positive 0.8, false-positive 0.1.", "request_type": "analytical_reasoning", "complexity": 3},
        {"id": "smoke-05", "query": "draft an email to my team about the outage", "context": "Outage 14:02-14:35 UTC, cause: expired cert, no data loss.", "request_type": "writing", "complexity": 2},
        {"id": "smoke-06", "query": "what is the capital of France?", "context": null, "request_type": "factual_lookup", "complexity": 1},
        {"id": "smoke-07", "query": "hello there", "context": null, "request_type": "general", "complexity": 1},
        {"id": "smoke-08", "query": "What is the capital of France? Also write me a Python sort function and draft an email about the outage.", "context": null, "request_type": "factual_lookup", "complexity": 2},
        {"id": "smoke-09", "query": "could you kindly tell me, if you don't mind, what the capital of France might possibly be, thanks so much in advance for your help here", "context": null, "request_type": "factual_lookup", "complexity": 1},
        {"id": "smoke-10", "query": "implement JWT auth with rate limits, an audit log, and thread-safe session storage without breaking backwards compatible clients", "context": "Axum service, Postgres sessions, p99 budget 40ms.", "request_type": "code_generation", "complexity": 4}
    ])
}

fn percentile(mut xs: Vec<u64>, p: f64) -> u64 {
    if xs.is_empty() {
        return 0;
    }
    xs.sort_unstable();
    xs[((p * xs.len() as f64).ceil() as usize).saturating_sub(1).min(xs.len() - 1)]
}

#[tokio::main]
async fn main() {
    let cfg = GatewayConfig::from_env();
    let backend = std::env::var("CLASSIFIER_BACKEND")
        .unwrap_or_else(|_| cfg.classifier_backend.clone());
    let timeout_ms: u64 = std::env::var("CLASSIFIER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cfg.classifier_timeout_ms);
    let low_conf: f32 = std::env::var("CLASSIFIER_LOW_CONFIDENCE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cfg.classifier_low_confidence);

    // Load the backend once, before the loop (load time excluded from latency).
    let load_start = Instant::now();
    let classifier: Arc<dyn RequestClassifier> = match backend
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "hosted" => {
            let endpoint = std::env::var("CLASSIFIER_ENDPOINT")
                .unwrap_or_else(|_| cfg.classifier_endpoint.clone());
            let model =
                std::env::var("CLASSIFIER_MODEL").unwrap_or_else(|_| "classifier".to_string());
            Arc::new(nasiko_llm_router::routing::HostedClassifier::new(
                endpoint, model,
            ))
        }
        "minilm" | "semantic" => {
            match nasiko_llm_router::routing::SemanticClassifier::load_minilm(
                &cfg.minilm_cache_dir,
                cfg.minilm_min_confidence,
            ) {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    eprintln!("classifier_eval: MiniLM load failed ({e}); regex fallback");
                    build_classifier("regex", None)
                }
            }
        }
        other => build_classifier(other, None),
    };
    let load_ms = load_start.elapsed().as_millis();
    let backend_name = classifier.name().to_string();

    // Cases: EVAL_SET file, else the embedded smoke set (no required arguments).
    let cases: Vec<Case> = match std::env::var("EVAL_SET").ok() {
        Some(path) => {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                eprintln!("classifier_eval: cannot read EVAL_SET={path}: {e}");
                std::process::exit(1);
            });
            let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("classifier_eval: invalid JSON in EVAL_SET={path}: {e}");
                std::process::exit(1);
            });
            let cases = parse_cases(&raw);
            if cases.is_empty() {
                eprintln!("classifier_eval: no cases found in EVAL_SET={path}");
                std::process::exit(1);
            }
            cases
        }
        None => {
            eprintln!("classifier_eval: EVAL_SET unset; running embedded smoke set");
            parse_cases(&smoke_set())
        }
    };

    let out_path = std::env::var("OUT").ok();
    let mut out: Box<dyn std::io::Write> = match &out_path {
        Some(p) => Box::new(std::fs::File::create(p).unwrap_or_else(|e| {
            eprintln!("classifier_eval: cannot create OUT={p}: {e}");
            std::process::exit(1);
        })),
        None => Box::new(std::io::stdout()),
    };

    // Per-case loop: one timed `classify` call each, decisions to OUT/stdout.
    let mut latencies: Vec<u64> = Vec::with_capacity(cases.len());
    let mut fallbacks = 0usize;
    let mut type_hits = 0usize;
    let mut type_total = 0usize;
    let mut cx_exact = 0usize;
    let mut cx_total = 0usize;
    let mut cx_abs_err = 0u64;
    for case in &cases {
        let input = ClassifyInput {
            query: &case.query,
            context: case.context.as_deref(),
        };
        let start = Instant::now();
        let (decision, fell_back) =
            classify_with_fallback(classifier.as_ref(), &input, timeout_ms).await;
        let latency_us = start.elapsed().as_micros() as u64;
        latencies.push(latency_us);
        if fell_back {
            fallbacks += 1;
        }
        let low_conf_fallback = !fell_back && decision.confidence < low_conf;
        if let Some(expected) = case.expected_type {
            type_total += 1;
            if expected == decision.request_type {
                type_hits += 1;
            }
        }
        if let Some(expected) = case.expected_complexity {
            cx_total += 1;
            if expected == decision.complexity {
                cx_exact += 1;
            }
            cx_abs_err += expected.abs_diff(decision.complexity) as u64;
        }
        let tier = match decision.tier() {
            nasiko_llm_router::routing::Tier::Tier1 => "tier_1",
            nasiko_llm_router::routing::Tier::Tier2 => "tier_2",
            nasiko_llm_router::routing::Tier::Tier3 => "tier_3",
        };
        let line = serde_json::json!({
            "id": case.id,
            "request_type": decision.request_type.as_str(),
            "complexity": decision.complexity,
            "confidence": (decision.confidence * 1000.0).round() / 1000.0,
            "latency_us": latency_us,
            "tier": tier,
            "backend": backend_name,
            "fallback": fell_back || low_conf_fallback,
        });
        writeln!(out, "{line}").unwrap_or_else(|e| {
            eprintln!("classifier_eval: write failed: {e}");
            std::process::exit(1);
        });
    }
    drop(out);

    // Aggregate metrics to stderr (OUT carries decisions only).
    eprintln!(
        "classifier_eval: backend={backend_name} cases={} load_ms={} fallback={} ({:.1}%)",
        cases.len(),
        load_ms,
        fallbacks,
        100.0 * fallbacks as f64 / cases.len().max(1) as f64,
    );
    eprintln!(
        "classifier_eval: latency_us p50={} p95={}",
        percentile(latencies.clone(), 0.50),
        percentile(latencies, 0.95),
    );
    if type_total > 0 {
        eprintln!(
            "classifier_eval: request_type accuracy={}/{} ({:.1}%)",
            type_hits,
            type_total,
            100.0 * type_hits as f64 / type_total as f64,
        );
    }
    if cx_total > 0 {
        eprintln!(
            "classifier_eval: complexity exact={}/{} ({:.1}%), MAE={:.2}",
            cx_exact,
            cx_total,
            100.0 * cx_exact as f64 / cx_total as f64,
            cx_abs_err as f64 / cx_total as f64,
        );
    }
}
