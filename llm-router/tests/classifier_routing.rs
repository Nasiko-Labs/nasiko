//! No model files or external services: prove classification cannot break routing.
use async_trait::async_trait;
use nasiko_llm_router::routing::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier,
};
use nasiko_llm_router::routing::decision::ClassifierRuntime;
use nasiko_llm_router::routing::{self, *};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[derive(Default)]
struct Cache(Mutex<Option<CachedDecision>>);
#[async_trait]
impl DecisionCache for Cache {
    async fn get(&self, _: &str, _: &str) -> Option<CachedDecision> {
        self.0.lock().unwrap().clone()
    }
    async fn put(&self, _: &str, _: &str, d: &CachedDecision) {
        *self.0.lock().unwrap() = Some(d.clone());
    }
}
struct Registry;
#[async_trait]
impl TierRegistry for Registry {
    async fn model_for(&self, provider: &str, tier: Tier) -> Option<String> {
        Some(format!("{provider}-{}", tier.as_level()))
    }
}
struct Classifier(AtomicUsize);
#[async_trait]
impl RequestClassifier for Classifier {
    fn name(&self) -> &str {
        "test"
    }
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        assert_eq!(input.context, Some("edit the code comment"));
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Classification {
            request_type: RequestType::CodeGeneration,
            complexity: 1,
            confidence: 0.9,
        })
    }
}
fn inputs(signals: &BoundarySignals) -> RouteInputs<'_> {
    RouteInputs {
        agent_id: "agent",
        provider: "provider-a",
        fallback_model: "configured",
        has_llm_config: true,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        signals,
        query: Some("yes, fix it"),
    }
}

#[tokio::test]
async fn boundary_classifies_once_and_continuation_uses_cache() {
    let backend = Arc::new(Classifier(AtomicUsize::new(0)));
    let runtime = ClassifierRuntime::new(backend.clone(), Duration::from_secs(1), 0.6, 42, true);
    let cache = Cache::default();
    let cells = InMemoryCellStore::new();
    let mut signals = BoundarySignals {
        conv_id: Some("conversation".into()),
        phase: Phase::ColdStart,
        mode: Mode::FreeFlowing,
    };
    let first = route_model_with_classifier(
        &cache,
        &Registry,
        &cells,
        &AllowAllGate,
        &inputs(&signals),
        &runtime,
        Some("edit the code comment"),
    )
    .await;
    assert_eq!(first.source, RouteSource::Classified);
    assert!(first.model.starts_with("provider-a-"));
    signals.phase = Phase::Continue;
    let second = route_model_with_classifier(
        &cache,
        &Registry,
        &cells,
        &AllowAllGate,
        &inputs(&signals),
        &runtime,
        None,
    )
    .await;
    assert_eq!(second.source, RouteSource::CacheHit);
    assert_eq!(first.model, second.model);
    assert_eq!(backend.0.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn continuation_cache_miss_and_pinned_flow_never_call_model() {
    let backend = Arc::new(Classifier(AtomicUsize::new(0)));
    let runtime = ClassifierRuntime::new(backend.clone(), Duration::from_secs(1), 0.6, 42, true);
    let cells = InMemoryCellStore::new();
    for (phase, mode) in [
        (Phase::Continue, Mode::FreeFlowing),
        (Phase::Switch, Mode::PinnedFlow),
    ] {
        let signals = BoundarySignals {
            conv_id: Some("c".into()),
            phase,
            mode,
        };
        let result = route_model_with_classifier(
            &NoopCache,
            &Registry,
            &cells,
            &AllowAllGate,
            &inputs(&signals),
            &runtime,
            None,
        )
        .await;
        assert_eq!(result.model, "configured");
    }
    let signals = BoundarySignals {
        conv_id: Some("c".into()),
        phase: Phase::Switch,
        mode: Mode::FreeFlowing,
    };
    let mut input = inputs(&signals);
    input.pinned_model = Some("pinned");
    let result = route_model_with_classifier(
        &NoopCache,
        &Registry,
        &cells,
        &AllowAllGate,
        &input,
        &runtime,
        None,
    )
    .await;
    assert_eq!(result.model, "pinned");
    assert_eq!(backend.0.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn repeated_opt_in_decisions_are_reproducible_and_override_still_wins() {
    let runtime = ClassifierRuntime::new(
        Arc::new(Classifier(AtomicUsize::new(0))),
        Duration::from_secs(1),
        0.6,
        42,
        true,
    );
    let cells = InMemoryCellStore::new();
    let signals = BoundarySignals {
        conv_id: Some("c".into()),
        phase: Phase::Switch,
        mode: Mode::FreeFlowing,
    };
    let mut models = std::collections::HashSet::new();
    for _ in 0..10 {
        let result = route_model_with_classifier(
            &NoopCache,
            &Registry,
            &cells,
            &AllowAllGate,
            &inputs(&signals),
            &runtime,
            Some("edit the code comment"),
        )
        .await;
        models.insert(result.model);
    }
    assert_eq!(models.len(), 1);
    let mut input = inputs(&signals);
    input.tier1_model = Some("override");
    input.tier2_model = Some("override");
    input.tier3_model = Some("override");
    let result = route_model_with_classifier(
        &NoopCache,
        &Registry,
        &cells,
        &AllowAllGate,
        &input,
        &runtime,
        Some("edit the code comment"),
    )
    .await;
    assert_eq!(result.model, "override");
}

#[tokio::test]
async fn default_backend_preserves_existing_nonclassification_decisions() {
    let runtime = ClassifierRuntime::default();
    let cells = InMemoryCellStore::new();
    let signals = BoundarySignals::inert();
    let old = routing::route_model(
        &NoopCache,
        &Registry,
        &cells,
        &AllowAllGate,
        &inputs(&signals),
    )
    .await;
    let new = route_model_with_classifier(
        &NoopCache,
        &Registry,
        &cells,
        &AllowAllGate,
        &inputs(&signals),
        &runtime,
        None,
    )
    .await;
    assert_eq!(old.model, new.model);
    assert_eq!(old.source, new.source);
    assert_eq!(old.tier, new.tier);
}

#[tokio::test]
async fn semantic_backend_preserves_gate_verdict_and_supplies_context() {
    struct RejectGate;
    #[async_trait]
    impl SalienceGate for RejectGate {
        async fn is_substantive(&self, query: &str) -> bool {
            assert_eq!(query, "edit the code comment\nCurrent message: yes, fix it");
            false
        }
    }
    let backend = Arc::new(Classifier(AtomicUsize::new(0)));
    let runtime = ClassifierRuntime::new(backend.clone(), Duration::from_secs(1), 0.35, 42, true);
    let signals = BoundarySignals {
        conv_id: Some("c".into()),
        phase: Phase::ColdStart,
        mode: Mode::FreeFlowing,
    };
    let cache = Cache::default();
    let result = route_model_with_classifier(
        &cache,
        &Registry,
        &InMemoryCellStore::new(),
        &RejectGate,
        &inputs(&signals),
        &runtime,
        Some("edit the code comment"),
    )
    .await;
    assert_eq!(result.source, RouteSource::SmallTalk);
    assert_eq!(backend.0.load(Ordering::Relaxed), 0);
    assert!(cache.0.lock().unwrap().is_none());
}
