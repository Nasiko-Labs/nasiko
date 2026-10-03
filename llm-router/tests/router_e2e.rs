//! Demo-readiness tests for routing: classifier, boundaries, stickiness, determinism.
use std::collections::HashMap;

use nasiko_llm_router::routing::classifier::CellMap;
use nasiko_llm_router::routing::{BoundarySignals, Mode, Phase, RequestType, classify, classify_request_type};
use rand::SeedableRng;
use rand::rngs::StdRng;

#[test]
fn requests_classify_to_expected_types() {
    assert_eq!(classify_request_type("what is the capital of France?"), RequestType::FactualLookup);
    assert_eq!(classify_request_type("hello there"), RequestType::General);
    assert_eq!(classify_request_type("build me a python script that parses CSV"), RequestType::CodeGeneration);
    assert_eq!(classify_request_type("how should I design this API?"), RequestType::TechnicalDesign);
}

#[test]
fn same_seed_same_tier() {
    let cells: CellMap = HashMap::new();
    for q in ["hello there", "design a sharded queue", "fix typo in comment"] {
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
