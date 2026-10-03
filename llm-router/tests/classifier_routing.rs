//! Request-classifier integration: backend selection from config, fallback to the regex, and
//! the guarded path the router and the eval share.
use nasiko_llm_router::routing::{ClassifyInput, RegexClassifier, RequestClassifier, RequestType};
use nasiko_llm_router::{ClassifierConfig, build_request_classifier};

fn build(cfg: ClassifierConfig) -> std::sync::Arc<nasiko_llm_router::GuardedClassifier> {
    build_request_classifier(&cfg, &reqwest::Client::new())
}

fn input(q: &str) -> ClassifyInput<'_> {
    ClassifyInput {
        query: q,
        context: None,
    }
}

#[tokio::test]
async fn factory_defaults_to_regex() {
    let c = build(ClassifierConfig::default());
    assert_eq!(c.name(), "regex");
    let q = "draft an email to my team about the outage";
    assert_eq!(
        c.classify(&input(q)).await.unwrap(),
        RegexClassifier::classify_sync(q)
    );
    assert_eq!(c.stats().snapshot().fallbacks(), 0);
}

#[tokio::test]
async fn factory_selects_local_backend() {
    let c = build(ClassifierConfig {
        backend: "local".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "local");
    let v = c
        .classify(&input("Fix typo in this comment"))
        .await
        .unwrap();
    assert!((1..=5).contains(&v.complexity));
    assert!((0.0..=1.0).contains(&v.confidence));
    assert_eq!(c.stats().snapshot().fallback_load, 0);
}

#[tokio::test]
async fn factory_bad_model_path_degrades_to_regex_and_counts_load_fallback() {
    let c = build(ClassifierConfig {
        backend: "local".into(),
        model_path: "/nonexistent/weights.json".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "regex");
    assert_eq!(c.stats().snapshot().fallback_load, 1);
    let v = c.classify(&input("hello there")).await.unwrap();
    assert_eq!(v.request_type, RequestType::General);
}

#[tokio::test]
async fn factory_unknown_backend_degrades_to_regex() {
    let c = build(ClassifierConfig {
        backend: "quantum".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "regex");
}

#[tokio::test]
async fn factory_hosted_without_endpoint_degrades_to_regex() {
    let c = build(ClassifierConfig {
        backend: "hosted".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "regex");
    assert_eq!(c.stats().snapshot().fallback_load, 1);
}

#[tokio::test]
async fn hosted_network_failure_falls_back_to_regex_and_counts_it() {
    // Port 9 (discard) on loopback: nothing listens, so the connection is refused.
    let c = build(ClassifierConfig {
        backend: "hosted".into(),
        endpoint: "http://127.0.0.1:9/v1".into(),
        model: "any".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "hosted");
    let q = "what is the capital of France?";
    let v = c.classify(&input(q)).await.unwrap();
    assert_eq!(v, RegexClassifier::classify_sync(q));
    assert_eq!(c.stats().snapshot().fallback_error, 1);
}

#[tokio::test]
async fn cascade_without_hosted_runs_local_alone() {
    let c = build(ClassifierConfig {
        backend: "cascade".into(),
        ..Default::default()
    });
    assert_eq!(c.name(), "local");
}
