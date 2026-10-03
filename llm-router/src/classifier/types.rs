//! Core types for request classification.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Context provided to classifiers for inference.
///
/// This structure contains all the information a classifier needs to make a
/// classification decision, including the request text, optional user context,
/// conversation history, and metadata.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use std::time::Instant;
/// use nasiko_llm_router::classifier::InferenceContext;
///
/// let context = InferenceContext {
///     request_text: "Write a function to sort an array".into(),
///     user_context: None,
///     conversation_history: None,
///     metadata: HashMap::new(),
///     received_at: Instant::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceContext {
    /// The request text to classify (required).
    pub request_text: String,

    /// Optional user context (e.g., user ID, preferences).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_context: Option<HashMap<String, String>>,

    /// Optional conversation history for context-aware classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_history: Option<Vec<ConversationTurn>>,

    /// Additional metadata (e.g., request ID, timestamp).
    #[serde(default)]
    pub metadata: HashMap<String, String>,

    /// Timestamp when the request was received (for latency tracking).
    ///
    /// This field is not serialized and is used internally for performance monitoring.
    #[serde(skip)]
    pub received_at: Instant,
}

/// A single turn in a conversation.
///
/// Used in [`InferenceContext`] to provide conversation history for context-aware
/// classification. Classifiers may use this to understand the broader context of
/// the current request.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use nasiko_llm_router::classifier::ConversationTurn;
///
/// let turn = ConversationTurn {
///     role: "user".into(),
///     content: "How do I sort an array?".into(),
///     timestamp: Utc::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Role of the speaker: typically "user" or "assistant".
    pub role: String,

    /// Content of the message.
    pub content: String,

    /// Timestamp when this turn occurred.
    pub timestamp: DateTime<Utc>,
}

/// Result of a classification operation.
///
/// Contains the predicted request type, confidence score, optional reasoning,
/// and metadata about the classification.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use chrono::Utc;
/// use nasiko_llm_router::classifier::ClassificationResult;
/// use nasiko_llm_router::routing::RequestType;
///
/// let result = ClassificationResult {
///     request_type: RequestType::CodeGeneration,
///     confidence: 0.95,
///     reasoning: Some("Request asks to 'create a function'".into()),
///     classifier_name: "hosted-backend".into(),
///     latency: Duration::from_millis(234),
///     timestamp: Utc::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// The predicted request type.
    pub request_type: crate::routing::RequestType,

    /// Confidence score in range [0.0, 1.0].
    ///
    /// Higher values indicate greater confidence in the classification.
    /// Different classifiers may have different confidence scales.
    pub confidence: f32,

    /// Optional reasoning or explanation for the classification.
    ///
    /// Some classifiers (e.g., LLM-based) can provide human-readable
    /// explanations for their decisions. Rule-based classifiers typically
    /// return `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    /// Name of the classifier that produced this result.
    ///
    /// Used for debugging, monitoring, and understanding which classifier
    /// in a fallback chain produced the final result.
    pub classifier_name: String,

    /// Latency of the classification operation.
    ///
    /// Measured from the start of the classify call to completion.
    #[serde(with = "serde_duration")]
    pub latency: Duration,

    /// Timestamp when classification completed.
    pub timestamp: DateTime<Utc>,
}

/// Health status of a classifier.
///
/// Returned by [`RequestClassifier::health_check`](crate::classifier::RequestClassifier::health_check)
/// to indicate whether the classifier is ready to accept requests.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use nasiko_llm_router::classifier::HealthStatus;
///
/// let status = HealthStatus {
///     healthy: true,
///     message: "All systems operational".into(),
///     last_check: Utc::now(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Whether the classifier is healthy and ready to accept requests.
    pub healthy: bool,

    /// Human-readable status message.
    ///
    /// Should explain the current state, especially if `healthy` is `false`.
    pub message: String,

    /// Timestamp of the last health check.
    pub last_check: DateTime<Utc>,
}

/// Metadata describing a classifier implementation.
///
/// Returned by [`RequestClassifier::metadata`](crate::classifier::RequestClassifier::metadata)
/// to provide static information about the classifier.
///
/// # Examples
///
/// ```
/// use nasiko_llm_router::classifier::{ClassifierMetadata, BackendType};
///
/// let metadata = ClassifierMetadata {
///     name: "hosted-backend".into(),
///     version: "1.0.0".into(),
///     backend_type: BackendType::HostedLLM,
///     capabilities: vec!["async".into(), "batch".into(), "reasoning".into()],
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierMetadata {
    /// Name of the classifier.
    pub name: String,

    /// Version of the classifier implementation.
    pub version: String,

    /// Type of backend used by this classifier.
    pub backend_type: BackendType,

    /// List of capabilities supported by this classifier.
    ///
    /// Common capabilities include:
    /// - `"async"` - Supports async operation
    /// - `"batch"` - Efficient batch processing
    /// - `"reasoning"` - Provides reasoning/explanations
    /// - `"few-shot"` - Supports few-shot learning
    /// - `"fast"` - Low-latency operation
    /// - `"always-available"` - No external dependencies
    pub capabilities: Vec<String>,
}

/// Type of backend used by a classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendType {
    /// Hosted LLM service (e.g., AWS Bedrock).
    HostedLLM,

    /// Rule-based pattern matching.
    RuleBased,

    /// Ensemble of multiple classifiers.
    Ensemble,
}

/// Errors that can occur during classification.
///
/// All classifier errors implement this enum to provide a consistent
/// error handling interface across different implementations.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use nasiko_llm_router::classifier::ClassifierError;
///
/// let error = ClassifierError::Timeout(Duration::from_secs(5));
/// println!("Classification failed: {}", error);
/// ```
#[derive(Debug, Error)]
pub enum ClassifierError {
    /// Inference operation failed.
    #[error("Inference failed: {0}")]
    InferenceFailed(String),

    /// Failed to parse classifier response.
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Operation timed out.
    #[error("Timeout after {0:?}")]
    Timeout(Duration),

    /// Rate limit exceeded.
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    /// Circuit breaker is open.
    #[error("Circuit breaker open: {0}")]
    CircuitBreakerOpen(String),

    /// All classifiers in the fallback chain failed.
    #[error("All classifiers failed: {0:?}")]
    AllFailed(Vec<String>),

    /// AWS SDK error.
    #[error("AWS SDK error: {0}")]
    AwsError(String),
}

/// Custom serialization for Duration to handle JSON serialization.
mod serde_duration {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    /// Serialize Duration as milliseconds.
    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    /// Deserialize Duration from milliseconds.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u128::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inference_context_creation() {
        let context = InferenceContext {
            request_text: "test".into(),
            user_context: None,
            conversation_history: None,
            metadata: HashMap::new(),
            received_at: Instant::now(),
        };

        assert_eq!(context.request_text, "test");
        assert!(context.user_context.is_none());
        assert!(context.conversation_history.is_none());
    }

    #[test]
    fn test_classification_result_confidence_bounds() {
        let result = ClassificationResult {
            request_type: crate::routing::RequestType::General,
            confidence: 0.95,
            reasoning: None,
            classifier_name: "test".into(),
            latency: Duration::from_millis(100),
            timestamp: Utc::now(),
        };

        assert!(result.confidence >= 0.0 && result.confidence <= 1.0);
    }

    #[test]
    fn test_health_status_healthy() {
        let status = HealthStatus {
            healthy: true,
            message: "OK".into(),
            last_check: Utc::now(),
        };

        assert!(status.healthy);
        assert_eq!(status.message, "OK");
    }

    #[test]
    fn test_classifier_metadata() {
        let metadata = ClassifierMetadata {
            name: "test-classifier".into(),
            version: "1.0.0".into(),
            backend_type: BackendType::RuleBased,
            capabilities: vec!["fast".into()],
        };

        assert_eq!(metadata.name, "test-classifier");
        assert_eq!(metadata.backend_type, BackendType::RuleBased);
        assert_eq!(metadata.capabilities.len(), 1);
    }

    #[test]
    fn test_backend_type_equality() {
        assert_eq!(BackendType::HostedLLM, BackendType::HostedLLM);
        assert_ne!(BackendType::HostedLLM, BackendType::RuleBased);
        assert_ne!(BackendType::RuleBased, BackendType::Ensemble);
    }

    #[test]
    fn test_classifier_error_display() {
        let error = ClassifierError::InferenceFailed("test failure".into());
        assert_eq!(error.to_string(), "Inference failed: test failure");

        let timeout_error = ClassifierError::Timeout(Duration::from_secs(5));
        assert!(timeout_error.to_string().contains("Timeout"));
    }

    #[test]
    fn test_conversation_turn_creation() {
        let turn = ConversationTurn {
            role: "user".into(),
            content: "Hello".into(),
            timestamp: Utc::now(),
        };

        assert_eq!(turn.role, "user");
        assert_eq!(turn.content, "Hello");
    }
}
