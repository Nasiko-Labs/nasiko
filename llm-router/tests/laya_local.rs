//! Real local Laya inference — **opt-in**: needs the ONNX bundle and an ONNX Runtime library
//! (`llm-router/scripts/laya-setup.sh`). Ignored by default so ordinary `cargo test` stays
//! model-free.
//!
//! ```sh
//! CLASSIFIER_MODEL_PATH=.laya/model CLASSIFIER_ORT_DYLIB=.laya/onnxruntime/lib/libonnxruntime.1.28.0.dylib \
//! cargo test -p nasiko-llm-router --test laya_local -- --ignored --nocapture
//! ```
//!
//! Establishes, when run: the bundle loads; the Rust sequence builder + session reproduce the
//! pinned PyTorch checkpoint's probabilities (`tests/data/classifier/laya-reference-public.json`,
//! recorded from the `laya` Python package) on the public sample; repeatability across fresh
//! calls; and the regex fallback on a timeout.
use std::time::{Duration, Instant};

use nasiko_llm_router::config::{ClassifierBackend, ClassifierConfig};
use nasiko_llm_router::routing::classifier::{ClassifyInput, RequestClassifier};
use nasiko_llm_router::routing::classifier_eval::read_eval_cases;
use nasiko_llm_router::routing::laya::LayaClassifier;
use nasiko_llm_router::routing::{ClassifierService, Disposition, FallbackReason};

fn local_config() -> Option<ClassifierConfig> {
    let model_path = std::env::var("CLASSIFIER_MODEL_PATH").ok()?;
    let mut cfg = ClassifierConfig::from_env();
    cfg.backend = ClassifierBackend::Laya;
    cfg.model_path = model_path;
    Some(cfg)
}

#[derive(serde::Deserialize)]
struct Reference {
    cases: Vec<RefCase>,
}
#[derive(serde::Deserialize)]
struct RefCase {
    id: String,
    choice: String,
    type_probabilities: std::collections::BTreeMap<String, f32>,
    complexity_probabilities: Vec<f32>,
}

const PUBLIC_SAMPLE: &str = include_str!("data/classifier/public-sample-v1.json");

#[tokio::test]
#[ignore = "local model: needs CLASSIFIER_MODEL_PATH (and CLASSIFIER_ORT_DYLIB) from laya-setup.sh"]
async fn local_bundle_reproduces_the_pytorch_reference_on_the_public_sample() {
    let Some(cfg) = local_config() else {
        eprintln!("skipped: CLASSIFIER_MODEL_PATH not set");
        return;
    };
    let t0 = Instant::now();
    let laya = LayaClassifier::new(&cfg).expect("bundle loads");
    eprintln!(
        "loaded {} in {:?} ({} MB weights)",
        laya.info().model_version,
        t0.elapsed(),
        laya.info().weights_bytes / 1_000_000
    );
    let reference: Reference =
        serde_json::from_str(include_str!("data/classifier/laya-reference-public.json")).unwrap();
    let cases = read_eval_cases(PUBLIC_SAMPLE).unwrap();
    let mut max_diff = 0.0f32;
    let mut latencies = Vec::new();
    for case in &cases {
        let r = reference
            .cases
            .iter()
            .find(|r| r.id == case.id)
            .expect("reference row");
        let t = Instant::now();
        let out = laya
            .classify_detailed(&ClassifyInput {
                query: &case.query,
                context: case.context.as_deref(),
            })
            .await
            .expect("inference");
        latencies.push(t.elapsed().as_millis() as u64);
        let d = out.diagnostics.unwrap();
        assert_eq!(
            out.classification.request_type.as_str(),
            r.choice,
            "{}",
            case.id
        );
        for (rt, p) in &d.type_probabilities {
            let want = r.type_probabilities[rt.as_str()];
            max_diff = max_diff.max((p - want).abs());
        }
        for (p, want) in d
            .complexity_probabilities
            .iter()
            .zip(&r.complexity_probabilities)
        {
            max_diff = max_diff.max((p - want).abs());
        }
        eprintln!(
            "{} {:?} cx={} p={:.3} tokens={:?} truncated={} {}ms",
            case.id,
            out.classification.request_type,
            out.classification.complexity,
            out.classification.confidence,
            d.input_tokens,
            d.input_truncated,
            latencies.last().unwrap()
        );
    }
    latencies.sort_unstable();
    eprintln!(
        "max |Δp| vs PyTorch reference = {max_diff:.5}; p50 = {} ms, p95 = {} ms",
        latencies[latencies.len() / 2],
        latencies[(latencies.len() * 95 / 100).min(latencies.len() - 1)]
    );
    // The reference rounds to 4 decimals; the ONNX export differs from PyTorch by ~5e-5.
    assert!(max_diff < 2e-3, "parity drift: {max_diff}");
}

#[tokio::test]
#[ignore = "local model: needs CLASSIFIER_MODEL_PATH (and CLASSIFIER_ORT_DYLIB) from laya-setup.sh"]
async fn local_inference_is_repeatable_and_times_out_into_regex() {
    let Some(cfg) = local_config() else {
        eprintln!("skipped: CLASSIFIER_MODEL_PATH not set");
        return;
    };
    let service = ClassifierService::from_config(&cfg);
    assert_eq!(service.status().effective, "laya");
    let input = ClassifyInput {
        query: "Explain why this closure needs `move`.",
        context: Some("```rust\nlet t = std::thread::spawn(move || println!(\"{name}\"));\n```"),
    };
    let a = service.classify(&input).await;
    let b = service.classify(&input).await;
    assert_eq!(a.disposition, Disposition::Primary);
    assert_eq!(
        a.classification, b.classification,
        "same input must give the same verdict"
    );
    assert_eq!(a.diagnostics, b.diagnostics);

    // An impossible deadline: the service falls back to regex and counts a timeout, while
    // the queued job finishes on its own without blocking anything.
    let mut tight = cfg.clone();
    tight.timeout_ms = 1;
    let strict = ClassifierService::from_config(&tight);
    let out = strict.classify(&input).await;
    assert_eq!(out.fallback_reason(), Some(FallbackReason::Timeout));
    assert_eq!(out.answered_by, "regex");
    tokio::time::sleep(Duration::from_secs(2)).await;
}
