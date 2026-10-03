//! Live Jev checks — **opt-in**, paid, networked. Ignored by default so ordinary
//! `cargo test` never needs a key or egress.
//!
//! ```sh
//! TYPESAFE_API_KEY=… cargo test -p nasiko-llm-router --test jev_live -- --ignored --nocapture
//! ```
//!
//! What these establish, and only when run:
//! - the adapter's request shape is accepted by the real endpoint and the response
//!   validates against the documented contract;
//! - the versioned model id answers and reports itself;
//! - repeatability across fresh requests for identical input (reported, not assumed —
//!   Jev documents no seed);
//! - option-order sensitivity (documented jaggedness of jev-1.13) on a few dev cases, as a
//!   robustness measurement, never as a change selected against held-out data.
use std::collections::BTreeMap;

use nasiko_llm_router::config::{ClassifierBackend, ClassifierConfig, Secret};
use nasiko_llm_router::routing::classifier::{ClassifyInput, RequestClassifier};
use nasiko_llm_router::routing::jev::{JevClassifier, OptionOrder};

fn live_config() -> Option<ClassifierConfig> {
    let key = std::env::var("TYPESAFE_API_KEY").ok()?;
    if key.trim().is_empty() {
        return None;
    }
    let mut cfg = ClassifierConfig::from_env();
    cfg.backend = ClassifierBackend::Jev;
    cfg.api_key = Secret::new(key);
    Some(cfg)
}

const SAMPLES: [(&str, Option<&str>); 4] = [
    (
        "Fix typo in this Python comment: `# retrun the cached value`.",
        Some("No other files or changes needed."),
    ),
    (
        "What does `Option::take()` do in Rust? Answer in one sentence.",
        Some("No codebase context."),
    ),
    (
        "Explain why this function returns the old value, not the incremented value.",
        Some("```rust\nfn next(n: &mut u64) -> u64 { let old = *n; *n += 1; old }\n```"),
    ),
    (
        "Design migration from synchronous payment-status callbacks to queued processing without changing public API semantics.",
        Some("Today POST /confirm returns 200 only after provider charge."),
    ),
];

#[tokio::test]
#[ignore = "live, paid: needs TYPESAFE_API_KEY and egress to the configured endpoint"]
async fn live_request_shape_is_accepted_and_response_validates() {
    let Some(cfg) = live_config() else {
        eprintln!("skipped: TYPESAFE_API_KEY not set");
        return;
    };
    let jev = JevClassifier::new(&cfg).expect("init");
    for (query, context) in SAMPLES {
        let out = jev
            .classify_detailed(&ClassifyInput { query, context })
            .await
            .expect("live classification");
        let d = out.diagnostics.expect("diagnostics");
        eprintln!(
            "{:?} cx={} p={:.3} vendor_conf={:?} model={:?} tokens_in={:?} :: {}",
            out.classification.request_type,
            out.classification.complexity,
            out.classification.confidence,
            d.vendor_type_confidence,
            d.model_version,
            d.input_tokens,
            query
        );
        assert_eq!(
            d.model_version.as_deref(),
            Some(cfg.model.as_str()),
            "a versioned id must answer as itself"
        );
    }
}

#[tokio::test]
#[ignore = "live, paid: needs TYPESAFE_API_KEY and egress to the configured endpoint"]
async fn live_repeatability_across_fresh_requests() {
    let Some(cfg) = live_config() else {
        eprintln!("skipped: TYPESAFE_API_KEY not set");
        return;
    };
    let jev = JevClassifier::new(&cfg).expect("init");
    let runs = 3;
    let mut label_changes = 0;
    let mut max_prob_delta = 0.0f32;
    for (query, context) in SAMPLES {
        let mut seen: Vec<(String, u8, f32)> = Vec::new();
        for _ in 0..runs {
            let out = jev
                .classify(&ClassifyInput { query, context })
                .await
                .expect("live");
            seen.push((
                out.request_type.as_str().to_string(),
                out.complexity,
                out.confidence,
            ));
        }
        let labels: BTreeMap<&str, usize> =
            seen.iter().fold(BTreeMap::new(), |mut m, (l, _, _)| {
                *m.entry(l.as_str()).or_default() += 1;
                m
            });
        if labels.len() > 1 {
            label_changes += 1;
        }
        let lo = seen.iter().map(|s| s.2).fold(f32::INFINITY, f32::min);
        let hi = seen.iter().map(|s| s.2).fold(f32::NEG_INFINITY, f32::max);
        max_prob_delta = max_prob_delta.max(hi - lo);
        eprintln!("{query:.50}… -> {seen:?}");
    }
    eprintln!(
        "repeatability over {} inputs × {runs} runs: inputs with a label change = {label_changes}; max confidence spread = {max_prob_delta:.4}",
        SAMPLES.len()
    );
    // Reported, not asserted: the point is to measure variance, not to hide it.
}

#[tokio::test]
#[ignore = "live, paid: needs TYPESAFE_API_KEY and egress to the configured endpoint"]
async fn live_option_order_sensitivity() {
    let Some(cfg) = live_config() else {
        eprintln!("skipped: TYPESAFE_API_KEY not set");
        return;
    };
    let canonical = JevClassifier::new(&cfg).expect("init");
    let reversed = JevClassifier::new(&cfg)
        .expect("init")
        .with_option_order(OptionOrder::Reversed);
    let mut disagreements = 0;
    for (query, context) in SAMPLES {
        let a = canonical
            .classify(&ClassifyInput { query, context })
            .await
            .expect("live");
        let b = reversed
            .classify(&ClassifyInput { query, context })
            .await
            .expect("live");
        if a.request_type != b.request_type {
            disagreements += 1;
        }
        eprintln!(
            "{query:.50}… canonical={} ({:.2}) reversed={} ({:.2})",
            a.request_type.as_str(),
            a.confidence,
            b.request_type.as_str(),
            b.confidence
        );
    }
    eprintln!(
        "option-order disagreements: {disagreements}/{}",
        SAMPLES.len()
    );
}
