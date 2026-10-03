//! Request classifier eval — the scorer's harness, running the router's configured classifier.
//!
//! Run:
//!   EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example classifier_eval
//!
//! Reads cases from `EVAL_SET` and writes one JSONL line of outputs per case
//! to `OUT`. It does not compute scores; our scorer does that.
//!
//! The backend is chosen exactly as in the router, by `REQUEST_CLASSIFIER`:
//! - `regex` (default): the original vote counter; complexity is the neutral 3.
//! - `local`: the in-process hashed n-gram model embedded in the binary (`assets/`).
//! - `laya`: the `laya-serve` sidecar at `LAYA_URL` (default `http://localhost:8000`), e.g.
//!   `python -m pip install 'laya[serve]==0.3.24' && LAYA_MODELS=multilingual laya-serve`.
//!   Any Laya failure falls back to regex for that case, never aborting the run;
//!   `LAYA_TIMEOUT_MS` (default 1500, the router's budget).
//! - `strands`: type-only Strands inference, probability confidence, local complexity.
//! - `strands_raw`: native two-question Strands, for baseline comparisons.
//! - `hybrid`: local-first classifier with optimized Strands for uncertain types.
//!
//! A case's optional `context` string is given to model-backed classifiers after the
//! query — the request first, so a long context can never truncate the request away.
use std::io::Write;
use std::time::Instant;

use nasiko_llm_router::routing::ClassifierInput;
use nasiko_llm_router::routing::local_classifier::eval_state;

#[tokio::main]
async fn main() {
    let path = std::env::var("EVAL_SET").expect("set EVAL_SET to the eval JSON path");
    let out_path = std::env::var("OUT").unwrap_or_else(|_| "classifier-out.jsonl".into());
    let raw = std::fs::read_to_string(&path).expect("read EVAL_SET");
    let data: serde_json::Value = serde_json::from_str(&raw).expect("valid eval JSON");
    let examples = data["examples"].as_array().expect("examples array");

    let cfg = nasiko_llm_router::GatewayConfig::from_env();
    let classifier = nasiko_llm_router::build_request_classifier(&cfg, &reqwest::Client::new());
    classifier
        .classify(&ClassifierInput {
            query: "hello",
            state: "Latest request:\nhello",
        })
        .await;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("create OUT"));
    let mut fallbacks = 0usize;
    for example in examples {
        let id = example["id"].as_str().expect("id");
        let query = example["query"].as_str().expect("query");
        let state = eval_state(query, example["context"].as_str());
        let started = Instant::now();
        let c = classifier
            .classify(&ClassifierInput {
                query,
                state: &state,
            })
            .await;
        let latency_us = started.elapsed().as_micros() as u64;
        if let Some(reason) = c.source.fallback_reason() {
            fallbacks += 1;
            eprintln!("{id}: classifier fallback ({reason})");
        }
        let complexity_source = match c.source {
            nasiko_llm_router::routing::ClassifierSource::Strands
                if cfg
                    .request_classifier
                    .trim()
                    .eq_ignore_ascii_case("strands_raw") =>
            {
                "strands"
            }
            nasiko_llm_router::routing::ClassifierSource::Strands
            | nasiko_llm_router::routing::ClassifierSource::HybridStrands
            | nasiko_llm_router::routing::ClassifierSource::Local => "local",
            nasiko_llm_router::routing::ClassifierSource::Laya => "laya",
            _ => "regex",
        };
        let type_probabilities = c.type_probabilities.as_ref().map(|probabilities| {
            probabilities
                .iter()
                .map(|(kind, p)| (kind.as_str(), *p))
                .collect::<std::collections::BTreeMap<_, _>>()
        });
        let line = serde_json::json!({
            "id": id,
            "request_type": c.request_type.as_str(),
            "complexity": c.complexity,
            "confidence": c.confidence,
            "latency_us": latency_us,
            "source": c.source.as_str(),
            "fallback_reason": c.source.fallback_reason(),
            "complexity_source": complexity_source,
            "type_probabilities": type_probabilities,
            "low_confidence": c.is_low_confidence(),
        });
        writeln!(out, "{line}").expect("write OUT");
    }
    out.flush().expect("flush OUT");
    eprintln!(
        "{} cases -> {out_path} ({fallbacks} fell back to regex)",
        examples.len()
    );
}
