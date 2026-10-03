//! A small, real router vertical slice: decision -> provider tier -> sticky continuation.
//! No answer-model calls. The provider's tier names below are illustrative config overrides.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use nasiko_llm_router::config::{ClassifierConfig, build_classifier};
use nasiko_llm_router::routing::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, Tier, classify_request_type,
};
use nasiko_llm_router::routing::classifier_runtime::ClassifierRuntime;
use nasiko_llm_router::routing::{
    AllowAllGate, BoundarySignals, CachedDecision, DecisionCache, InMemoryCellStore, Mode, Phase,
    RouteInputs, TierRegistry, route_model_with_classifier,
};

#[derive(Default)]
struct DemoCache(Mutex<HashMap<(String, String), CachedDecision>>);

#[async_trait::async_trait]
impl DecisionCache for DemoCache {
    async fn get(&self, conversation: &str, agent: &str) -> Option<CachedDecision> {
        self.0
            .lock()
            .ok()?
            .get(&(conversation.into(), agent.into()))
            .cloned()
    }
    async fn put(&self, conversation: &str, agent: &str, decision: &CachedDecision) {
        if let Ok(mut cache) = self.0.lock() {
            cache.insert((conversation.into(), agent.into()), decision.clone());
        }
    }
}

struct NoRegistry;

#[async_trait::async_trait]
impl TierRegistry for NoRegistry {
    async fn model_for(&self, _: &str, _: Tier) -> Option<String> {
        None
    }
}

/// Explicit fault injection exercises the router's timeout policy without an outage.
struct TimeoutFixture;

#[async_trait::async_trait]
impl RequestClassifier for TimeoutFixture {
    fn name(&self) -> &str {
        "timeout-fixture"
    }
    async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        std::future::pending().await
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("nasiko::llm_router::request_classifier=info")
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .compact()
        .init();
    let runtime = build_classifier(&ClassifierConfig::from_env());
    println!(
        "Nasiko routing demo; backend={}; no answer-model calls",
        runtime.name()
    );
    println!(
        "Simple effort permits all tiers; it does not force a cheap model. Complexity >=4 enforces the strongest tier. These are unvalidated routing hypotheses."
    );
    let cache = DemoCache::default();
    let cells = InMemoryCellStore::new();
    for (id, query, context) in [
        (
            "small",
            "Replace `false` with `true` in this assignment and change nothing else.",
            "const ENABLE_EXPORT = false;",
        ),
        (
            "complex",
            "Design a concurrency-safe recovery protocol and its adversarial tests.",
            "Regions can partition. A payment can succeed after a timeout. Queue deliveries duplicate and reorder; no customer may be charged twice. Explain atomicity, reconciliation, rollout and rollback.",
        ),
    ] {
        println!(
            "\n{id}: regex type={}",
            classify_request_type(query).as_str()
        );
        let mut signals = BoundarySignals {
            conv_id: Some(id.into()),
            phase: Phase::ColdStart,
            mode: Mode::FreeFlowing,
        };
        let before = runtime.counts().0;
        let decision = route_once(&runtime, &cache, &cells, &signals, query, context).await;
        println!(
            "boundary -> source={:?}, tier={:?}, model={}, backend_calls={}",
            decision.source,
            decision.tier,
            decision.model,
            runtime.counts().0 - before
        );
        signals.phase = Phase::Continue;
        let before = runtime.counts().0;
        let continuation = route_once(
            &runtime,
            &cache,
            &cells,
            &signals,
            query,
            "tool result arrived",
        )
        .await;
        println!(
            "tool continuation -> source={:?}, same_model={}, backend_calls={}",
            continuation.source,
            decision.model == continuation.model,
            runtime.counts().0 - before
        );
    }
    let timeout = ClassifierRuntime::new(
        std::sync::Arc::new(TimeoutFixture),
        Duration::from_millis(20),
        0.6,
        42,
    );
    let signals = BoundarySignals {
        conv_id: Some("fault".into()),
        phase: Phase::ColdStart,
        mode: Mode::FreeFlowing,
    };
    let result = route_once(
        &timeout,
        &cache,
        &cells,
        &signals,
        "implement a parser",
        "bounded task",
    )
    .await;
    println!(
        "\nInjected timeout (fixture, not a real Jev outage) -> source={:?}, tier={:?}, fallbacks={}",
        result.source,
        result.tier,
        timeout.counts().1
    );
}

async fn route_once(
    runtime: &ClassifierRuntime,
    cache: &dyn DecisionCache,
    cells: &InMemoryCellStore,
    signals: &BoundarySignals,
    query: &str,
    context: &str,
) -> nasiko_llm_router::routing::RouteDecision {
    route_model_with_classifier(
        cache,
        &NoRegistry,
        cells,
        &AllowAllGate,
        &RouteInputs {
            agent_id: "demo-agent",
            provider: "demo-provider",
            fallback_model: "agent-configured-model",
            has_llm_config: true,
            pinned_model: None,
            tier1_model: Some("provider-strong"),
            tier2_model: Some("provider-medium"),
            tier3_model: Some("provider-small"),
            signals,
            query: Some(query),
        },
        runtime,
        Some(context),
    )
    .await
}
