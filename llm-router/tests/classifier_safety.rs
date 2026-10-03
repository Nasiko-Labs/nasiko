use async_trait::async_trait;
use nasiko_llm_router::routing::classifier::{
    Classification, GuardedClassifier, RequestClassifier,
};
use nasiko_llm_router::routing::{RequestType, classifier_context};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

struct Reply {
    confidence: f32,
    delay: Duration,
}
#[async_trait]
impl RequestClassifier for Reply {
    async fn classify(&self, _: &str, _: &str) -> Result<Classification, String> {
        tokio::time::sleep(self.delay).await;
        Ok(Classification {
            request_type: RequestType::Writing,
            complexity: 2,
            confidence: self.confidence,
            fallback_reason: None,
            decision_cost_usd: Some(0.00002),
        })
    }
}

#[tokio::test]
async fn timeout_returns_keyword_result_and_counts_fallback() {
    let classifier = GuardedClassifier::new(
        Arc::new(Reply {
            confidence: 0.9,
            delay: Duration::from_secs(1),
        }),
        Duration::from_millis(5),
        0.6,
    );
    let result = classifier
        .classify("write me a Python sort function", "")
        .await
        .unwrap();
    assert_eq!(result.request_type, RequestType::CodeGeneration);
    assert_eq!(result.fallback_reason, Some("timeout"));
    assert_eq!(classifier.fallbacks.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn low_confidence_preserves_reported_decision_cost() {
    let classifier = GuardedClassifier::new(
        Arc::new(Reply {
            confidence: 0.2,
            delay: Duration::ZERO,
        }),
        Duration::from_secs(1),
        0.6,
    );
    let result = classifier
        .classify("write me a Python sort function", "")
        .await
        .unwrap();
    assert_eq!(result.request_type, RequestType::CodeGeneration);
    assert_eq!(result.fallback_reason, Some("low_confidence"));
    assert_eq!(result.decision_cost_usd, Some(0.00002));
    assert_eq!(classifier.fallbacks.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn invalid_probability_uses_keyword_fallback() {
    let classifier = GuardedClassifier::new(
        Arc::new(Reply {
            confidence: f32::NAN,
            delay: Duration::ZERO,
        }),
        Duration::from_secs(1),
        0.6,
    );
    let result = classifier
        .classify("write me a Python sort function", "")
        .await
        .unwrap();
    assert_eq!(result.request_type, RequestType::CodeGeneration);
    assert_eq!(result.fallback_reason, Some("invalid_output"));
}

#[test]
fn context_retains_recent_text_under_long_instructions_and_keeps_request_intact() {
    let messages: Vec<nasiko_llm_router::ir::Message> = serde_json::from_value(serde_json::json!([
        {"role":"system","content":"instruction".repeat(100)},
        {"role":"user","content":"Earlier question"},
        {"role":"assistant","content":"Relevant prior answer"},
        {"role":"user","content":"Explain that"}
    ]))
    .unwrap();
    let before = serde_json::to_value(&messages).unwrap();
    let context = classifier_context(&messages, 100);
    assert!(context.contains("Relevant prior answer"));
    assert!(!context.contains("Explain that"));
    assert!(context.chars().count() <= 100);
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
    assert_eq!(classifier_context(&messages, 0), "");
}
