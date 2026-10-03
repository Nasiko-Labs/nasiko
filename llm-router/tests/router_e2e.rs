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

use async_trait::async_trait;
use nasiko_llm_router::routing::{
    AllowAllGate, CachedDecision, Classification, ClassifyInput, DecisionCache, HostedClassifier,
    InMemoryCellStore, RequestClassifier, RouteInputs, RouteSource, Tier, TierRegistry,
    route_model,
};
use std::sync::{Arc, Mutex};

struct TestCache {
    decisions: Mutex<HashMap<(String, String), CachedDecision>>,
}

impl TestCache {
    fn new() -> Self {
        Self {
            decisions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl DecisionCache for TestCache {
    async fn get(&self, conv_id: &str, agent_id: &str) -> Option<CachedDecision> {
        self.decisions
            .lock()
            .unwrap()
            .get(&(conv_id.to_string(), agent_id.to_string()))
            .cloned()
    }

    async fn put(&self, conv_id: &str, agent_id: &str, decision: &CachedDecision) {
        self.decisions.lock().unwrap().insert(
            (conv_id.to_string(), agent_id.to_string()),
            decision.clone(),
        );
    }
}

struct TestTierRegistry;

#[async_trait]
impl TierRegistry for TestTierRegistry {
    async fn model_for(&self, provider: &str, tier: Tier) -> Option<String> {
        Some(match tier {
            Tier::Tier1 => format!("{provider}-tier1"),
            Tier::Tier2 => format!("{provider}-tier2"),
            Tier::Tier3 => format!("{provider}-tier3"),
        })
    }
}

struct CountingClassifier {
    rt: RequestType,
    calls: Mutex<usize>,
}

#[async_trait]
impl RequestClassifier for CountingClassifier {
    async fn classify(&self, _input: ClassifyInput) -> Classification {
        *self.calls.lock().unwrap() += 1;
        Classification {
            request_type: self.rt,
            complexity: 4,
            confidence: 0.99,
        }
    }
}

#[tokio::test]
async fn routing_using_request_classifier() {
    let cache = TestCache::new();
    let registry = TestTierRegistry;
    let cells = InMemoryCellStore::new();
    let gate = AllowAllGate;
    let classifier = Arc::new(CountingClassifier {
        rt: RequestType::Writing,
        calls: Mutex::new(0),
    });

    let signals = BoundarySignals {
        conv_id: Some("conv-1".into()),
        phase: Phase::Switch,
        mode: Mode::FreeFlowing,
    };

    let inputs = RouteInputs {
        agent_id: "agent-1",
        provider: "openai",
        fallback_model: "fallback",
        has_llm_config: true,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        signals: &signals,
        query: Some("rewrite this article"),
    };

    let decision = route_model(
        &cache,
        &registry,
        &cells,
        &gate,
        classifier.clone(),
        &inputs,
    )
    .await;

    assert_eq!(decision.source, RouteSource::Classified);
    assert_eq!(*classifier.calls.lock().unwrap(), 1);

    let cached = cache.get("conv-1", "agent-1").await.expect("cached");
    assert_eq!(cached.request_type, Some(RequestType::Writing));
    assert_eq!(cached.model, decision.model);
}

#[tokio::test]
async fn sticky_continuation_does_not_reclassify() {
    let cache = TestCache::new();
    let registry = TestTierRegistry;
    let cells = InMemoryCellStore::new();
    let gate = AllowAllGate;
    let classifier = Arc::new(CountingClassifier {
        rt: RequestType::CodeGeneration,
        calls: Mutex::new(0),
    });

    // Turn 1: Cold start / switch -> fires classification
    let turn1_signals = BoundarySignals {
        conv_id: Some("conv-sticky".into()),
        phase: Phase::Switch,
        mode: Mode::FreeFlowing,
    };
    let turn1_inputs = RouteInputs {
        agent_id: "agent-1",
        provider: "anthropic",
        fallback_model: "fallback",
        has_llm_config: true,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        signals: &turn1_signals,
        query: Some("write code"),
    };

    let d1 = route_model(
        &cache,
        &registry,
        &cells,
        &gate,
        classifier.clone(),
        &turn1_inputs,
    )
    .await;
    assert_eq!(d1.source, RouteSource::Classified);
    assert_eq!(*classifier.calls.lock().unwrap(), 1);

    // Turn 2: Continue phase (e.g. tool loop or continuation turn)
    let turn2_signals = BoundarySignals {
        conv_id: Some("conv-sticky".into()),
        phase: Phase::Continue,
        mode: Mode::FreeFlowing,
    };
    let turn2_inputs = RouteInputs {
        agent_id: "agent-1",
        provider: "anthropic",
        fallback_model: "fallback",
        has_llm_config: true,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        signals: &turn2_signals,
        query: Some("more code please"),
    };

    let d2 = route_model(
        &cache,
        &registry,
        &cells,
        &gate,
        classifier.clone(),
        &turn2_inputs,
    )
    .await;

    // Cache hit: same model reused, classifier count is STILL 1
    assert_eq!(d2.source, RouteSource::CacheHit);
    assert_eq!(d2.model, d1.model);
    assert_eq!(*classifier.calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn router_fallback_when_hosted_classifier_fails() {
    let cache = TestCache::new();
    let registry = TestTierRegistry;
    let cells = InMemoryCellStore::new();
    let gate = AllowAllGate;

    let hosted = Arc::new(
        HostedClassifier::new(
            "http://127.0.0.1:1/nonexistent".to_string(),
            "model".to_string(),
            "key".to_string(),
            std::time::Duration::from_millis(50),
        )
        .expect("hosted"),
    );

    let signals = BoundarySignals {
        conv_id: Some("conv-fallback".into()),
        phase: Phase::Switch,
        mode: Mode::FreeFlowing,
    };
    let inputs = RouteInputs {
        agent_id: "agent-1",
        provider: "openai",
        fallback_model: "fallback",
        has_llm_config: true,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        signals: &signals,
        query: Some("write a python function to parse json"),
    };

    let decision = route_model(&cache, &registry, &cells, &gate, hosted, &inputs).await;

    assert_eq!(decision.source, RouteSource::Classified);
    let cached = cache.get("conv-fallback", "agent-1").await.expect("cached");
    assert_eq!(cached.request_type, Some(RequestType::CodeGeneration));
}
