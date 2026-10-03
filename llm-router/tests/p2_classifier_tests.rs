use nasiko_llm_router::routing::{
    P2ClassifierConfig, P2FastClassifier, TargetModelTier,
};

#[test]
fn test_heuristic_routes_git_status_to_commodity() {
    let classifier = P2FastClassifier::new(P2ClassifierConfig::default());
    let decision = classifier.classify("git status");

    assert_eq!(decision.tier, TargetModelTier::Commodity);
    assert_eq!(decision.recommended_model, "gpt-4o-mini");
    assert!(decision.confidence_score > 0.90);
    assert_eq!(decision.estimated_cost_savings_pct, 97.0);
    assert!(!decision.fail_closed_triggered);
}

#[test]
fn test_heuristic_routes_distributed_raft_to_frontier() {
    let classifier = P2FastClassifier::new(P2ClassifierConfig::default());
    let decision = classifier.classify("Fix split brain deadlock in distributed consensus Raft algorithm implementation");

    assert_eq!(decision.tier, TargetModelTier::Frontier);
    assert_eq!(decision.recommended_model, "gpt-4o");
    assert_eq!(decision.estimated_cost_savings_pct, 0.0);
}

#[test]
fn test_complexity_scoring_on_complex_code() {
    let classifier = P2FastClassifier::new(P2ClassifierConfig::default());
    let prompt = r#"
Please analyze this concurrent Rust code and optimize the lock-free state transitions:
```rust
pub struct ConcurrentQueue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
}
impl<T> ConcurrentQueue<T> {
    pub fn push(&self, val: T) {
        // how to avoid ABA problem and memory ordering invariants?
    }
}
```
Explain trade-offs, deadlock potential, and formal invariants.
"#;
    let decision = classifier.classify(prompt);

    assert_eq!(decision.tier, TargetModelTier::Frontier);
    assert!(decision.complexity_score > 0.60);
}

#[test]
fn test_fail_closed_uncertainty_fallback() {
    // Config with strict confidence threshold
    let config = P2ClassifierConfig {
        fail_closed_confidence_threshold: 0.90,
        commodity_complexity_ceiling: 0.20,
        frontier_complexity_floor: 0.80,
    };
    let classifier = P2FastClassifier::new(config);

    // An ambiguous query in the middle zone (moderate reasoning, no obvious whitelist)
    let ambiguous_query = "What are the common practices for designing a clean notification system schema?";
    let decision = classifier.classify(ambiguous_query);

    // Should fail closed to Frontier to guarantee agent accuracy
    assert_eq!(decision.tier, TargetModelTier::Frontier);
    assert!(decision.fail_closed_triggered);
    assert!(decision.reasoning.contains("Uncertainty fallback triggered"));
}

#[test]
fn test_sub_millisecond_classification_latency() {
    let classifier = P2FastClassifier::new(P2ClassifierConfig::default());
    let prompts = [
        "git status",
        "echo hello world",
        "format this json for me: {\"key\": 1}",
        "what is the time complexity of quicksort?",
        "refactor this database connection pool logic",
    ];

    for prompt in prompts {
        let decision = classifier.classify(prompt);
        // Latency must be sub-millisecond (< 1000 microseconds)
        assert!(
            decision.latency_micros < 1000,
            "Latency exceeded 1ms: {}µs",
            decision.latency_micros
        );
    }
}
