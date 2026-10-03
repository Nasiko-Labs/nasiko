//! Request classifier eval.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case to `OUT`. It does
//! not compute scores; our scorer does that.
//!
//! The backend is built by the router's own `build_classifier` from the same env vars the router
//! reads (`config.rs`), and every case goes through the same `ClassifierChain::decide` the router
//! calls at Level 3 — so timeouts, the regex fallback and low-confidence handling are exercised
//! exactly as in production.
//!
//! | env                         | default  | meaning                                        |
//! |-----------------------------|----------|------------------------------------------------|
//! | `CLASSIFIER_BACKEND`        | `local`  | `regex` (baseline) · `local` · `hosted`         |
//! | `CLASSIFIER_MODEL_PATH`     | embedded | `local`: model JSON from `classifier_train`     |
//! | `CLASSIFIER_ENDPOINT`       | —        | `hosted`: OpenAI-compatible API base URL        |
//! | `CLASSIFIER_MODEL`          | —        | `hosted`: model id                              |
//! | `CLASSIFIER_API_KEY`        | —        | `hosted`: bearer key (never commit it)          |
//! | `CLASSIFIER_TIMEOUT_MS`     | `800`    | per-call deadline, then regex fallback          |
//! | `CLASSIFIER_MIN_CONFIDENCE` | `0.3`    | below it, regex fallback (`low_confidence`)     |
//!
//! The eval defaults to `local`; the router itself defaults to `regex`. Output lines add
//! `backend` and `fallback` (null, `error`, `timeout`, `low_confidence`, `load`) to the contract
//! fields. Load time, p50/p95 latency and fallback counts go to stderr.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::GatewayConfig;
use nasiko_llm_router::routing::ClassifyInput;

fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let mut cfg = GatewayConfig::from_env();
    if std::env::var("CLASSIFIER_BACKEND").is_err() {
        cfg.classifier_backend = "local".into();
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Load once, before the loop; per-call latency excludes it.
    let load_started = Instant::now();
    let chain = nasiko_llm_router::build_classifier(&cfg, reqwest::Client::new());
    let load_ms = load_started.elapsed().as_secs_f64() * 1e3;

    let mut latencies = Vec::with_capacity(examples.len());
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let context = example["context"].as_str().filter(|c| !c.trim().is_empty());
        let started = Instant::now();
        let decision = rt.block_on(chain.decide(&ClassifyInput { query, context }));
        let latency_us = started.elapsed().as_micros() as u64;
        latencies.push(latency_us);
        let c = decision.classification;
        let line = serde_json::json!({
            "id": id,
            "request_type": c.request_type.as_str(),
            "complexity": c.complexity,
            "confidence": (c.confidence as f64 * 1e4).round() / 1e4,
            "latency_us": latency_us,
            "backend": chain.backend(),
            "fallback": decision.fallback.map(|f| f.as_str()),
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");

    latencies.sort_unstable();
    let pct = |p: f64| {
        latencies
            .get(((latencies.len() as f64 * p).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0)
    };
    let s = chain.stats();
    eprintln!(
        "classifier_eval: backend={} cases={} load={load_ms:.1}ms p50={}us p95={}us \
         fallbacks: errors={} timeouts={} low_confidence={} → {out_path}",
        chain.backend(),
        examples.len(),
        pct(0.50),
        pct(0.95),
        s.errors,
        s.timeouts,
        s.low_confidence,
    );
}
