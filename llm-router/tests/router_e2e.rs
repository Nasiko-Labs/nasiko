//! Demo-readiness tests for routing: classifier, boundaries, stickiness, determinism.
use std::collections::HashMap;

use nasiko_llm_router::routing::classifier::CellMap;
use nasiko_llm_router::routing::{
    BoundarySignals, Mode, Phase, RequestType, classify, classify_request_type,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

#[test]
fn requests_classify_to_expected_types() {
    assert_eq!(
        classify_request_type("what is the capital of France?"),
        RequestType::FactualLookup
    );
    assert_eq!(classify_request_type("hello there"), RequestType::General);
    assert_eq!(
        classify_request_type("build me a python script that parses CSV"),
        RequestType::CodeGeneration
    );
    assert_eq!(
        classify_request_type("how should I design this API?"),
        RequestType::TechnicalDesign
    );
}

#[test]
fn same_seed_same_tier() {
    let cells: CellMap = HashMap::new();
    for q in [
        "hello there",
        "design a sharded queue",
        "fix typo in comment",
    ] {
        let a = classify(q, "openai", &cells, &mut StdRng::seed_from_u64(7));
        let b = classify(q, "openai", &cells, &mut StdRng::seed_from_u64(7));
        assert_eq!(a, b, "non-deterministic for {q}");
    }
}

#[test]
fn phase_and_mode_parse_safely() {
    assert_eq!(Phase::from_label("COLD_START"), Phase::ColdStart);
    assert_eq!(Phase::from_label("switch"), Phase::Switch);
    assert_eq!(Phase::from_label(""), Phase::Continue);
    assert_eq!(Phase::from_label("garbage"), Phase::Continue);
    assert_eq!(Mode::from_label("pinned_flow"), Mode::PinnedFlow);
    assert_eq!(Mode::from_label("x"), Mode::FreeFlowing);
}

#[test]
fn only_free_flowing_boundaries_fire() {
    assert!(!BoundarySignals::inert().is_fireable_boundary());
    assert!(BoundarySignals::in_flow("f".into(), Mode::FreeFlowing).is_fireable_boundary());
    assert!(!BoundarySignals::in_flow("f".into(), Mode::PinnedFlow).is_fireable_boundary());
}

#[test]
fn tool_loop_stays_sticky() {
    let first = BoundarySignals::for_coding_agent("a", 1, Some("refactor this"), false);
    let loop_turn = BoundarySignals::for_coding_agent("a", 1, Some("refactor this"), true);
    assert!(first.is_fireable_boundary());
    assert!(!loop_turn.is_fireable_boundary());
    assert_eq!(first.conv_id, loop_turn.conv_id);
    let next = BoundarySignals::for_coding_agent("a", 2, Some("now add tests"), false);
    assert_ne!(first.conv_id, next.conv_id);
}

// --- pluggable request classifier (CLASSIFIER_BACKEND) ---

use nasiko_llm_router::routing::classifier::{REGEX_COMPLEXITY, REGEX_CONFIDENCE};
use nasiko_llm_router::routing::{ClassifyInput, ClassifyOutcome, select_tier};
use nasiko_llm_router::{ClassifierBackend, GatewayConfig, build_request_classifier};

const QUERIES: &[&str] = &[
    "hello there",
    "build me a python script that parses CSV",
    "explain what this function does",
    "how should I design this API?",
    "calculate the probability that it rains tomorrow",
    "draft an email to my team about the outage",
    "what is the capital of France?",
    "Draft a Python script that renames files",
    "",
];

#[tokio::test]
async fn default_config_is_regex_and_routes_exactly_like_before() {
    // With no env set the backend is regex, and the trait path (classifier + select_tier)
    // yields the same request type and the same Thompson-sampled tier as the legacy
    // `classify` for every query and seed.
    let cfg = GatewayConfig::default();
    assert_eq!(cfg.classifier_backend, ClassifierBackend::Regex);
    let guard = build_request_classifier(&cfg);
    assert_eq!(guard.backend_name(), "regex");
    let cells: CellMap = HashMap::new();
    for q in QUERIES {
        let (c, outcome) = guard
            .classify(&ClassifyInput {
                query: q,
                context: None,
            })
            .await;
        assert_eq!(outcome, ClassifyOutcome::Backend);
        assert_eq!(c.request_type, classify_request_type(q));
        assert_eq!(
            (c.complexity, c.confidence),
            (REGEX_COMPLEXITY, REGEX_CONFIDENCE)
        );
        for seed in 0..20 {
            let legacy = classify(q, "openai", &cells, &mut StdRng::seed_from_u64(seed));
            let tier = select_tier(
                q,
                c.request_type,
                "openai",
                &cells,
                &mut StdRng::seed_from_u64(seed),
            );
            assert_eq!(
                (tier, c.request_type),
                legacy,
                "diverged on {q:?} seed {seed}"
            );
        }
    }
    assert_eq!(guard.fallbacks(), 0);
}

#[tokio::test]
async fn regex_backend_ignores_context() {
    let guard = build_request_classifier(&GatewayConfig::default());
    let q = "what does this do";
    let (bare, _) = guard
        .classify(&ClassifyInput {
            query: q,
            context: None,
        })
        .await;
    let (with_ctx, _) = guard
        .classify(&ClassifyInput {
            query: q,
            context: Some("```rust\nfn main() {}\n```"),
        })
        .await;
    assert_eq!(bare, with_ctx);
}

#[tokio::test]
async fn nb_backend_is_selected_by_config_and_is_deterministic() {
    let cfg = GatewayConfig {
        classifier_backend: ClassifierBackend::NaiveBayes,
        ..GatewayConfig::default()
    };
    let a = build_request_classifier(&cfg);
    let b = build_request_classifier(&cfg);
    assert_eq!(a.backend_name(), "nb");
    for q in QUERIES {
        let input = ClassifyInput {
            query: q,
            context: Some("no other context"),
        };
        let (x, ox) = a.classify(&input).await;
        let (y, _) = a.classify(&input).await;
        let (z, _) = b.classify(&input).await;
        assert_eq!(ox, ClassifyOutcome::Backend);
        assert_eq!(x, y, "same instance, different answer for {q:?}");
        assert_eq!(x, z, "different instances, different answer for {q:?}");
        assert!((0.0..=1.0).contains(&x.confidence) && (1..=5).contains(&x.complexity));
    }
    // The near-miss the regex gets wrong ("draft" ⇒ writing).
    let (c, _) = a
        .classify(&ClassifyInput {
            query: "Draft a Python script that renames files",
            context: None,
        })
        .await;
    assert_eq!(c.request_type, RequestType::CodeGeneration);
}
